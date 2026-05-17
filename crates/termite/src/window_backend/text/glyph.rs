use ab_glyph::{Font, GlyphId, PxScale, ScaleFont, point};

use crate::runner::{TerminalMetrics, TextRenderConfig};

use super::font::LoadedFont;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct GlyphKey {
    pub(super) font: usize,
    pub(super) ch: char,
    pub(super) columns: u8,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ShapedGlyphKey {
    pub(super) font: usize,
    pub(super) glyph_id: u16,
}

pub(super) struct GlyphBitmap {
    pub(super) left: i16,
    pub(super) top: i16,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) alpha: Vec<u8>,
}

#[inline]
pub(super) fn glyph_metrics(base: TerminalMetrics, columns: u16) -> TerminalMetrics {
    TerminalMetrics {
        cell_width: base.cell_width * u32::from(columns.max(1)),
        cell_height: base.cell_height,
    }
}

pub(super) fn rasterize_glyph(
    font: &LoadedFont,
    ch: char,
    metrics: TerminalMetrics,
    render: TextRenderConfig,
) -> GlyphBitmap {
    rasterize_scaled_glyph(font, font.scale, ch, metrics, render)
}

fn rasterize_scaled_glyph(
    font: &LoadedFont,
    scale: PxScale,
    ch: char,
    metrics: TerminalMetrics,
    render: TextRenderConfig,
) -> GlyphBitmap {
    let scaled = font.font.as_scaled(scale);
    let glyph_id = scaled.glyph_id(ch);
    let advance = scaled.h_advance(glyph_id);
    rasterize_positioned_glyph(font, scale, glyph_id, advance, metrics, render, true)
}

pub(super) fn rasterize_glyph_id(
    font: &LoadedFont,
    glyph_id: GlyphId,
    metrics: TerminalMetrics,
    render: TextRenderConfig,
) -> GlyphBitmap {
    rasterize_positioned_glyph(font, font.scale, glyph_id, 0.0, metrics, render, false)
}

fn rasterize_positioned_glyph(
    font: &LoadedFont,
    scale: PxScale,
    glyph_id: GlyphId,
    advance: f32,
    metrics: TerminalMetrics,
    render: TextRenderConfig,
    center_x: bool,
) -> GlyphBitmap {
    let scaled = font.font.as_scaled(scale);
    let x = if center_x {
        ((metrics.cell_width as f32 - advance) * 0.5)
            .floor()
            .max(-1.0)
    } else {
        0.0
    };
    let baseline = ((metrics.cell_height as f32 - scaled.height()) * 0.5 + scaled.ascent()).round();
    let glyph = glyph_id.with_scale_and_position(scale, point(x, baseline));
    let Some(outlined) = scaled.outline_glyph(glyph) else {
        return GlyphBitmap {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            alpha: Vec::new(),
        };
    };

    let bounds = outlined.px_bounds();
    let width = bounds.width().max(0.0) as u16;
    let height = bounds.height().max(0.0) as u16;
    let mut alpha = vec![0; usize::from(width) * usize::from(height)];
    outlined.draw(|x, y, coverage| {
        let index =
            usize::try_from(y).unwrap_or(0) * usize::from(width) + usize::try_from(x).unwrap_or(0);
        if let Some(alpha) = alpha.get_mut(index) {
            let (weight, gamma) = if font.symbol {
                (render.symbol_weight, render.symbol_gamma)
            } else {
                (render.text_weight, render.text_gamma)
            };
            let coverage = coverage.powf(gamma) * weight;
            *alpha = coverage.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8;
        }
    });

    let left = bounds.min.x.floor() as i16;
    let top = clamp_glyph_axis(
        bounds.min.y.floor() as i16,
        height,
        metrics.cell_height as u16,
    );

    GlyphBitmap {
        left,
        top,
        width,
        height,
        alpha,
    }
}

fn clamp_glyph_axis(offset: i16, glyph_size: u16, cell_size: u16) -> i16 {
    let glyph_size = glyph_size as i16;
    let cell_size = cell_size as i16;
    if glyph_size <= 0 || cell_size <= 0 {
        return offset;
    }
    if glyph_size <= cell_size {
        return offset.clamp(0, cell_size - glyph_size);
    }
    (cell_size - glyph_size) / 2
}
