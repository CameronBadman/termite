use std::{collections::HashMap, fs};

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use c_term_core::{Cell, Color, Style};
use font8x8::{
    BASIC_FONTS, BLOCK_FONTS, BOX_FONTS, GREEK_FONTS, HIRAGANA_FONTS, LATIN_FONTS, MISC_FONTS,
    SGA_FONTS, UnicodeFonts,
};
use rustybuzz::{Face, UnicodeBuffer};

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ShapedGlyphKey {
    font: usize,
    glyph_id: u16,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FontLookupKey {
    ch: char,
    bold: bool,
    italic: bool,
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
    bytes: Vec<u8>,
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
    ascii_font_lookup: [[Option<Option<usize>>; 128]; 4],
    extended_font_lookup: HashMap<FontLookupKey, Option<usize>>,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
    shaped_glyphs: HashMap<ShapedGlyphKey, GlyphBitmap>,
    metrics: TerminalMetrics,
    text_render: TextRenderConfig,
    theme: Theme,
}

#[derive(Clone, Copy)]
pub(super) struct CellPaint {
    pub(super) fg: [u8; 3],
    pub(super) bg: [u8; 3],
    pub(super) background_opaque: bool,
    pub(super) metrics: TerminalMetrics,
}

#[derive(Clone, Copy)]
struct GlyphPaint {
    color: [u8; 3],
    x_shift: f32,
    origin_metrics: TerminalMetrics,
    glyph_metrics: TerminalMetrics,
}

#[derive(Clone, Copy)]
struct ShapedGlyph {
    key: ShapedGlyphKey,
    x: f32,
    synthetic_bold: bool,
}

#[derive(Clone, Copy)]
struct TextRun {
    x: u16,
    y: u16,
    end: u16,
    first: Cell,
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
            if cell.ch == ' ' && !cell.style.underline {
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
        let Some(glyphs) = self.shape_text(font, &text, run.end - run.x, run.first.style) else {
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

        if run.first.style.underline {
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
                draw_bitmap_cell(frame, width, cell_x, cell_y, ch, style.bold, paint);
            }
        }
        if style.underline {
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
            bold: style.bold,
            italic: style.italic,
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

    fn shape_text(
        &self,
        font: usize,
        text: &str,
        columns: u16,
        style: Style,
    ) -> Option<Vec<ShapedGlyph>> {
        let face = Face::from_slice(&self.fonts[font].bytes, 0)?;
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        let output = rustybuzz::shape(&face, &[], buffer);
        let infos = output.glyph_infos();
        let positions = output.glyph_positions();
        if infos.is_empty() || !shaping_changed(&self.fonts[font].font, text, infos) {
            return None;
        }

        let units_per_em = face.units_per_em() as f32;
        let scale = self.fonts[font].scale.x / units_per_em.max(1.0);
        let max_x = self.metrics.cell_width as f32 * f32::from(columns.max(1));
        let synthetic_bold = style.bold && !self.fonts[font].role.is_bold();
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

fn is_shapable_cell(cell: Cell) -> bool {
    !cell.spacer
        && !cell.wide
        && cell.ch != ' '
        && !is_private_use_symbol(cell.ch)
        && !is_special_cell(cell.ch)
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
fn may_need_shaping(text: &str) -> bool {
    let mut previous = '\0';
    for ch in text.chars() {
        if char_may_need_shaping(ch, previous) {
            return true;
        }
        previous = ch;
    }
    false
}

fn char_may_need_shaping(ch: char, previous: char) -> bool {
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
        if let Ok(bytes) = fs::read(&path)
            && let Ok(font) = FontArc::try_from_vec(bytes.clone())
        {
            loaded.push(LoadedFont {
                bytes,
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

fn font_style_index(style: Style) -> usize {
    usize::from(style.bold) | (usize::from(style.italic) << 1)
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
    rasterize_positioned_glyph(font, scale, glyph_id, advance, metrics, render, true)
}

fn rasterize_glyph_id(
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

fn is_special_cell(ch: char) -> bool {
    matches!(
        ch,
        '█' | '▀'
            | '▄'
            | '▌'
            | '▐'
            | '▘'
            | '▝'
            | '▖'
            | '▗'
            | '▚'
            | '▞'
            | '▙'
            | '▛'
            | '▜'
            | '▟'
            | '░'
            | '▒'
            | '▓'
    ) || box_segments(ch).is_some()
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

fn fill_row(frame: &mut [u8], width: usize, y: u16, color: [u8; 3], metrics: TerminalMetrics) {
    let row_start = usize::from(y) * metrics.cell_height as usize * width * 4;
    let row_len = metrics.cell_height as usize * width * 4;
    if let Some(row) = frame.get_mut(row_start..row_start + row_len) {
        for pixel in row.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[color[0], color[1], color[2], 0xff]);
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
            if !bit && !paint.background_opaque {
                continue;
            }
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
    draw_block_cell_inner(frame, width, cell_x, cell_y, ch, paint, false)
        || draw_shade_cell_inner(frame, width, cell_x, cell_y, ch, paint, false)
        || draw_box_cell_inner(frame, width, cell_x, cell_y, ch, paint, false)
}

#[cfg(test)]
pub(super) fn draw_block_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_block_cell_inner(frame, width, cell_x, cell_y, ch, paint, true)
}

fn draw_block_cell_inner(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
    fill_background: bool,
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

    if fill_background {
        fill_cell(frame, width, cell_x, cell_y, paint.bg, metrics);
    }
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

#[cfg(test)]
pub(super) fn draw_shade_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_shade_cell_inner(frame, width, cell_x, cell_y, ch, paint, true)
}

fn draw_shade_cell_inner(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
    fill_background: bool,
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
            if pattern >= threshold && !fill_background {
                continue;
            }
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

#[cfg(test)]
pub(super) fn draw_box_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
) -> bool {
    draw_box_cell_inner(frame, width, cell_x, cell_y, ch, paint, true)
}

fn draw_box_cell_inner(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    paint: CellPaint,
    fill_background: bool,
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
            if !(horizontal || vertical || fill_background) {
                continue;
            }
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
