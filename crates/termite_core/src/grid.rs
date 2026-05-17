use std::ops::Range;

use crate::{DamageRegion, DamageTracker, EraseMode, Generation};
use unicode_width::UnicodeWidthChar;

const SCROLLED_ROW_POOL_LIMIT: usize = 1024;
const STYLE_BOLD: u8 = 1 << 0;
const STYLE_ITALIC: u8 = 1 << 1;
const STYLE_UNDERLINE: u8 = 1 << 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    DefaultForeground,
    DefaultBackground,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub foreground: Color,
    pub background: Color,
    flags: u8,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            foreground: Color::DefaultForeground,
            background: Color::DefaultBackground,
            flags: 0,
        }
    }
}

impl Style {
    pub fn bold(self) -> bool {
        self.flags & STYLE_BOLD != 0
    }

    pub fn italic(self) -> bool {
        self.flags & STYLE_ITALIC != 0
    }

    pub fn underline(self) -> bool {
        self.flags & STYLE_UNDERLINE != 0
    }

    pub fn set_bold(&mut self, enabled: bool) {
        self.set_flag(STYLE_BOLD, enabled);
    }

    pub fn set_italic(&mut self, enabled: bool) {
        self.set_flag(STYLE_ITALIC, enabled);
    }

    pub fn set_underline(&mut self, enabled: bool) {
        self.set_flag(STYLE_UNDERLINE, enabled);
    }

