use std::collections::HashMap;

use ab_glyph::{Font, GlyphId};
use termite_core::{Cell, Color, Style};

use crate::{
    runner::{FontConfig, TerminalMetrics, TextRenderConfig},
    theme::Theme,
};

mod bitmap;
mod font;
mod geometry;
mod glyph;
mod paint;
mod shape;

pub(super) use bitmap::ascii_glyph_fallback;
#[cfg(test)]
pub(super) use bitmap::{bitmap_glyph, sample_bitmap_axis};
#[cfg(test)]
pub(super) use geometry::box_segments;
#[cfg(test)]
pub(super) use geometry::{draw_block_cell, draw_box_cell, draw_shade_cell};
pub(super) use paint::CellPaint;

use bitmap::draw_bitmap_cell;
use font::{FontRole, LoadedFont, font_style_index, load_fonts, preferred_font_roles};
use geometry::{draw_special_cell, is_private_use_symbol};
use glyph::{
    GlyphBitmap, GlyphKey, ShapedGlyphKey, glyph_metrics, rasterize_glyph, rasterize_glyph_id,
};
use paint::{GlyphPaint, draw_glyph_bitmap, draw_underline_span, fill_cell_span, fill_row};
#[cfg(test)]
use shape::may_need_shaping;
use shape::{TextRun, char_may_need_shaping, is_shapable_cell, shape_text};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FontLookupKey {
    ch: char,
    bold: bool,
    italic: bool,
}

