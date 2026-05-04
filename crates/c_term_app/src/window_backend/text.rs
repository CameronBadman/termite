use std::{collections::HashMap, fs};

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use c_term_core::{Cell, Style};
use font8x8::{
    BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, HIRAGANA_FONTS, LATIN_FONTS, MISC_FONTS,
    SGA_FONTS, UnicodeFonts,
};

use crate::{runner::FontConfig, theme::Theme};

use super::{CELL_HEIGHT, CELL_WIDTH};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphKey {
    font: usize,
    ch: char,
}

struct LoadedFont {
    font: FontArc,
    scale: PxScale,
    ascent: f32,
    height: f32,
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
    theme: Theme,
}

impl TextRenderer {
    pub(super) fn new(font: FontConfig, theme: Theme) -> Self {
        Self {
            fonts: load_fonts(font),
            glyphs: HashMap::new(),
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
        for x in 0..cols {
            let cell = row.get(usize::from(x)).copied().unwrap_or_default();
            let ch = if cell.spacer { ' ' } else { cell.ch };
            self.draw_cell(frame, width, x, y, ch, cell.style);
        }
    }

    fn draw_cell(
        &mut self,
        frame: &mut [u8],
        width: usize,
        cell_x: u16,
        cell_y: u16,
        ch: char,
        style: Style,
    ) {
        let fg = self.theme.color(style.foreground);
        let bg = self.theme.color(style.background);
        if draw_box_cell(frame, width, cell_x, cell_y, ch, fg, bg) {
            return;
        }

        fill_cell(frame, width, cell_x, cell_y, bg);
        if ch != ' ' {
            if let Some(key) = self.glyph_key(ch) {
                self.ensure_glyph(key);
                if let Some(glyph) = self.glyphs.get(&key) {
                    draw_glyph_bitmap(frame, width, cell_x, cell_y, glyph, fg, 0);
                    if style.bold {
                        draw_glyph_bitmap(frame, width, cell_x, cell_y, glyph, fg, 1);
                    }
                }
            } else {
                draw_bitmap_cell(frame, width, cell_x, cell_y, ch, (fg, bg), style.bold);
            }
        }
        if style.underline {
            draw_underline(frame, width, cell_x, cell_y, fg);
        }
    }

    fn glyph_key(&self, ch: char) -> Option<GlyphKey> {
        self.font_index(ch)
            .map(|font| GlyphKey { font, ch })
            .or_else(|| self.font_index('?').map(|font| GlyphKey { font, ch: '?' }))
    }

    fn font_index(&self, ch: char) -> Option<usize> {
        self.fonts
            .iter()
            .position(|font| font.font.glyph_id(ch) != GlyphId(0))
    }

    fn ensure_glyph(&mut self, key: GlyphKey) {
        if self.glyphs.contains_key(&key) {
            return;
        }
        let glyph = rasterize_glyph(&self.fonts[key.font], key.ch);
        self.glyphs.insert(key, glyph);
    }
}

fn load_fonts(font: FontConfig) -> Vec<LoadedFont> {
    let FontConfig::GlyphAtlas { path, size } = font else {
        return Vec::new();
    };

    let mut loaded = Vec::new();
    if let Ok(bytes) = fs::read(path)
        && let Ok(font) = FontArc::try_from_vec(bytes)
    {
        let scaled = font.as_scaled(size);
        let ascent = scaled.ascent();
        let height = scaled.height();
        loaded.push(LoadedFont {
            font,
            scale: PxScale::from(size),
            ascent,
            height,
        });
    }
    loaded
}

fn rasterize_glyph(font: &LoadedFont, ch: char) -> GlyphBitmap {
    let scaled = font.font.as_scaled(font.scale);
    let glyph_id = scaled.glyph_id(ch);
    let advance = scaled.h_advance(glyph_id);
    let x = ((CELL_WIDTH as f32 - advance) * 0.5).floor().max(-1.0);
    let baseline = ((CELL_HEIGHT as f32 - font.height) * 0.5 + font.ascent).round();
    let glyph = glyph_id.with_scale_and_position(font.scale, point(x, baseline));
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
            *alpha = coverage.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8;
        }
    });

    GlyphBitmap {
        left: bounds.min.x.floor() as i16,
        top: bounds.min.y.floor() as i16,
        width,
        height,
        alpha,
    }
}

fn fill_cell(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, color: [u8; 3]) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for py in 0..CELL_HEIGHT as usize {
        for px in 0..CELL_WIDTH as usize {
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
    color: [u8; 3],
    x_shift: i16,
) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for y in 0..glyph.height {
        let cell_py = glyph.top + y as i16;
        if !(0..CELL_HEIGHT as i16).contains(&cell_py) {
            continue;
        }
        for x in 0..glyph.width {
            let cell_px = glyph.left + x as i16 + x_shift;
            if !(0..CELL_WIDTH as i16).contains(&cell_px) {
                continue;
            }
            let alpha = glyph.alpha[usize::from(y) * usize::from(glyph.width) + usize::from(x)];
            if alpha == 0 {
                continue;
            }
            let index = ((origin_y + cell_py as usize) * width + origin_x + cell_px as usize) * 4;
            blend_pixel(&mut frame[index..index + 4], color, alpha);
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
    colors: ([u8; 3], [u8; 3]),
    bold: bool,
) {
    let (fg, bg) = colors;
    let glyph = bitmap_glyph(ch).unwrap_or_else(|| bitmap_glyph('?').unwrap_or([0; 8]));
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for py in 0..CELL_HEIGHT as usize {
        let glyph_row = glyph[py / 2];
        for px in 0..CELL_WIDTH as usize {
            let bit = ((glyph_row >> px) & 1) != 0
                || (bold && px > 0 && ((glyph_row >> (px - 1)) & 1) != 0);
            let color = if bit { fg } else { bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
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

fn draw_underline(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, color: [u8; 3]) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let y = origin_y + CELL_HEIGHT as usize - 2;
    for px in 0..CELL_WIDTH as usize {
        let index = (y * width + origin_x + px) * 4;
        frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
    }
}

pub(super) fn draw_box_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
) -> bool {
    let Some((left, right, up, down)) = box_segments(ch) else {
        return false;
    };
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let center_x = CELL_WIDTH as usize / 2;
    let center_y = CELL_HEIGHT as usize / 2;
    let thickness = 2;

    for py in 0..CELL_HEIGHT as usize {
        for px in 0..CELL_WIDTH as usize {
            let horizontal = py.abs_diff(center_y) < thickness
                && ((left && px <= center_x) || (right && px >= center_x));
            let vertical = px.abs_diff(center_x) < thickness
                && ((up && py <= center_y) || (down && py >= center_y));
            let color = if horizontal || vertical { fg } else { bg };
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