    fn set_flag(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    pub fn attribute_bits(self) -> u8 {
        self.flags
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
    pub wide: bool,
    pub spacer: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
            wide: false,
            spacer: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Beam,
    Underline,
}

#[derive(Debug)]
pub struct Grid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
    row_offset: usize,
    cursor: Cursor,
    synchronized: bool,
    scroll_top: u16,
    scroll_bottom: u16,
    wrap: bool,
    pending_wrap: bool,
    scrolled_rows: Vec<Vec<Cell>>,
    scrolled_row_pool: Vec<Vec<Cell>>,
    wide_rows: Vec<bool>,
    row_lengths: Vec<u16>,
    generation: Generation,
    damage: DamageTracker,
}

impl Grid {
    pub fn new(width: u16, height: u16) -> Self {
        let len = usize::from(width) * usize::from(height);
        let mut damage = DamageTracker::default();
        damage.mark(DamageRegion::Viewport);
        Self {
            width,
            height,
            cells: vec![Cell::default(); len],
            row_offset: 0,
            cursor: Cursor {
                x: 0,
                y: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            synchronized: false,
            scroll_top: 0,
            scroll_bottom: height.saturating_sub(1),
            wrap: true,
            pending_wrap: false,
            scrolled_rows: Vec::new(),
            scrolled_row_pool: Vec::new(),
            wide_rows: vec![false; usize::from(height)],
            row_lengths: vec![0; usize::from(height)],
            generation: 1,
            damage,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn generation(&self) -> Generation {
        self.generation
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn is_synchronized(&self) -> bool {
        self.synchronized
    }

    pub fn cell(&self, x: u16, y: u16) -> Option<Cell> {
        self.index(x, y).map(|index| self.cells[index])
    }

    #[allow(dead_code)]
    pub fn visible_cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn row(&self, y: u16) -> Option<&[Cell]> {
        if y >= self.height {
            return None;
        }
        let start = self.row_start(y);
        let end = start + usize::from(self.width);
        Some(&self.cells[start..end])
    }

    pub fn put_char(&mut self, ch: char, style: Style) -> Option<(u16, u16, Cell)> {
        if ch == '\n' {
            self.line_feed();
            return None;
        }

        self.print_linewrap();

        let mut width = char_width(ch);
        if width == 0 {
            return None;
        }

        if width > 1 && self.cursor.x + width > self.width {
            if self.wrap {
                let x = self.cursor.x;
                let y = self.cursor.y;
                let cell = Cell {
                    ch: ' ',
                    style,
                    wide: false,
                    spacer: false,
                };
                let changed = self.replace_cell(x, y, cell);
                self.pending_wrap = true;
                if changed {
                    self.mark_line_damage(y, x, self.width - x);
                }
                self.mark_row_has_wide_state(y);
                self.print_linewrap();
            } else {
                width = 1;
            }
        }

        let x = self.cursor.x;
        let y = self.cursor.y;
        let (mut damage_start, mut damage_end, mut changed) = self.clear_cell_for_write(x, y);
        if width > 1 && x + 1 < self.width {
            let (start, end, range_changed) = self.clear_cell_for_write(x + 1, y);
            damage_start = damage_start.min(start);
            damage_end = damage_end.max(end);
            changed |= range_changed;
        }

        let cell = Cell {
            ch,
            style,
            wide: width > 1,
            spacer: false,
        };
        changed |= self.replace_cell(x, y, cell);
        damage_start = damage_start.min(x);
        damage_end = damage_end.max(x);

        if width > 1 && x + 1 < self.width {
            changed |= self.replace_cell(
                x + 1,
                y,
                Cell {
                    ch: ' ',
                    style,
                    wide: false,
                    spacer: true,
                },
            );
            damage_end = damage_end.max(x + 1);
        }
        if width > 1 {
            self.mark_row_has_wide_state(y);
        }
        self.update_row_length_after_write(y, x, width);

        if changed {
            self.mark_line_damage(y, damage_start, damage_end - damage_start + 1);
        }
        self.advance_cursor(width);
        Some((x, y, cell))
    }

    pub fn put_ascii_run(&mut self, bytes: &[u8], style: Style) {
        let mut offset = 0;
        while offset < bytes.len() {
            self.print_linewrap();

            let x = self.cursor.x;
            let y = self.cursor.y;
            let available = usize::from(self.width.saturating_sub(x)).max(1);
            let count = available.min(bytes.len() - offset);
            if self.row_may_have_wide_state(y) && self.row_range_has_wide_state(y, x, count as u16)
            {
                let _ = self.put_char(char::from(bytes[offset]), style);
                offset += 1;
                continue;
            }

            let Some(start) = self.index(x, y) else {
                return;
            };
            let row = &mut self.cells[start..start + count];
            for (cell, byte) in row.iter_mut().zip(&bytes[offset..offset + count]) {
                *cell = Cell {
                    ch: char::from(*byte),
                    style,
                    wide: false,
                    spacer: false,
                };
            }
            self.update_row_length_after_ascii_write(y, x, &bytes[offset..offset + count], style);
            self.mark_line_damage(y, x, count as u16);
            self.advance_cursor(count as u16);
            offset += count;
        }
    }

    pub fn put_ascii_run_crlf(&mut self, bytes: &[u8], style: Style) -> bool {
        if bytes.is_empty() || self.pending_wrap {
            return false;
        }

        let x = self.cursor.x;
        let y = self.cursor.y;
        let count = bytes.len();
        let available = usize::from(self.width.saturating_sub(x));
        if count > available {
            return false;
        }
        if self.row_may_have_wide_state(y) && self.row_range_has_wide_state(y, x, count as u16) {
            return false;
        }

        let Some(start) = self.index(x, y) else {
            return false;
        };
        for (cell, byte) in self.cells[start..start + count].iter_mut().zip(bytes) {
            *cell = Cell {
                ch: char::from(*byte),
                style,
                wide: false,
                spacer: false,
            };
        }
        self.update_row_length_after_ascii_write(y, x, bytes, style);
        self.mark_line_damage(y, x, count as u16);

        let old = self.cursor;
        self.cursor.x = 0;
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up_region();
        } else {
            self.cursor.y = (self.cursor.y + 1).min(self.height - 1);
        }
        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
        true
    }

    pub fn put_tab(&mut self, style: Style) {
        let next_stop = ((self.cursor.x / 8) + 1) * 8;
        let spaces = next_stop.saturating_sub(self.cursor.x).max(1);
        for _ in 0..spaces {
            let _ = self.put_char(' ', style);
        }
    }

    pub fn carriage_return_line_feed(&mut self) {
        let old = self.cursor;
        self.pending_wrap = false;
        self.cursor.x = 0;
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up_region();
        } else {
            self.cursor.y = (self.cursor.y + 1).min(self.height - 1);
        }
        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
    }

    pub fn move_cursor(&mut self, x: u16, y: u16) -> Option<(Cursor, Cursor)> {
        let new = Cursor {
            x: x.min(self.width.saturating_sub(1)),
            y: y.min(self.height.saturating_sub(1)),
            visible: self.cursor.visible,
            shape: self.cursor.shape,
        };
        if new == self.cursor {
            return None;
        }

        let old = self.cursor;
        self.cursor = new;
        self.pending_wrap = false;
        self.generation += 1;
        self.damage.mark(DamageRegion::Cursor {
            old: Some((old.x, old.y)),
            new: (new.x, new.y),
        });
        Some((old, new))
    }

    pub fn move_cursor_relative(&mut self, dx: i16, dy: i16) -> Option<(Cursor, Cursor)> {
        let x = clamp_add(self.cursor.x, dx, self.width);
        let y = clamp_add(self.cursor.y, dy, self.height);
        self.move_cursor(x, y)
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        if self.cursor.visible == visible {
            return;
        }
        let old = self.cursor;
        self.cursor.visible = visible;
        self.generation += 1;
        self.damage.mark(DamageRegion::Cursor {
            old: Some((old.x, old.y)),
            new: (self.cursor.x, self.cursor.y),
        });
    }

    pub fn set_cursor_shape(&mut self, shape: CursorShape) {
        if self.cursor.shape == shape {
            return;
        }
        let old = self.cursor;
        self.cursor.shape = shape;
        self.generation += 1;
        self.damage.mark(DamageRegion::Cursor {
            old: Some((old.x, old.y)),
            new: (self.cursor.x, self.cursor.y),
        });
    }

    pub fn set_synchronized(&mut self, synchronized: bool) {
        self.synchronized = synchronized;
    }

    pub fn set_wrap(&mut self, enabled: bool) {
        self.wrap = enabled;
        if !enabled {
            self.pending_wrap = false;
        }
    }

    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let top = top.min(self.height.saturating_sub(1));
        let bottom = bottom.min(self.height.saturating_sub(1));
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            let _ = self.move_cursor(0, 0);
        } else {
            self.reset_scroll_region();
        }
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.height.saturating_sub(1);
    }

