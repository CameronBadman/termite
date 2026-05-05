use std::{collections::HashMap, fs};

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use c_term_core::{Cell, Style};
use font8x8::{
    BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, HIRAGANA_FONTS, LATIN_FONTS, MISC_FONTS,
    SGA_FONTS, UnicodeFonts,
};

use crate::{
    runner::{FontConfig, TerminalMetrics, TextRenderConfig},
    theme::Theme,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphKey {
    font: usize,
    ch: char,
    columns: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FontRole {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontRole {
    fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }
}

struct LoadedFont {
    font: FontArc,
    role: FontRole,
    symbol: bool,
    scale: PxScale,
}

struct GlyphBitmap {
    left: i16,
    top: i16,
    width: u16,
    height: u16,
    alpha: Vec<u8>,
}

pub(super) struct TextRenderer {
    fonts: Vec<LoadedFont>,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
    metrics: TerminalMetrics,
    text_render: TextRenderConfig,
    theme: Theme,
}

#[derive(Clone, Copy)]
pub(super) struct CellPaint {
    pub(super) fg: [u8; 3],
    pub(super) bg: [u8; 3],
    pub(super) metrics: TerminalMetrics,
}

#[derive(Clone, Copy)]
struct GlyphPaint {
    color: [u8; 3],
    x_shift: i16,
    origin_metrics: TerminalMetrics,
    glyph_metrics: TerminalMetrics,
}

impl TextRenderer {
    pub(super) fn new(
        font: FontConfig,
        theme: Theme,
        metrics: TerminalMetrics,
        text_render: TextRenderConfig,
    ) -> Self {
        Self {
            fonts: load_fonts(font),
            glyphs: HashMap::new(),
            metrics,
            text_render,
            theme,
        }
    }

    pub(super) fn draw_row_to_frame(
        &mut self,
        row: &[Cell],
        frame: &mut [u8],
        width: usize,
        y: u16,
        cols: u16,
    ) {
        let mut x = 0;
        while x < cols {
            let cell = row.get(usize::from(x)).copied().unwrap_or_default();
            if cell.spacer {
                let bg = self.theme.color(cell.style.background);
                fill_cell(frame, width, x, y, bg, self.metrics);
                x += 1;
                continue;
            }

            let base_columns = if cell.wide && x + 1 < cols { 2 } else { 1 };
            let columns = self.display_columns(row, x, cols, cell, base_columns);
            self.draw_cell(frame, width, x, y, cell, columns);
            x += columns;
        }
    }

    fn display_columns(
        &self,
        row: &[Cell],
        x: u16,
        cols: u16,
        cell: Cell,
        base_columns: u16,
    ) -> u16 {
        if base_columns > 1 || !is_private_use_symbol(cell.ch) || x + 1 >= cols {
            return base_columns;
        }

        let Some(key) = self.glyph_key(cell.ch, cell.style, base_columns) else {
            return base_columns;
        };
        if !self.fonts[key.font].symbol {
            return base_columns;
        }

        let next = row.get(usize::from(x + 1)).copied().unwrap_or_default();
        if next.ch == ' '
            && !next.wide
            && !next.spacer
            && next.style.background == cell.style.background
        {
            2
        } else {
            base_columns
        }
    }

    fn draw_cell(
        &mut self,
        frame: &mut [u8],
        width: usize,
        cell_x: u16,
        cell_y: u16,
        cell: Cell,
        columns: u16,
    ) {
        let ch = cell.ch;
        let style = cell.style;
        let fg = self.theme.color(style.foreground);
        let bg = self.theme.color(style.background);
        let paint = CellPaint {
            fg,
            bg,
            metrics: self.metrics,
        };
        if columns == 1 && draw_special_cell(frame, width, cell_x, cell_y, ch, paint) {
            return;
        }

        fill_cell_span(frame, width, cell_x, cell_y, columns, bg, self.metrics);
        if ch != ' ' {
            if let Some(key) = self.glyph_key(ch, style, columns) {
                let synthetic_bold = style.bold && !self.fonts[key.font].role.is_bold();
                let glyph_metrics = glyph_metrics(self.metrics, columns);
                self.ensure_glyph(key, glyph_metrics);
                if let Some(glyph) = self.glyphs.get(&key) {
                    draw_glyph_bitmap(
                        frame,
                        width,
                        cell_x,
                        cell_y,
                        glyph,
                        GlyphPaint {
                            color: fg,
                            x_shift: 0,
                            origin_metrics: self.metrics,
                            glyph_metrics,
                        },
                    );
                    if synthetic_bold {
                        draw_glyph_bitmap(
                            frame,
                            width,
                            cell_x,
                            cell_y,
                            glyph,
                            GlyphPaint {
                                color: fg,
                                x_shift: 1,
                                origin_metrics: self.metrics,
                                glyph_metrics,
                            },
                        );
                    }
                }
            } else {
                draw_bitmap_cell(frame, width, cell_x, cell_y, ch, style.bold, paint);
            }
        }
        if style.underline {
            draw_underline_span(frame, width, cell_x, cell_y, columns, fg, self.metrics);
        }
    }

    fn glyph_key(&self, ch: char, style: Style, columns: u16) -> Option<GlyphKey> {
        let columns = columns.min(u16::from(u8::MAX)) as u8;
        self.font_index(ch, preferred_font_roles(style))
            .map(|font| GlyphKey { font, ch, columns })
            .or_else(|| {
                ascii_glyph_fallback(ch).and_then(|ch| {
                    self.font_index(ch, preferred_font_roles(style))
                        .map(|font| GlyphKey { font, ch, columns })
                })
            })
            .or_else(|| {
                self.font_index('?', preferred_font_roles(style))
                    .map(|font| GlyphKey {
                        font,
                        ch: '?',
                        columns,
                    })
            })
    }

    fn font_index(&self, ch: char, roles: &[FontRole]) -> Option<usize> {
        for role in roles {
            if let Some(index) = self
                .fonts
                .iter()
                .position(|font| font.role == *role && font.font.glyph_id(ch) != GlyphId(0))
            {
                return Some(index);
            }
        }
        self.fonts
            .iter()
            .position(|font| font.font.glyph_id(ch) != GlyphId(0))
    }

    fn ensure_glyph(&mut self, key: GlyphKey, metrics: TerminalMetrics) {
        if self.glyphs.contains_key(&key) {
            return;
        }
        let glyph = rasterize_glyph(&self.fonts[key.font], key.ch, metrics, self.text_render);
        self.glyphs.insert(key, glyph);
    }
}

fn glyph_metrics(base: TerminalMetrics, columns: u16) -> TerminalMetrics {
    TerminalMetrics {
        cell_width: base.cell_width * u32::from(columns.max(1)),
        cell_height: base.cell_height,
    }
}

fn load_fonts(font: FontConfig) -> Vec<LoadedFont> {
    let FontConfig::GlyphAtlas { paths, size } = font else {
        return Vec::new();
    };

    let mut loaded = Vec::new();
    for path in paths {
        let path = path;
        if let Ok(bytes) = fs::read(&path)
            && let Ok(font) = FontArc::try_from_vec(bytes)
        {
            loaded.push(LoadedFont {
                font,
                role: font_role_from_path(&path),
                symbol: is_symbol_font_path(&path),
                scale: PxScale::from(size),
            });
        }
    }
    loaded
}

fn preferred_font_roles(style: Style) -> &'static [FontRole] {
    match (style.bold, style.italic) {
        (true, true) => &[
            FontRole::BoldItalic,
            FontRole::Bold,
            FontRole::Italic,
            FontRole::Regular,
        ],
        (true, false) => &[FontRole::Bold, FontRole::BoldItalic, FontRole::Regular],
        (false, true) => &[FontRole::Italic, FontRole::BoldItalic, FontRole::Regular],
        (false, false) => &[FontRole::Regular],
    }
}

fn font_role_from_path(path: &str) -> FontRole {
    let lower = path.to_ascii_lowercase();
    let bold = lower.contains("bold");
    let italic = lower.contains("italic") || lower.contains("oblique");
    match (bold, italic) {
        (true, true) => FontRole::BoldItalic,
        (true, false) => FontRole::Bold,
        (false, true) => FontRole::Italic,
        (false, false) => FontRole::Regular,
    }
}

fn rasterize_glyph(
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
    let x = ((metrics.cell_width as f32 - advance) * 0.5)
        .floor()
        .max(-1.0);
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

    let left = clamp_glyph_axis(
        bounds.min.x.floor() as i16,
        width,
        metrics.cell_width as u16,
    );
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

fn is_symbol_font_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("symbolsnerd")
        || lower.contains("symbols-nerd")
        || lower.contains("nerd-font")
        || lower.contains("standardsymbols")
}

fn is_private_use_symbol(ch: char) -> bool {
    matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

fn fill_cell(
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

fn fill_cell_span(
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

fn draw_glyph_bitmap(
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
            let cell_px = glyph.left + x as i16 + paint.x_shift;
            if !(0..glyph_metrics.cell_width as i16).contains(&cell_px) {
                continue;
            }
            let alpha = glyph.alpha[usize::from(y) * usize::from(glyph.width) + usize::from(x)];
            if alpha == 0 {
                continue;
            }
            let index = ((origin_y + cell_py as usize) * width + origin_x + cell_px as usize) * 4;
            blend_pixel(&mut frame[index..index + 4], paint.color, alpha);
        }
    }
}

fn blend_pixel(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    let alpha = u16::from(alpha);
    let inv = 255 - alpha;
    for (channel, color) in pixel[..3].iter_mut().zip(color) {
        *channel = ((u16::from(*channel) * inv + u16::from(color) * alpha) / 255) as u8;
    }
    pixel[3] = 0xff;
}

fn draw_bitmap_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    bold: bool,
    paint: CellPaint,
) {
    let metrics = paint.metrics;
    let glyph = bitmap_glyph(ch)
        .or_else(|| ascii_glyph_fallback(ch).and_then(bitmap_glyph))
        .unwrap_or_else(|| bitmap_glyph('?').unwrap_or([0; 8]));
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    for py in 0..metrics.cell_height as usize {
        let glyph_y = sample_bitmap_axis(py, metrics.cell_height as usize);
        let glyph_row = glyph[glyph_y];
        for px in 0..metrics.cell_width as usize {
            let glyph_x = sample_bitmap_axis(px, metrics.cell_width as usize);
            let bit = ((glyph_row >> glyph_x) & 1) != 0
                || (bold && glyph_x > 0 && ((glyph_row >> (glyph_x - 1)) & 1) != 0);
            let color = if bit { paint.fg } else { paint.bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

pub(super) fn sample_bitmap_axis(pixel: usize, cell_size: usize) -> usize {
    ((pixel * 8 + cell_size / 2) / cell_size).min(7)
}

pub(super) fn ascii_glyph_fallback(ch: char) -> Option<char> {
    match ch {
        '‘' | '’' | '‚' | '‛' => Some('\''),
        '“' | '”' | '„' | '‟' => Some('"'),
        '‐' | '‑' | '‒' | '–' | '—' | '―' => Some('-'),
        '…' => Some('.'),
        _ => None,
    }
}

pub(super) fn bitmap_glyph(ch: char) -> Option<[u8; 8]> {
    BASIC_FONTS
        .get(ch)
        .or_else(|| LATIN_FONTS.get(ch))
        .or_else(|| BOX_FONTS.get(ch))
        .or_else(|| BLOCK_FONTS.get(ch))
        .or_else(|| MISC_FONTS.get(ch))
        .or_else(|| GREEK_FONTS.get(ch))
        .or_else(|| HIRAGANA_FONTS.get(ch))
        .or_else(|| SGA_FONTS.get(ch))
}

fn draw_underline_span(
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

fn draw_special_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_block_cell(frame, width, cell_x, cell_y, ch, paint)
        || draw_shade_cell(frame, width, cell_x, cell_y, ch, paint)
        || draw_box_cell(frame, width, cell_x, cell_y, ch, paint)
}

pub(super) fn draw_block_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    let metrics = paint.metrics;
    let cell_width = metrics.cell_width as usize;
    let cell_height = metrics.cell_height as usize;
    let mut regions = [(0, 0, 0, 0); 2];
    let region_count = match ch {
        '█' => {
            regions[0] = (0, 0, cell_width, cell_height);
            1
        }
        '▀' => {
            regions[0] = (0, 0, cell_width, cell_height / 2);
            1
        }
        '▄' => {
            regions[0] = (0, cell_height / 2, cell_width, cell_height / 2);
            1
        }
        '▌' => {
            regions[0] = (0, 0, cell_width / 2, cell_height);
            1
        }
        '▐' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height);
            1
        }
        '▘' => {
            regions[0] = (0, 0, cell_width / 2, cell_height / 2);
            1
        }
        '▝' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height / 2);
            1
        }
        '▖' => {
            regions[0] = (0, cell_height / 2, cell_width / 2, cell_height / 2);
            1
        }
        '▗' => {
            regions[0] = (
                cell_width / 2,
                cell_height / 2,
                cell_width / 2,
                cell_height / 2,
            );
            1
        }
        '▚' => {
            regions[0] = (0, 0, cell_width / 2, cell_height / 2);
            regions[1] = (
                cell_width / 2,
                cell_height / 2,
                cell_width / 2,
                cell_height / 2,
            );
            2
        }
        '▞' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width / 2, cell_height / 2);
            2
        }
        '▙' => {
            regions[0] = (0, 0, cell_width / 2, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width, cell_height / 2);
            2
        }
        '▛' => {
            regions[0] = (0, 0, cell_width, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width / 2, cell_height / 2);
            2
        }
        '▜' => {
            regions[0] = (0, 0, cell_width, cell_height / 2);
            regions[1] = (
                cell_width / 2,
                cell_height / 2,
                cell_width / 2,
                cell_height / 2,
            );
            2
        }
        '▟' => {
            regions[0] = (cell_width / 2, 0, cell_width / 2, cell_height / 2);
            regions[1] = (0, cell_height / 2, cell_width, cell_height / 2);
            2
        }
        _ => return false,
    };

    fill_cell(frame, width, cell_x, cell_y, paint.bg, metrics);
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    for &(x, y, region_width, region_height) in &regions[..region_count] {
        for py in y..y + region_height {
            for px in x..x + region_width {
                let index = ((origin_y + py) * width + origin_x + px) * 4;
                frame[index..index + 4].copy_from_slice(&[
                    paint.fg[0],
                    paint.fg[1],
                    paint.fg[2],
                    0xff,
                ]);
            }
        }
    }
    true
}

