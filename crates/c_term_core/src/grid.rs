use crate::{DamageRegion, DamageTracker, Generation};

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

    pub fn clear_screen(&mut self) {
        let mut changed = false;
        for cell in &mut self.cells {
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

    pub fn clear_line_from_cursor(&mut self) {
        let start = self.index(self.cursor.x, self.cursor.y).unwrap_or(0);
        let end = usize::from(self.cursor.y + 1) * usize::from(self.width);
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
                x: self.cursor.x,
                y: self.cursor.y,
                width: self.width.saturating_sub(self.cursor.x),
                height: 1,
            });
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) -> bool {
        if width == self.width && height == self.height {
            return false;
        }

        self.width = width.max(1);
        self.height = height.max(1);
        self.cells.resize(
            usize::from(self.width) * usize::from(self.height),
            Cell::default(),
        );
        self.cursor.x = self.cursor.x.min(self.width - 1);
        self.cursor.y = self.cursor.y.min(self.height - 1);
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

    fn advance_cursor(&mut self) {
        let old = self.cursor;
        if self.cursor.x + 1 < self.width {
            self.cursor.x += 1;
        } else {
            self.cursor.x = 0;
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

    fn line_feed(&mut self) {
        let old = self.cursor;
        self.cursor.x = 0;
        if self.cursor.y + 1 >= self.height {
            self.scroll_up();
        } else {
            self.cursor.y += 1;
        }
        if old != self.cursor {
            self.generation += 1;
            self.damage.mark(DamageRegion::Cursor {
                old: Some((old.x, old.y)),
                new: (self.cursor.x, self.cursor.y),
            });
        }
    }

    fn scroll_up(&mut self) {
        if self.height <= 1 {
            self.cells.fill(Cell::default());
        } else {
            let width = usize::from(self.width);
            self.cells.copy_within(width.., 0);
            let last_row = self.cells.len() - width;
            self.cells[last_row..].fill(Cell::default());
        }
        self.generation += 1;
        self.damage.mark(DamageRegion::Viewport);
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