    pub fn screen_alignment(&mut self, style: Style) {
        let cell = Cell {
            ch: 'E',
            style,
            wide: false,
            spacer: false,
        };
        self.cells.fill(cell);
        self.wide_rows.fill(false);
        self.row_lengths.fill(self.width);
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
    }

    pub fn clear_screen(&mut self, mode: EraseMode) {
        let mut changed = false;
        match mode {
            EraseMode::All => {
                changed = self.cells.iter().any(|cell| *cell != Cell::default());
                self.cells.fill(Cell::default());
                self.wide_rows.fill(false);
                self.row_lengths.fill(0);
            }
            EraseMode::FromCursor => {
                changed |= self.clear_cells(
                    self.index(self.cursor.x, self.cursor.y).unwrap_or(0)
                        ..self.row_start(self.cursor.y) + usize::from(self.width),
                );
                for y in self.cursor.y.saturating_add(1)..self.height {
                    let row_start = self.row_start(y);
                    changed |= self.clear_cells(row_start..row_start + usize::from(self.width));
                }
            }
            EraseMode::ToCursor => {
                for y in 0..self.cursor.y {
                    let row_start = self.row_start(y);
                    changed |= self.clear_cells(row_start..row_start + usize::from(self.width));
                }
                changed |= self.clear_cells(
                    self.row_start(self.cursor.y)
                        ..self.index(self.cursor.x, self.cursor.y).unwrap_or(0) + 1,
                );
            }
        }
        if changed {
            self.generation += 1;
            self.damage.mark(DamageRegion::Viewport);
        }
    }

    pub fn clear_line(&mut self, mode: EraseMode) {
        let row_start = self.row_start(self.cursor.y);
        let row_end = row_start + usize::from(self.width);
        let cursor = self
            .index(self.cursor.x, self.cursor.y)
            .unwrap_or(row_start);
        let (start, end, x, width) = match mode {
            EraseMode::FromCursor => (
                cursor,
                row_end,
                self.cursor.x,
                self.width.saturating_sub(self.cursor.x),
            ),
            EraseMode::ToCursor => (row_start, cursor + 1, 0, self.cursor.x.saturating_add(1)),
            EraseMode::All => (row_start, row_end, 0, self.width),
        };
        if self.clear_cells(start..end) {
            if matches!(mode, EraseMode::All) {
                let physical_row = self.physical_row(self.cursor.y);
                if let Some(wide) = self.wide_rows.get_mut(physical_row) {
                    *wide = false;
                }
            }
            self.mark_line_damage(self.cursor.y, x, width);
        }
    }

    pub fn erase_chars(&mut self, count: u16) {
        let end = self.cursor.x.saturating_add(count).min(self.width);
        self.fill_line_range(self.cursor.y, self.cursor.x, end);
    }

    pub fn delete_chars(&mut self, count: u16) {
        self.shift_chars(count, false);
    }

    pub fn insert_blank_chars(&mut self, count: u16) {
        self.shift_chars(count, true);
    }

    pub fn delete_lines(&mut self, count: u16) {
        self.shift_lines(self.cursor.y, self.scroll_bottom, count, false);
    }