pub(super) fn draw_shade_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    let metrics = paint.metrics;
    let threshold = match ch {
        '░' => 1,
        '▒' => 2,
        '▓' => 3,
        _ => return false,
    };
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    for py in 0..metrics.cell_height as usize {
        for px in 0..metrics.cell_width as usize {
            let pattern = (px + py * 3) & 3;
            let color = if pattern < threshold {
                paint.fg
            } else {
                paint.bg
            };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
    true
}

pub(super) fn draw_box_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    let metrics = paint.metrics;
    let Some((left, right, up, down)) = box_segments(ch) else {
        return false;
    };
    let origin_x = usize::from(cell_x) * metrics.cell_width as usize;
    let origin_y = usize::from(cell_y) * metrics.cell_height as usize;
    let center_x = metrics.cell_width as usize / 2;
    let center_y = metrics.cell_height as usize / 2;
    let thickness = 2;

    for py in 0..metrics.cell_height as usize {
        for px in 0..metrics.cell_width as usize {
            let horizontal = py.abs_diff(center_y) < thickness
                && ((left && px <= center_x) || (right && px >= center_x));
            let vertical = px.abs_diff(center_x) < thickness
                && ((up && py <= center_y) || (down && py >= center_y));
            let color = if horizontal || vertical {
                paint.fg
            } else {
                paint.bg
            };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
    true
}

pub(super) fn box_segments(ch: char) -> Option<(bool, bool, bool, bool)> {
    match ch {
        '─' | '━' | '╌' | '╍' | '⎺' | '⎻' | '⎼' | '⎽' => {
            Some((true, true, false, false))
        }
        '╴' => Some((true, false, false, false)),
        '╶' => Some((false, true, false, false)),
        '│' | '┃' | '╎' | '╏' | '┆' | '┇' | '┊' | '┋' => {
            Some((false, false, true, true))
        }
        '╵' => Some((false, false, true, false)),
        '╷' => Some((false, false, false, true)),
        '┌' | '┏' | '╭' => Some((false, true, false, true)),
        '┐' | '┓' | '╮' => Some((true, false, false, true)),
        '└' | '┗' | '╰' => Some((false, true, true, false)),
        '┘' | '┛' | '╯' => Some((true, false, true, false)),
        '├' | '┣' => Some((false, true, true, true)),
        '┤' | '┫' => Some((true, false, true, true)),
        '┬' | '┳' => Some((true, true, false, true)),
        '┴' | '┻' => Some((true, true, true, false)),
        '┼' | '╋' => Some((true, true, true, true)),
        _ => None,
    }
}
