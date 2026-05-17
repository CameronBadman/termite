use ab_glyph::{Font, FontArc};
use rustybuzz::{Face, UnicodeBuffer};
use termite_core::{Cell, Style};

use super::{
    font::LoadedFont,
    geometry::{is_private_use_symbol, is_special_cell},
    glyph::ShapedGlyphKey,
};

#[derive(Clone, Copy)]
pub(super) struct ShapedGlyph {
    pub(super) key: ShapedGlyphKey,
    pub(super) x: f32,
    pub(super) synthetic_bold: bool,
}

#[derive(Clone, Copy)]
pub(super) struct TextRun {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) end: u16,
    pub(super) first: Cell,
}

#[inline]
pub(super) fn is_shapable_cell(cell: Cell) -> bool {
    !cell.spacer
        && !cell.wide
        && cell.ch != ' '
        && !is_private_use_symbol(cell.ch)
        && !is_special_cell(cell.ch)
}

pub(super) fn shape_text(
    font: usize,
    fonts: &[LoadedFont],
    cell_width: u32,
    text: &str,
    columns: u16,
    style: Style,
) -> Option<Vec<ShapedGlyph>> {
    let face = Face::from_slice(&fonts[font].bytes, 0)?;
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    let output = rustybuzz::shape(&face, &[], buffer);
    let infos = output.glyph_infos();
    let positions = output.glyph_positions();
    if infos.is_empty() || !shaping_changed(&fonts[font].font, text, infos) {
        return None;
    }

    let units_per_em = face.units_per_em() as f32;
    let scale = fonts[font].scale.x / units_per_em.max(1.0);
    let max_x = cell_width as f32 * f32::from(columns.max(1));
    let synthetic_bold = style.bold() && !fonts[font].role.is_bold();
    let mut x = 0.0;
    let mut glyphs = Vec::with_capacity(infos.len());
    for (info, position) in infos.iter().zip(positions) {
        let glyph_id = u16::try_from(info.glyph_id).ok()?;
        let glyph_x = x + position.x_offset as f32 * scale;
        if glyph_x < max_x {
            glyphs.push(ShapedGlyph {
                key: ShapedGlyphKey { font, glyph_id },
                x: glyph_x,
                synthetic_bold,
            });
        }
        x += position.x_advance as f32 * scale;
    }
    Some(glyphs)
}

fn shaping_changed(font: &FontArc, text: &str, infos: &[rustybuzz::GlyphInfo]) -> bool {
    let chars = text.chars().collect::<Vec<_>>();
    if infos.len() != chars.len() {
        return true;
    }
    infos
        .iter()
        .zip(chars)
        .any(|(info, ch)| u32::from(font.glyph_id(ch).0) != info.glyph_id)
}

#[cfg(test)]
pub(super) fn may_need_shaping(text: &str) -> bool {
    let mut previous = '\0';
    for ch in text.chars() {
        if char_may_need_shaping(ch, previous) {
            return true;
        }
        previous = ch;
    }
    false
}

#[inline]
pub(super) fn char_may_need_shaping(ch: char, previous: char) -> bool {
    !ch.is_ascii()
        || matches!(
            (previous, ch),
            ('=', '>')
                | ('=', '=')
                | ('=', '<')
                | ('<', '=')
                | ('>', '=')
                | ('!', '=')
                | ('-', '>')
                | ('<', '-')
                | ('|', '>')
                | ('<', '|')
                | (':', ':')
                | ('.', '.')
                | ('f', 'i')
                | ('f', 'l')
                | ('f', 'f')
        )
}