    pub fn insert_blank_lines(&mut self, count: u16) {
        self.shift_lines(self.cursor.y, self.scroll_bottom, count, true);
    }

    pub fn scroll_up(&mut self, count: u16) {
        self.shift_lines(self.scroll_top, self.scroll_bottom, count, false);
    }

    pub fn scroll_down(&mut self, count: u16) {
        self.shift_lines(self.scroll_top, self.scroll_bottom, count, true);
    }

    pub fn resize(&mut self, width: u16, height: u16) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return false;
        }

        let old_height = self.height;
        let mut cells = vec![Cell::default(); usize::from(width) * usize::from(height)];
        let copy_width = self.width.min(width);
        let copy_height = old_height.min(height);
        for y in 0..copy_height {
            let new_start = usize::from(y) * usize::from(width);
            let len = usize::from(copy_width);
            if let Some(row) = self.row(y) {
                cells[new_start..new_start + len].copy_from_slice(&row[..len]);
            }
        }

        self.width = width;
        self.height = height;
        self.cells = cells;
        self.row_offset = 0;
        self.rebuild_wide_rows();
        self.rebuild_row_lengths();
        self.cursor.x = self.cursor.x.min(self.width - 1);
        self.cursor.y = self.cursor.y.min(self.height - 1);
        self.pending_wrap = false;
        self.reset_scroll_region();
        for y in 0..self.height {
            let _ = self.sanitize_row(y);
        }
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
        true
    }

    pub fn resize_reflow(&mut self, width: u16, height: u16) -> bool {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return false;
        }

        let old_width = self.width;
        let old_height = self.height;
        let old_cells = self.logical_cells();
        let old_cursor = self.cursor;
        let mut rows = Vec::new();
        let mut cursor_row = 0usize;
        let mut cursor_x = 0u16;

        let row_limit = reflow_row_limit(&old_cells, old_width, old_height, old_cursor.y);
        for y in 0..row_limit {
            let row_start = usize::from(y) * usize::from(old_width);
            let row = &old_cells[row_start..row_start + usize::from(old_width)];
            let line_start = rows.len();
            let cursor = (y == old_cursor.y).then_some(old_cursor.x);
            let line_cursor = append_reflowed_row(row, width, &mut rows, cursor);
            if let Some((line_cursor_row, line_cursor_x)) = line_cursor {
                cursor_row = line_start + line_cursor_row;
                cursor_x = line_cursor_x;
            }
        }

        if rows.is_empty() {
            rows.push(blank_row(width));
        }

        let visible_height = usize::from(height);
        let scroll_count = rows.len().saturating_sub(visible_height);
        if scroll_count > 0 {
            for row in &rows[..scroll_count] {
                let buffer = self.scrolled_row_pool.pop();
                self.scrolled_rows.push(scrollback_row(row, buffer));
            }
        }

        self.width = width;
        self.height = height;
        self.cells = rows[scroll_count..]
            .iter()
            .take(visible_height)
            .flatten()
            .copied()
            .collect();
        while self.cells.len() < usize::from(width) * visible_height {
            self.cells.extend(blank_row(width));
        }
        self.row_offset = 0;
        self.rebuild_wide_rows();
        self.rebuild_row_lengths();

        self.cursor.x = cursor_x.min(self.width - 1);
        self.cursor.y = cursor_row
            .saturating_sub(scroll_count)
            .min(visible_height.saturating_sub(1)) as u16;
        self.pending_wrap = false;
        self.reset_scroll_region();
        for y in 0..self.height {
            let _ = self.sanitize_row(y);
        }
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
        true
    }

    pub fn drain_damage(&mut self) -> crate::DamageBatch {
        self.damage.drain(self.generation)
    }

    pub fn drain_scrolled_rows(&mut self) -> Vec<Vec<Cell>> {
        let capacity = self.scrolled_rows.capacity();
        std::mem::replace(&mut self.scrolled_rows, Vec::with_capacity(capacity))
    }

    pub fn recycle_scrolled_rows(&mut self, rows: impl IntoIterator<Item = Vec<Cell>>) {
        for mut row in rows {
            if self.scrolled_row_pool.len() >= SCROLLED_ROW_POOL_LIMIT {
                return;
            }
            row.clear();
            self.scrolled_row_pool.push(row);
        }
    }

    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(self.row_start(y) + usize::from(x))
        } else {
            None
        }
    }

    fn row_start(&self, y: u16) -> usize {
        self.physical_row(y) * usize::from(self.width)
    }

    fn physical_row(&self, y: u16) -> usize {
        (self.row_offset + usize::from(y)) % usize::from(self.height.max(1))
    }

    fn logical_cells(&self) -> Vec<Cell> {
        let mut cells = Vec::with_capacity(self.cells.len());
        for y in 0..self.height {
            if let Some(row) = self.row(y) {
                cells.extend_from_slice(row);
            }
        }
        cells
    }

    fn fill_line_range(&mut self, y: u16, x_start: u16, x_end: u16) {
        if y >= self.height || x_start >= x_end {
            return;
        }
        let row_start = self.row_start(y);
        let start = row_start + usize::from(x_start);
        let end = row_start + usize::from(x_end.min(self.width));
        if self.clear_cells(start..end) {
            self.mark_line_damage(y, x_start, x_end.saturating_sub(x_start));
        }
    }

    fn clear_cells(&mut self, range: Range<usize>) -> bool {
        let range = self.expand_clear_range(range);
        let changed = self.cells[range.clone()]
            .iter()
            .any(|cell| *cell != Cell::default());
        self.cells[range.clone()].fill(Cell::default());
        if changed {
            self.refresh_row_lengths_for_range(range);
        }
        changed
    }

    fn shift_chars(&mut self, count: u16, right: bool) {
        if self.cursor.x >= self.width {
            return;
        }

        let row_start = self.row_start(self.cursor.y);
        let x = usize::from(self.cursor.x);
        let width = usize::from(self.width);
        let count = usize::from(count.min(self.width - self.cursor.x));
        if count == 0 {
            return;
        }
        let row_end = row_start + width;
        let mut changed = self.cells[row_start + x..row_end]
            .iter()
            .any(|cell| *cell != Cell::default());
        if !right
            || self
                .cell(self.cursor.x, self.cursor.y)
                .is_some_and(|cell| cell.spacer)
        {
            let (_, _, boundary_changed) = self.clear_cell_for_write(self.cursor.x, self.cursor.y);
            changed |= boundary_changed;
        }

        if right {
            self.cells
                .copy_within(row_start + x..row_end - count, row_start + x + count);
            self.cells[row_start + x..row_start + x + count].fill(Cell::default());
        } else {
            self.cells
                .copy_within(row_start + x + count..row_end, row_start + x);
            self.cells[row_end - count..row_end].fill(Cell::default());
        }

        let sanitized = self.sanitize_row(self.cursor.y);
        self.refresh_row_length(self.cursor.y);
        if changed || sanitized {
            self.mark_line_damage(self.cursor.y, self.cursor.x, self.width - self.cursor.x);
        }
    }

    fn shift_lines(&mut self, top: u16, bottom: u16, count: u16, down: bool) {
        if top < self.scroll_top || bottom > self.scroll_bottom || top > bottom {
            return;
        }

        let width = usize::from(self.width);
        let count = usize::from(count.min(bottom - top + 1));
        if count == 0 {
            return;
        }
        if !down && top == 0 && bottom == self.height.saturating_sub(1) {
            let height = usize::from(self.height);
            for row in 0..count.min(height) {
                let row_start = self.row_start(row as u16);
                let row_len = usize::from(self.row_length(row as u16));
                let buffer = self.scrolled_row_pool.pop();
                self.scrolled_rows.push(scrollback_row_trimmed(
                    &self.cells[row_start..row_start + row_len],
                    buffer,
                ));
            }
            self.row_offset = (self.row_offset + count) % height.max(1);
            for row in height.saturating_sub(count)..height {
                let row_start = self.row_start(row as u16);
                self.cells[row_start..row_start + width].fill(Cell::default());
                let physical_row = self.physical_row(row as u16);
                if let Some(wide) = self.wide_rows.get_mut(physical_row) {
                    *wide = false;
                }
                if let Some(length) = self.row_lengths.get_mut(physical_row) {
                    *length = 0;
                }
            }
            self.generation += 1;
            self.damage.mark(DamageRegion::Scroll {
                top,
                bottom,
                count: count as u16,
                down,
            });
            return;
        }

        let region_len = usize::from(bottom - top + 1);
        if down {
            for offset in (count..region_len).rev() {
                self.copy_row(top + (offset - count) as u16, top + offset as u16);
            }
            for y in top..top + count as u16 {
                self.clear_row(y);
            }
        } else {
            for offset in 0..region_len - count {
                self.copy_row(top + (offset + count) as u16, top + offset as u16);
            }
            for y in bottom - count as u16 + 1..=bottom {
                self.clear_row(y);
            }
        }

        self.generation += 1;
        self.damage.mark(DamageRegion::Scroll {
            top,
            bottom,
            count: count as u16,
            down,
        });
    }

    fn mark_line_damage(&mut self, y: u16, x: u16, width: u16) {
        self.generation = self.generation.saturating_add(u64::from(width.max(1)));
        self.damage.mark(DamageRegion::Cells {
            x,
            y,
            width,
            height: 1,
        });
    }

    fn advance_cursor(&mut self, amount: u16) {
        let old = self.cursor;
        if self.cursor.x + amount < self.width {
            self.cursor.x += amount;
            self.pending_wrap = false;
        } else if !self.wrap {
            self.cursor.x = self.width.saturating_sub(1);
            self.pending_wrap = false;
        } else {
            self.cursor.x = self.width.saturating_sub(1);
            self.pending_wrap = true;
        }

        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
    }

    fn line_feed(&mut self) {
        let old = self.cursor;
        self.pending_wrap = false;
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up_region();
        } else {
            self.cursor.y = (self.cursor.y + 1).min(self.height - 1);
        }
        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
    }

    pub fn reverse_index(&mut self) {
        let old = self.cursor;
        self.pending_wrap = false;
        if self.cursor.y == self.scroll_top {
            self.scroll_down_region();
        } else {
            self.cursor.y = self.cursor.y.saturating_sub(1);
        }
        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
    }

    fn scroll_up_region(&mut self) {
        self.scroll_up(1);
    }

    fn scroll_down_region(&mut self) {
        self.scroll_down(1);
    }

    fn print_linewrap(&mut self) {
        if !self.pending_wrap {
            return;
        }

        self.pending_wrap = false;
        let old = self.cursor;
        self.cursor.x = 0;
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up_region();
        } else {
            self.cursor.y = (self.cursor.y + 1).min(self.height - 1);
        }
        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
    }

    fn copy_row(&mut self, src_y: u16, dst_y: u16) {
        if src_y == dst_y {
            return;
        }

        let width = usize::from(self.width);
        let src_start = self.row_start(src_y);
        let dst_start = self.row_start(dst_y);
        self.cells
            .copy_within(src_start..src_start + width, dst_start);

        let src_row = self.physical_row(src_y);
        let dst_row = self.physical_row(dst_y);
        let has_wide = self.wide_rows.get(src_row).copied().unwrap_or(false);
        if let Some(wide) = self.wide_rows.get_mut(dst_row) {
            *wide = has_wide;
        }
        let row_length = self.row_lengths.get(src_row).copied().unwrap_or(0);
        if let Some(length) = self.row_lengths.get_mut(dst_row) {
            *length = row_length;
        }
    }

    fn clear_row(&mut self, y: u16) {
        let width = usize::from(self.width);
        let row_start = self.row_start(y);
        self.cells[row_start..row_start + width].fill(Cell::default());

        let physical_row = self.physical_row(y);
        if let Some(wide) = self.wide_rows.get_mut(physical_row) {
            *wide = false;
        }
        if let Some(length) = self.row_lengths.get_mut(physical_row) {
            *length = 0;
        }
    }

    fn replace_cell(&mut self, x: u16, y: u16, cell: Cell) -> bool {
        let Some(index) = self.index(x, y) else {
            return false;
        };
        if self.cells[index] == cell {
            return false;
        }
        self.cells[index] = cell;
        true
    }

    fn clear_cell_for_write(&mut self, x: u16, y: u16) -> (u16, u16, bool) {
        let mut start = x;
        let mut end = x;
        let mut changed = false;

        if self.cell(x, y).is_some_and(|cell| cell.spacer) && x > 0 {
            changed |= self.replace_cell(x - 1, y, Cell::default());
            changed |= self.replace_cell(x, y, Cell::default());
            start = x - 1;
        }

        if x + 1 < self.width && self.cell(x + 1, y).is_some_and(|cell| cell.spacer) {
            changed |= self.replace_cell(x + 1, y, Cell::default());
            end = x + 1;
        }
        if changed {
            self.refresh_row_length(y);
        }

        (start, end, changed)
    }

    fn row_range_has_wide_state(&self, y: u16, x: u16, width: u16) -> bool {
        if y >= self.height || width == 0 {
            return false;
        }
        let row_start = self.row_start(y);
        let start = usize::from(x);
        let end = start
            .saturating_add(usize::from(width))
            .min(usize::from(self.width));
        if start > 0 && self.cells[row_start + start].spacer {
            return true;
        }
        self.cells[row_start + start..row_start + end]
            .iter()
            .any(|cell| cell.wide || cell.spacer)
            || (end < usize::from(self.width) && self.cells[row_start + end].spacer)
    }

    fn row_may_have_wide_state(&self, y: u16) -> bool {
        self.wide_rows
            .get(self.physical_row(y))
            .copied()
            .unwrap_or(true)
    }

    fn mark_row_has_wide_state(&mut self, y: u16) {
        let row = self.physical_row(y);
        if let Some(wide) = self.wide_rows.get_mut(row) {
            *wide = true;
        }
    }

    fn refresh_wide_row(&mut self, y: u16) {
        let row = self.physical_row(y);
        let row_start = row * usize::from(self.width);
        let has_wide =
            row_contains_wide_state(&self.cells[row_start..row_start + usize::from(self.width)]);
        if let Some(wide) = self.wide_rows.get_mut(row) {
            *wide = has_wide;
        }
    }

    fn rebuild_wide_rows(&mut self) {
        self.wide_rows = vec![false; usize::from(self.height)];
        for y in 0..self.height {
            self.refresh_wide_row(y);
        }
    }

    fn row_length(&self, y: u16) -> u16 {
        self.row_lengths
            .get(self.physical_row(y))
            .copied()
            .unwrap_or(self.width)
    }

    fn mark_row_used_through(&mut self, y: u16, end: u16) {
        let row = self.physical_row(y);
        if let Some(length) = self.row_lengths.get_mut(row) {
            *length = (*length).max(end.min(self.width));
        }
    }

    fn update_row_length_after_write(&mut self, y: u16, x: u16, width: u16) {
        let end = x.saturating_add(width).min(self.width);
        let row = self.physical_row(y);
        let current = self.row_lengths.get(row).copied().unwrap_or(0);
        let Some(index) = self.index(x, y) else {
            return;
        };
        if self.cells[index] != Cell::default() || width > 1 {
            self.mark_row_used_through(y, end);
        } else if x < current && end >= current {
            self.refresh_row_length(y);
        }
    }

    fn update_row_length_after_ascii_write(&mut self, y: u16, x: u16, bytes: &[u8], style: Style) {
        let row = self.physical_row(y);
        let current = self.row_lengths.get(row).copied().unwrap_or(0);
        let used_end = ascii_run_used_end(x, bytes, style);
        if let Some(end) = used_end {
            self.mark_row_used_through(y, end);
        }
        let written_end = x.saturating_add(bytes.len() as u16).min(self.width);
        if x < current && written_end >= current && used_end.is_none_or(|end| end < current) {
            self.refresh_row_length(y);
        }
    }

    fn refresh_row_length(&mut self, y: u16) {
        let row = self.physical_row(y);
        self.refresh_physical_row_length(row);
    }

    fn refresh_physical_row_length(&mut self, row: usize) {
        let width = usize::from(self.width);
        let row_start = row * width;
        let end = logical_row_end(&self.cells[row_start..row_start + width]) as u16;
        if let Some(length) = self.row_lengths.get_mut(row) {
            *length = end;
        }
    }

    fn refresh_row_lengths_for_range(&mut self, range: Range<usize>) {
        if range.is_empty() || self.width == 0 {
            return;
        }
        let width = usize::from(self.width);
        let start_row = range.start / width;
        let end_row = (range.end.saturating_sub(1) / width).min(usize::from(self.height) - 1);
        for row in start_row..=end_row {
            self.refresh_physical_row_length(row);
        }
    }

    fn rebuild_row_lengths(&mut self) {
        self.row_lengths = vec![0; usize::from(self.height)];
        for row in 0..usize::from(self.height) {
            self.refresh_physical_row_length(row);
        }
    }

    fn expand_clear_range(&self, range: Range<usize>) -> Range<usize> {
        if range.is_empty() || self.width == 0 {
            return range;
        }

        let width = usize::from(self.width);
        let mut start = range.start.min(self.cells.len());
        let mut end = range.end.min(self.cells.len());

        if start < self.cells.len() && !start.is_multiple_of(width) && self.cells[start].spacer {
            start -= 1;
        }
        if end < self.cells.len() && !end.is_multiple_of(width) && self.cells[end].spacer {
            end += 1;
        }

        start..end
    }

    fn sanitize_row(&mut self, y: u16) -> bool {
        if y >= self.height {
            return false;
        }

        let row_start = self.row_start(y);
        let mut changed = false;
        for x in 0..usize::from(self.width) {
            let index = row_start + x;
            if !self.cells[index].spacer {
                continue;
            }

            let has_lead = x > 0 && self.cells[index - 1].wide;
            if !has_lead {
                self.cells[index] = Cell::default();
                changed = true;
            }
        }
        for x in 0..usize::from(self.width) {
            let index = row_start + x;
            if !self.cells[index].wide {
                continue;
            }

            let has_spacer = x + 1 < usize::from(self.width) && self.cells[index + 1].spacer;
            if !has_spacer {
                self.cells[index].wide = false;
                changed = true;
            }
        }
        if changed {
            self.refresh_row_length(y);
        }
        changed
    }
}

