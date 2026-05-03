use std::ops::Range;

use crate::{DamageRegion, DamageTracker, EraseMode, Generation};
use unicode_width::UnicodeWidthChar;

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
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            foreground: Color::DefaultForeground,
            background: Color::DefaultBackground,
            bold: false,
            italic: false,
            underline: false,
        }
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
    cursor: Cursor,
    synchronized: bool,
    scroll_top: u16,
    scroll_bottom: u16,
    wrap: bool,
    pending_wrap: bool,
    scrolled_rows: Vec<Vec<Cell>>,
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

    pub fn visible_cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn row(&self, y: u16) -> Option<&[Cell]> {
        if y >= self.height {
            return None;
        }
        let start = usize::from(y) * usize::from(self.width);
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
                    spacer: true,
                };
                let changed = self.replace_cell(x, y, cell);
                self.pending_wrap = true;
                if changed {
                    self.mark_line_damage(y, x, self.width - x);
                }
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

        if changed {
            self.mark_line_damage(y, damage_start, damage_end - damage_start + 1);
        }
        self.advance_cursor(width);
        Some((x, y, cell))
    }

    pub fn put_tab(&mut self, style: Style) {
        let next_stop = ((self.cursor.x / 8) + 1) * 8;
        let spaces = next_stop.saturating_sub(self.cursor.x).max(1);
        for _ in 0..spaces {
            let _ = self.put_char(' ', style);
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

    pub fn clear_screen(&mut self, mode: EraseMode) {
        let range = match mode {
            EraseMode::All => 0..self.cells.len(),
            EraseMode::FromCursor => {
                self.index(self.cursor.x, self.cursor.y).unwrap_or(0)..self.cells.len()
            }
            EraseMode::ToCursor => {
                let end = self.index(self.cursor.x, self.cursor.y).unwrap_or(0) + 1;
                0..end
            }
        };
        if self.clear_cells(range) {
            self.generation += 1;
            self.damage.mark(DamageRegion::Viewport);
        }
    }

    pub fn clear_line(&mut self, mode: EraseMode) {
        let row_start = usize::from(self.cursor.y) * usize::from(self.width);
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

        let old_width = self.width;
        let old_height = self.height;
        let mut cells = vec![Cell::default(); usize::from(width) * usize::from(height)];
        let copy_width = old_width.min(width);
        let copy_height = old_height.min(height);
        for y in 0..copy_height {
            let old_start = usize::from(y) * usize::from(old_width);
            let new_start = usize::from(y) * usize::from(width);
            let len = usize::from(copy_width);
            cells[new_start..new_start + len]
                .copy_from_slice(&self.cells[old_start..old_start + len]);
        }

        self.width = width;
        self.height = height;
        self.cells = cells;
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

    pub fn drain_damage(&mut self) -> crate::DamageBatch {
        self.damage.drain(self.generation)
    }

    pub fn drain_scrolled_rows(&mut self) -> Vec<Vec<Cell>> {
        std::mem::take(&mut self.scrolled_rows)
    }

    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(usize::from(y) * usize::from(self.width) + usize::from(x))
        } else {
            None
        }
    }

    fn fill_line_range(&mut self, y: u16, x_start: u16, x_end: u16) {
        if y >= self.height || x_start >= x_end {
            return;
        }
        let row_start = usize::from(y) * usize::from(self.width);
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
        self.cells[range].fill(Cell::default());
        changed
    }

    fn shift_chars(&mut self, count: u16, right: bool) {
        if self.cursor.x >= self.width {
            return;
        }

        let row_start = usize::from(self.cursor.y) * usize::from(self.width);
        let x = usize::from(self.cursor.x);
        let width = usize::from(self.width);
        let count = usize::from(count.min(self.width - self.cursor.x));
        let row_end = row_start + width;
        let mut changed = self.cells[row_start + x..row_end]
            .iter()
            .any(|cell| *cell != Cell::default());
        let (_, _, boundary_changed) = self.clear_cell_for_write(self.cursor.x, self.cursor.y);
        changed |= boundary_changed;

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
        let start = usize::from(top) * width;
        let end = (usize::from(bottom) + 1) * width;
        let count_cells = count * width;

        if !down && top == 0 && bottom == self.height.saturating_sub(1) {
            for row in 0..count {
                let row_start = start + row * width;
                self.scrolled_rows
                    .push(self.cells[row_start..row_start + width].to_vec());
            }
        }

        if down {
            self.cells
                .copy_within(start..end - count_cells, start + count_cells);
            self.cells[start..start + count_cells].fill(Cell::default());
        } else {
            self.cells.copy_within(start + count_cells..end, start);
            self.cells[end - count_cells..end].fill(Cell::default());
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
        self.generation += 1;
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

        (start, end, changed)
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

        let row_start = usize::from(y) * usize::from(self.width);
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
