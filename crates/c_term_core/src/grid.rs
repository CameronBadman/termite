use crate::{DamageRegion, DamageTracker, EraseMode, Generation};

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
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            style: Style::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
}

#[derive(Debug)]
pub struct Grid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
    cursor: Cursor,
    scroll_top: u16,
    scroll_bottom: u16,
    wrap: bool,
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
            },
            scroll_top: 0,
            scroll_bottom: height.saturating_sub(1),
            wrap: true,
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

    pub fn cell(&self, x: u16, y: u16) -> Option<Cell> {
        self.index(x, y).map(|index| self.cells[index])
    }

    pub fn visible_cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn put_char(&mut self, ch: char, style: Style) -> Option<(u16, u16, Cell)> {
        if ch == '\n' {
            self.line_feed();
            return None;
        }

        let x = self.cursor.x;
        let y = self.cursor.y;
        let index = self.index(x, y)?;
        let cell = Cell { ch, style };
        if self.cells[index] != cell {
            self.cells[index] = cell;
            self.generation += 1;
            self.damage.mark(DamageRegion::Cells {
                x,
                y,
                width: 1,
                height: 1,
            });
        }
        self.advance_cursor();
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
        };
        if new == self.cursor {
            return None;
        }

        let old = self.cursor;
        self.cursor = new;
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

    pub fn set_wrap(&mut self, enabled: bool) {
        self.wrap = enabled;
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
        let mut changed = false;
        for cell in &mut self.cells[range] {
            if *cell != Cell::default() {
                *cell = Cell::default();
                changed = true;
            }
        }
        if changed {
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
        let mut changed = false;
        for cell in &mut self.cells[start..end] {
            if *cell != Cell::default() {
                *cell = Cell::default();
                changed = true;
            }
        }
        if changed {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cells {
                x,
                y: self.cursor.y,
                width,
                height: 1,
            });
        }
    }

    pub fn erase_chars(&mut self, count: u16) {
        let end = self.cursor.x.saturating_add(count).min(self.width);
        self.fill_line_range(self.cursor.y, self.cursor.x, end);
    }

    pub fn delete_chars(&mut self, count: u16) {
        if self.cursor.x >= self.width {
            return;
        }
        let count = count.min(self.width - self.cursor.x);
        let y = self.cursor.y;
        let row_start = usize::from(y) * usize::from(self.width);
        let x = usize::from(self.cursor.x);
        let width = usize::from(self.width);
        let count = usize::from(count);
        let changed = self.cells[row_start + x..row_start + width]
            .iter()
            .any(|cell| *cell != Cell::default());

        for index in x..width - count {
            self.cells[row_start + index] = self.cells[row_start + index + count];
        }
        self.cells[row_start + width - count..row_start + width].fill(Cell::default());
        if changed {
            self.mark_line_damage(y, self.cursor.x, self.width - self.cursor.x);
        }
    }

    pub fn insert_blank_chars(&mut self, count: u16) {
        if self.cursor.x >= self.width {
            return;
        }
        let count = count.min(self.width - self.cursor.x);
        let y = self.cursor.y;
        let row_start = usize::from(y) * usize::from(self.width);
        let x = usize::from(self.cursor.x);
        let width = usize::from(self.width);
        let count = usize::from(count);
        let changed = self.cells[row_start + x..row_start + width]
            .iter()
            .any(|cell| *cell != Cell::default());

        for index in (x + count..width).rev() {
            self.cells[row_start + index] = self.cells[row_start + index - count];
        }
        self.cells[row_start + x..row_start + x + count].fill(Cell::default());
        if changed {
            self.mark_line_damage(y, self.cursor.x, self.width - self.cursor.x);
        }
    }

    pub fn delete_lines(&mut self, count: u16) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        let count = count.min(self.scroll_bottom - self.cursor.y + 1);
        let width = usize::from(self.width);
        let start = usize::from(self.cursor.y) * width;
        let bottom = usize::from(self.scroll_bottom) * width;
        let count_cells = usize::from(count) * width;
        if start + count_cells <= bottom {
            self.cells
                .copy_within(start + count_cells..bottom + width, start);
        }
        self.cells[bottom + width - count_cells..bottom + width].fill(Cell::default());
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
    }

    pub fn insert_blank_lines(&mut self, count: u16) {
        if self.cursor.y < self.scroll_top || self.cursor.y > self.scroll_bottom {
            return;
        }
        let count = count.min(self.scroll_bottom - self.cursor.y + 1);
        let width = usize::from(self.width);
        let start = usize::from(self.cursor.y) * width;
        let bottom = usize::from(self.scroll_bottom) * width;
        let count_cells = usize::from(count) * width;
        if start + count_cells <= bottom {
            self.cells
                .copy_within(start..bottom + width - count_cells, start + count_cells);
        }
        self.cells[start..start + count_cells].fill(Cell::default());
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
    }

    pub fn scroll_up(&mut self, count: u16) {
        let count = count.min(self.scroll_bottom - self.scroll_top + 1);
        let width = usize::from(self.width);
        let top = usize::from(self.scroll_top) * width;
        let bottom = usize::from(self.scroll_bottom) * width;
        let count_cells = usize::from(count) * width;
        if top + count_cells <= bottom {
            self.cells
                .copy_within(top + count_cells..bottom + width, top);
        }
        self.cells[bottom + width - count_cells..bottom + width].fill(Cell::default());
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
    }

    pub fn scroll_down(&mut self, count: u16) {
        let count = count.min(self.scroll_bottom - self.scroll_top + 1);
        let width = usize::from(self.width);
        let top = usize::from(self.scroll_top) * width;
        let bottom = usize::from(self.scroll_bottom) * width;
        let count_cells = usize::from(count) * width;
        if top + count_cells <= bottom {
            self.cells
                .copy_within(top..bottom + width - count_cells, top + count_cells);
        }
        self.cells[top..top + count_cells].fill(Cell::default());
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
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
        self.reset_scroll_region();
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
        true
    }

    pub fn drain_damage(&mut self) -> crate::DamageBatch {
        self.damage.drain(self.generation)
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
        let changed = self.cells[start..end]
            .iter()
            .any(|cell| *cell != Cell::default());
        self.cells[start..end].fill(Cell::default());
        if changed {
            self.mark_line_damage(y, x_start, x_end.saturating_sub(x_start));
        }
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

    fn advance_cursor(&mut self) {
        let old = self.cursor;
        if self.cursor.x + 1 < self.width {
            self.cursor.x += 1;
        } else if !self.wrap {
            self.cursor.x = self.width.saturating_sub(1);
        } else {
            self.cursor.x = 0;
            if self.cursor.y == self.scroll_bottom {
                self.scroll_up_region();
            } else {
                self.cursor.y = (self.cursor.y + 1).min(self.height - 1);
            }
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

    pub fn reverse_index(&mut self) {
        let old = self.cursor;
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
}

fn clamp_add(value: u16, delta: i16, limit: u16) -> u16 {
    let max = limit.saturating_sub(1);
    if delta < 0 {
        value.saturating_sub(delta.unsigned_abs()).min(max)
    } else {
        value.saturating_add(delta as u16).min(max)
    }
}