fn clamp_add(value: u16, delta: i16, limit: u16) -> u16 {
    let max = limit.saturating_sub(1);
    if delta < 0 {
        value.saturating_sub(delta.unsigned_abs()).min(max)
    } else {
        value.saturating_add(delta as u16).min(max)
    }
}

fn char_width(ch: char) -> u16 {
    UnicodeWidthChar::width(ch).unwrap_or(0).min(2) as u16
}

fn blank_row(width: u16) -> Vec<Cell> {
    vec![Cell::default(); usize::from(width)]
}

fn scrollback_row(row: &[Cell], buffer: Option<Vec<Cell>>) -> Vec<Cell> {
    scrollback_row_trimmed(&row[..logical_row_end(row)], buffer)
}

fn scrollback_row_trimmed(row: &[Cell], buffer: Option<Vec<Cell>>) -> Vec<Cell> {
    let mut buffer = buffer.unwrap_or_default();
    buffer.clear();
    buffer.extend_from_slice(row);
    buffer
}

fn ascii_run_used_end(x: u16, bytes: &[u8], style: Style) -> Option<u16> {
    let index = if style == Style::default() {
        bytes.iter().rposition(|byte| *byte != b' ')?
    } else {
        bytes.len().checked_sub(1)?
    };
    Some(x.saturating_add(index as u16).saturating_add(1))
}

