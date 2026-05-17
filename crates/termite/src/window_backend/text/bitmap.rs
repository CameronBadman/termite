use font8x8::{
    BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, HIRAGANA_FONTS, LATIN_FONTS, MISC_FONTS,
    SGA_FONTS, UnicodeFonts,
};

use super::paint::CellPaint;

#[inline]
pub(super) fn draw_bitmap_cell(
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
            if !bit && !paint.background_opaque {
                continue;
            }
            let color = if bit { paint.fg } else { paint.bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

#[inline]
pub(in crate::window_backend) fn sample_bitmap_axis(pixel: usize, cell_size: usize) -> usize {
    ((pixel * 8 + cell_size / 2) / cell_size).min(7)
}

#[inline]
pub(in crate::window_backend) fn ascii_glyph_fallback(ch: char) -> Option<char> {
    match ch {
        '‘' | '’' | '‚' | '‛' => Some('\''),
        '“' | '”' | '„' | '‟' => Some('"'),
        '‐' | '‑' | '‒' | '–' | '—' | '―' => Some('-'),
        '…' => Some('.'),
        _ => None,
    }
}

#[inline]
pub(in crate::window_backend) fn bitmap_glyph(ch: char) -> Option<[u8; 8]> {
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