pub(super) struct TextRenderer {
    fonts: Vec<LoadedFont>,
    ascii_font_lookup: [[Option<Option<usize>>; 128]; 4],
    extended_font_lookup: HashMap<FontLookupKey, Option<usize>>,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
    shaped_glyphs: HashMap<ShapedGlyphKey, GlyphBitmap>,
    metrics: TerminalMetrics,
    text_render: TextRenderConfig,
    theme: Theme,
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
            ascii_font_lookup: [[None; 128]; 4],
            extended_font_lookup: HashMap::new(),
            glyphs: HashMap::new(),
            shaped_glyphs: HashMap::new(),
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
        self.fill_row_backgrounds(row, frame, width, y, cols);
        self.draw_row_foregrounds(row, frame, width, y, cols);
    }

    fn fill_row_backgrounds(
        &self,
        row: &[Cell],
        frame: &mut [u8],
        width: usize,
        y: u16,
        cols: u16,
    ) {
        if cols == 0 {
            return;
        }

        let mut x = 0;
        while x < cols {
            let cell = row.get(usize::from(x)).copied().unwrap_or_default();
            let background = cell.style.background;
            let mut end = x + 1;
            while end < cols
                && row
                    .get(usize::from(end))
                    .copied()
                    .unwrap_or_default()
                    .style
                    .background
                    == background
            {
                end += 1;
            }
            if matches!(background, Color::DefaultBackground) {
                x = end;
                continue;
            }
            let bg = self.theme.color(background);
            if x == 0 && end == cols {
                fill_row(frame, width, y, bg, self.metrics);
            } else {
                fill_cell_span(frame, width, x, y, end - x, bg, self.metrics);
            }
            x = end;
        }
    }

    fn draw_row_foregrounds(
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
                x += 1;
                continue;
            }
            if cell.ch == ' ' && !cell.style.underline() {
                x += 1;
                continue;
            }

            if is_shapable_cell(cell) && !cell.ch.is_ascii() {
                x = self.draw_text_run_foregrounds(row, frame, width, x, y, cols);
                continue;
            }

            let base_columns = if cell.wide && x + 1 < cols { 2 } else { 1 };
            let columns = self.display_columns(row, x, cols, cell, base_columns);
            self.draw_cell_foreground(frame, width, x, y, cell, columns);
            x += columns;
        }
    }

    fn draw_text_run_foregrounds(
        &mut self,
        row: &[Cell],
        frame: &mut [u8],
        width: usize,
        x: u16,
        y: u16,
        cols: u16,
    ) -> u16 {
        let first = row.get(usize::from(x)).copied().unwrap_or_default();
        let mut end = x + 1;
        let mut previous = first.ch;
        let mut needs_shaping = char_may_need_shaping(first.ch, '\0');
        while end < cols {
            let cell = row.get(usize::from(end)).copied().unwrap_or_default();
            if cell.style != first.style || !is_shapable_cell(cell) {
                break;
            }
            needs_shaping |= char_may_need_shaping(cell.ch, previous);
            previous = cell.ch;
            end += 1;
        }

        if needs_shaping
            && end - x >= 2
            && self.draw_shaped_text_run(row, frame, width, TextRun { x, y, end, first })
        {
            return end;
        }

        for cell_x in x..end {
            let cell = row.get(usize::from(cell_x)).copied().unwrap_or_default();
            self.draw_cell_foreground(frame, width, cell_x, y, cell, 1);
        }
        end
    }

    fn draw_shaped_text_run(
        &mut self,
        row: &[Cell],
        frame: &mut [u8],
        width: usize,
        run: TextRun,
    ) -> bool {
        let text = row[usize::from(run.x)..usize::from(run.end)]
            .iter()
            .map(|cell| cell.ch)
            .collect::<String>();
        let Some(font) = self.shaping_font_index(&text, run.first.style) else {
            return false;
        };
        let Some(glyphs) = shape_text(
            font,
            &self.fonts,
            self.metrics.cell_width,
            &text,
            run.end - run.x,
            run.first.style,
        ) else {
            return false;
        };
        if glyphs.is_empty() {
            return false;
        }

        let fg = self.theme.color(run.first.style.foreground);
        let glyph_metrics = glyph_metrics(self.metrics, run.end - run.x);
        for glyph in glyphs {
            self.ensure_shaped_glyph(glyph.key, self.metrics);
            if let Some(bitmap) = self.shaped_glyphs.get(&glyph.key) {
                draw_glyph_bitmap(
                    frame,
                    width,
                    run.x,
                    run.y,
                    bitmap,
                    GlyphPaint {
                        color: fg,
                        x_shift: glyph.x,
                        origin_metrics: self.metrics,
                        glyph_metrics,
                    },
                );
                if glyph.synthetic_bold {
                    draw_glyph_bitmap(
                        frame,
                        width,
                        run.x,
                        run.y,
                        bitmap,
                        GlyphPaint {
                            color: fg,
                            x_shift: glyph.x + 1.0,
                            origin_metrics: self.metrics,
                            glyph_metrics,
                        },
                    );
                }
            }
        }

        if run.first.style.underline() {
            draw_underline_span(
                frame,
                width,
                run.x,
                run.y,
                run.end - run.x,
                fg,
                self.metrics,
            );
        }
        true
    }

    fn display_columns(
        &mut self,
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

    fn draw_cell_foreground(
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
            background_opaque: !matches!(style.background, Color::DefaultBackground),
            metrics: self.metrics,
        };
        if columns == 1 && draw_special_cell(frame, width, cell_x, cell_y, ch, paint) {
            return;
        }

        if ch != ' ' {
            if let Some(key) = self.glyph_key(ch, style, columns) {
                let synthetic_bold = style.bold() && !self.fonts[key.font].role.is_bold();
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
                            x_shift: 0.0,
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
                                x_shift: 1.0,
                                origin_metrics: self.metrics,
                                glyph_metrics,
                            },
                        );
                    }
                }
            } else {
                draw_bitmap_cell(frame, width, cell_x, cell_y, ch, style.bold(), paint);
            }
        }
        if style.underline() {
            draw_underline_span(frame, width, cell_x, cell_y, columns, fg, self.metrics);
        }
    }

    fn glyph_key(&mut self, ch: char, style: Style, columns: u16) -> Option<GlyphKey> {
        let columns = columns.min(u16::from(u8::MAX)) as u8;
        self.font_index_for_char(ch, style)
            .map(|font| GlyphKey { font, ch, columns })
            .or_else(|| {
                ascii_glyph_fallback(ch).and_then(|ch| {
                    self.font_index_for_char(ch, style)
                        .map(|font| GlyphKey { font, ch, columns })
                })
            })
            .or_else(|| {
                self.font_index_for_char('?', style).map(|font| GlyphKey {
                    font,
                    ch: '?',
                    columns,
                })
            })
    }

    fn font_index_for_char(&mut self, ch: char, style: Style) -> Option<usize> {
        if ch.is_ascii() {
            let style_index = font_style_index(style);
            let ch_index = ch as usize;
            if let Some(index) = self.ascii_font_lookup[style_index][ch_index] {
                return index;
            }

            let index = self.find_font_index(ch, preferred_font_roles(style));
            self.ascii_font_lookup[style_index][ch_index] = Some(index);
            return index;
        }

        let key = FontLookupKey {
            ch,
            bold: style.bold(),
            italic: style.italic(),
        };
        if let Some(index) = self.extended_font_lookup.get(&key) {
            return *index;
        }

        let index = self.find_font_index(ch, preferred_font_roles(style));
        self.extended_font_lookup.insert(key, index);
        index
    }

    fn find_font_index(&self, ch: char, roles: &[FontRole]) -> Option<usize> {
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

    fn shaping_font_index(&self, text: &str, style: Style) -> Option<usize> {
        let roles = preferred_font_roles(style);
        for role in roles {
            if let Some(index) = self.fonts.iter().position(|font| {
                font.role == *role
                    && !font.symbol
                    && text.chars().all(|ch| font.font.glyph_id(ch) != GlyphId(0))
            }) {
                return Some(index);
            }
        }
        self.fonts.iter().position(|font| {
            !font.symbol && text.chars().all(|ch| font.font.glyph_id(ch) != GlyphId(0))
        })
    }

    fn ensure_glyph(&mut self, key: GlyphKey, metrics: TerminalMetrics) {
        if self.glyphs.contains_key(&key) {
            return;
        }
        let glyph = rasterize_glyph(&self.fonts[key.font], key.ch, metrics, self.text_render);
        self.glyphs.insert(key, glyph);
    }

    fn ensure_shaped_glyph(&mut self, key: ShapedGlyphKey, metrics: TerminalMetrics) {
        if self.shaped_glyphs.contains_key(&key) {
            return;
        }
        let glyph = rasterize_glyph_id(
            &self.fonts[key.font],
            GlyphId(key.glyph_id),
            metrics,
            self.text_render,
        );
        self.shaped_glyphs.insert(key, glyph);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_bitmap_is_clipped_to_its_cell_span() {
        let metrics = TerminalMetrics {
            cell_width: 3,
            cell_height: 1,
        };
        let mut frame = vec![0; 6 * 4];
        let glyph = GlyphBitmap {
            left: 2,
            top: 0,
            width: 3,
            height: 1,
            alpha: vec![255; 3],
        };

        draw_glyph_bitmap(
            &mut frame,
            6,
            0,
            0,
            &glyph,
            GlyphPaint {
                color: [255, 255, 255],
                x_shift: 0.0,
                origin_metrics: metrics,
                glyph_metrics: metrics,
            },
        );

        assert_eq!(&frame[2 * 4..2 * 4 + 3], &[255, 255, 255]);
        assert!(
            frame[3 * 4..]
                .chunks_exact(4)
                .all(|pixel| pixel[..3] == [0, 0, 0])
        );
    }

    #[test]
    fn shaping_guard_skips_plain_words_and_accepts_ligature_candidates() {
        assert!(!may_need_shaping("plainWord123"));
        assert!(!may_need_shaping("a=b.c-d/e"));
        assert!(may_need_shaping("=>"));
        assert!(may_need_shaping("office"));
        assert!(may_need_shaping("λx"));
    }

    #[test]
    fn shapable_cells_exclude_terminal_geometry_and_wide_cells() {
        assert!(is_shapable_cell(Cell {
            ch: 'a',
            ..Cell::default()
        }));
        assert!(!is_shapable_cell(Cell {
            ch: '─',
            ..Cell::default()
        }));
        assert!(!is_shapable_cell(Cell {
            ch: '表',
            wide: true,
            ..Cell::default()
        }));
    }
}
