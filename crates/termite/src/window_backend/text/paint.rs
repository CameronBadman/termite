use crate::runner::TerminalMetrics;

use super::glyph::GlyphBitmap;

#[derive(Clone, Copy)]
pub(in crate::window_backend) struct CellPaint {
    pub(in crate::window_backend) fg: [u8; 3],
    pub(in crate::window_backend) bg: [u8; 3],
    pub(in crate::window_backend) background_opaque: bool,
    pub(in crate::window_backend) metrics: TerminalMetrics,
}

#[derive(Clone, Copy)]
pub(super) struct GlyphPaint {
    pub(super) color: [u8; 3],
    pub(super) x_shift: f32,
    pub(super) origin_metrics: TerminalMetrics,
    pub(super) glyph_metrics: TerminalMetrics,
}

#[inline]
pub(super) fn fill_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    color: [u8; 3],
    metrics: TerminalMetrics,
) {
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    for py in 0..metrics.cell_height as usize {
        for px in 0..metrics.cell_width as usize {
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

#[inline]
pub(super) fn fill_row(
    frame: &mut [u8],
    width: usize,
    y: u16,
    color: [u8; 3],
    metrics: TerminalMetrics,
) {
    let row_start = usize::from(y) * metrics.cell_height as usize * width * 4;
    let row_len = metrics.cell_height as usize * width * 4;
    if let Some(row) = frame.get_mut(row_start..row_start + row_len) {
        for pixel in row.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

#[inline]
pub(super) fn fill_cell_span(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    columns: u16,
    color: [u8; 3],
    metrics: TerminalMetrics,
) {
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    let pixel_width = metrics.cell_width as usize * usize::from(columns.max(1));
    for py in 0..metrics.cell_height as usize {
        for px in 0..pixel_width {
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

#[inline]
pub(super) fn draw_glyph_bitmap(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    glyph: &GlyphBitmap,
    paint: GlyphPaint,
) {
    let origin_metrics = paint.origin_metrics;
    let glyph_metrics = paint.glyph_metrics;
    let origin_x = usize::from(cell_x) * origin_metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * origin_metrics.cell_height as usize;
    for y in 0..glyph.height {
        let cell_py = glyph.top + y as i16;
        if !(0..glyph_metrics.cell_height as i16).contains(&cell_py) {
            continue;
        }
        for x in 0..glyph.width {
            let cell_px = glyph.left as f32 + x as f32 + paint.x_shift;
            if cell_px < 0.0 || cell_px >= glyph_metrics.cell_width as f32 {
                continue;
            }
            let frame_x = origin_x as i32 + cell_px.floor() as i32;
            if frame_x < 0 || frame_x >= width as i32 {
                continue;
            }
            let alpha = glyph.alpha[usize::from(y) * usize::from(glyph.width) + usize::from(x)];
            if alpha == 0 {
                continue;
            }
            let index = ((origin_y + cell_py as usize) * width + frame_x as usize) * 4;
            if alpha == u8::MAX {
                frame[index..index + 4].copy_from_slice(&[
                    paint.color[0],
                    paint.color[1],
                    paint.color[2],
                    0xff,
                ]);
            } else {
                blend_pixel(&mut frame[index..index + 4], paint.color, alpha);
            }
        }
    }
}

#[inline]
fn blend_pixel(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    if alpha == 0 {
        return;
    }
    if pixel[3] == 0 {
        pixel[..3].copy_from_slice(&color);
        pixel[3] = alpha;
        return;
    }
    if alpha == u8::MAX {
        pixel[..3].copy_from_slice(&color);
        pixel[3] = u8::MAX;
        return;
    }

    let src_alpha = u16::from(alpha);
    let dst_alpha = u16::from(pixel[3]);
    if dst_alpha == 255 {
        let inv = 255 - src_alpha;
        for (channel, color) in pixel[..3].iter_mut().zip(color) {
            *channel = ((u16::from(*channel) * inv + u16::from(color) * src_alpha) / 255) as u8;
        }
        return;
    }

    let inv = 255 - src_alpha;
    let out_alpha = src_alpha + (dst_alpha * inv + 127) / 255;
    if out_alpha == 0 {
        return;
    }
    for (channel, color) in pixel[..3].iter_mut().zip(color) {
        let src = u16::from(color) * src_alpha;
        let dst = (u16::from(*channel) * dst_alpha * inv + 127) / 255;
        *channel = ((src + dst + out_alpha / 2) / out_alpha) as u8;
    }
    pixel[3] = out_alpha as u8;
}

#[inline]
pub(super) fn draw_underline_span(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    columns: u16,
    color: [u8; 3],
    metrics: TerminalMetrics,
) {
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    let y = origin_y + metrics.cell_height as usize - 2;
    let pixel_width = metrics.cell_width as usize * usize::from(columns.max(1));
    for px in 0..pixel_width {
        let index = (y * width + origin_x + px) * 4;
        frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
    }
}