fn row_contains_wide_state(row: &[Cell]) -> bool {
    row.iter().any(|cell| cell.wide || cell.spacer)
}

fn append_reflowed_row(
    row: &[Cell],
    width: u16,
    rows: &mut Vec<Vec<Cell>>,
    cursor_x: Option<u16>,
) -> Option<(usize, u16)> {
    let width_usize = usize::from(width);
    let end = logical_row_end(row);
    let cursor = cursor_x.map(|x| cursor_position_for_row(row, end, x, width));
    let mut current = blank_row(width);
    let mut x = 0usize;

    if end == 0 {
        rows.push(current);
        return cursor.or(Some((0, 0))).filter(|_| cursor_x.is_some());
    }

    for cell in row.iter().take(end).copied() {
        if cell.spacer {
            continue;
        }
        let cell_width = reflow_cell_width(cell, width);
        if x + usize::from(cell_width) > width_usize && x > 0 {
            rows.push(current);
            current = blank_row(width);
            x = 0;
        }

        current[x] = Cell {
            wide: cell_width > 1,
            spacer: false,
            ..cell
        };
        if cell_width > 1 && x + 1 < width_usize {
            current[x + 1] = Cell {
                ch: ' ',
                style: cell.style,
                wide: false,
                spacer: true,
            };
        }
        x += usize::from(cell_width);
    }
    rows.push(current);
    cursor
}

fn logical_row_end(row: &[Cell]) -> usize {
    row.iter()
        .rposition(|cell| *cell != Cell::default())
        .map_or(0, |index| index + 1)
}

fn reflow_row_limit(cells: &[Cell], width: u16, height: u16, cursor_y: u16) -> u16 {
    let width = usize::from(width);
    let last_nonblank = (0..height).rev().find(|y| {
        let start = usize::from(*y) * width;
        logical_row_end(&cells[start..start + width]) > 0
    });
    last_nonblank
        .unwrap_or(0)
        .max(cursor_y)
        .saturating_add(1)
        .min(height)
}

fn cursor_position_for_row(row: &[Cell], end: usize, cursor_x: u16, width: u16) -> (usize, u16) {
    let columns = row
        .iter()
        .take(end.min(usize::from(cursor_x)))
        .filter(|cell| !cell.spacer)
        .map(|cell| usize::from(reflow_cell_width(*cell, width)))
        .sum::<usize>();
    if columns == 0 {
        return (0, 0);
    }

    let width = usize::from(width);
    if columns.is_multiple_of(width) {
        (columns / width - 1, (width - 1) as u16)
    } else {
        (columns / width, (columns % width) as u16)
    }
}

fn reflow_cell_width(cell: Cell, grid_width: u16) -> u16 {
    if grid_width > 1 {
        char_width(cell.ch).max(1)
    } else {
        1
    }
}
