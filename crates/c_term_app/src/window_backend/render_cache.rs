use c_term_core::{Cell, DamageBatch, DamageRegion, Grid, TerminalCore};

use crate::{
    runner::{FontConfig, TerminalMetrics, TextRenderConfig},
    theme::Theme,
};

use super::text::TextRenderer;

pub(super) struct RenderCache {
    pub(super) frame: Vec<u8>,
    dirty_rows: Vec<bool>,
    upload_rows: Vec<bool>,
    upload_full: bool,
    scrolls: Vec<ScrollDamage>,
    upload_scrolls: Vec<ScrollDamage>,
    pub(super) dirty: bool,
    pub(super) scroll_start: Option<usize>,
    metrics: TerminalMetrics,
    text: TextRenderer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScrollDamage {
    pub(super) top: u16,
    pub(super) bottom: u16,
    pub(super) count: u16,
    pub(super) down: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RowBand {
    pub(super) start: u16,
    pub(super) end: u16,
}

#[derive(Debug, Default)]
pub(super) struct TextureUpdate {
    pub(super) full: bool,
    pub(super) scrolls: Vec<ScrollDamage>,
    pub(super) rows: Vec<RowBand>,
}

impl TextureUpdate {
    fn full() -> Self {
        Self {
            full: true,
            ..Default::default()
        }
    }
}

impl RenderCache {
    pub(super) fn new(
        font: FontConfig,
        theme: Theme,
        metrics: TerminalMetrics,
        text_render: TextRenderConfig,
    ) -> Self {
        Self {
            frame: Vec::new(),
            dirty_rows: Vec::new(),
            upload_rows: Vec::new(),
            upload_full: true,
            scrolls: Vec::new(),
            upload_scrolls: Vec::new(),
            dirty: true,
            scroll_start: None,
            metrics,
            text: TextRenderer::new(font, theme, metrics, text_render),
        }
    }

    pub(super) fn resize(&mut self, cols: u16, rows: u16) {
        self.frame = vec![0; frame_len(cols, rows, self.metrics)];
        self.dirty_rows = vec![false; usize::from(rows)];
        self.upload_rows = vec![false; usize::from(rows)];
        self.scrolls.clear();
        self.upload_scrolls.clear();
        self.dirty = true;
        self.upload_full = true;
    }

    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
        self.upload_full = true;
    }

    pub(super) fn apply_damage(&mut self, damage: &DamageBatch, rows: u16) {
        for region in &damage.regions {
            match *region {
                DamageRegion::Viewport => self.invalidate(),
                DamageRegion::Scroll {
                    top,
                    bottom,
                    count,
                    down,
                } => {
                    if top <= bottom && count > 0 && rows > 0 {
                        let bottom = bottom.min(rows - 1);
                        if let Some(last) = self.scrolls.last_mut()
                            && last.top == top
                            && last.bottom == bottom
                            && last.down == down
                        {
                            last.count = last.count.saturating_add(count);
                            continue;
                        }
                        self.scrolls.push(ScrollDamage {
                            top,
                            bottom,
                            count,
                            down,
                        });
                    }
                }
                DamageRegion::Cells { y, height, .. } => {
                    let end = y.saturating_add(height).min(rows);
                    for row in y..end {
                        if let Some(dirty) = self.dirty_rows.get_mut(usize::from(row)) {
                            *dirty = true;
                        }
                    }
                }
                DamageRegion::Cursor { .. } => {}
            }
        }
    }

    pub(super) fn update(&mut self, grid: &Grid) -> &[u8] {
        self.scroll_start = None;
        self.update_rows(grid.width(), grid.height(), |y| grid.row(y))
    }

    pub(super) fn update_scrollback(
        &mut self,
        terminal: &TerminalCore,
        scroll_offset: usize,
    ) -> &[u8] {
        let grid = terminal.grid();
        let cols = grid.width();
        let rows = grid.height();
        let height = usize::from(rows);
        let history_len = terminal.scrollback_len();
        let total_rows = history_len + height;
        let start = total_rows
            .saturating_sub(height)
            .saturating_sub(scroll_offset.min(history_len));

        self.ensure_shape(cols, rows);
        let width = usize::from(cols) * self.metrics.cell_width as usize;
        let previous = self.scroll_start;

        if self.dirty || previous.is_none() {
            self.frame.fill(0);
            for y in 0..rows {
                if let Some(row) = scrollback_row_at(terminal, start + usize::from(y)) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y, cols);
                }
            }
            self.dirty_rows.fill(false);
            self.dirty = false;
            self.scroll_start = Some(start);
            self.upload_full = true;
            return &self.frame;
        }

        let previous = previous.unwrap_or(start);
        if previous == start {
            return &self.frame;
        }

        let row_bytes = self.metrics.cell_height as usize * width * 4;
        let delta = start as isize - previous as isize;
        let distance = delta.unsigned_abs();
        if distance >= height {
            self.frame.fill(0);
            for y in 0..rows {
                if let Some(row) = scrollback_row_at(terminal, start + usize::from(y)) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y, cols);
                }
            }
        } else if delta > 0 {
            self.frame
                .copy_within(distance * row_bytes..height * row_bytes, 0);
            for y in height - distance..height {
                clear_grid_row(&mut self.frame, width, y as u16, self.metrics);
                if let Some(row) = scrollback_row_at(terminal, start + y) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y as u16, cols);
                }
            }
        } else {
            self.frame
                .copy_within(0..(height - distance) * row_bytes, distance * row_bytes);
            for y in 0..distance {
                clear_grid_row(&mut self.frame, width, y as u16, self.metrics);
                if let Some(row) = scrollback_row_at(terminal, start + y) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y as u16, cols);
                }
            }
        }

        self.dirty_rows.fill(false);
        self.dirty = false;
        self.upload_full = true;
        self.scroll_start = Some(start);
        &self.frame
    }

    fn update_rows<'a>(
        &mut self,
        cols: u16,
        rows: u16,
        mut row_at: impl FnMut(u16) -> Option<&'a [Cell]>,
    ) -> &[u8] {
        let expected_len = frame_len(cols, rows, self.metrics);
        if self.frame.len() != expected_len {
            self.frame.resize(expected_len, 0);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.dirty_rows.len() != usize::from(rows) {
            self.dirty_rows.resize(usize::from(rows), true);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.upload_rows.len() != usize::from(rows) {
            self.upload_rows.resize(usize::from(rows), false);
            self.upload_full = true;
        }

        let width = usize::from(cols) * self.metrics.cell_width as usize;
        if self.dirty {
            self.frame.fill(0);
            for y in 0..rows {
                if let Some(row) = row_at(y) {
                    self.text
                        .draw_row_to_frame(row, &mut self.frame, width, y, cols);
                }
            }
            self.dirty_rows.fill(false);
            self.dirty = false;
            self.scrolls.clear();
            self.upload_scrolls.clear();
            self.upload_rows.fill(false);
            self.upload_full = true;
            return &self.frame;
        }

        self.apply_scrolls(width, cols, rows, &mut row_at);

        for y in 0..rows {
            let Some(dirty) = self.dirty_rows.get_mut(usize::from(y)) else {
                continue;
            };
            if !*dirty {
                continue;
            }
            clear_grid_row(&mut self.frame, width, y, self.metrics);
            if let Some(row) = row_at(y) {
                self.text
                    .draw_row_to_frame(row, &mut self.frame, width, y, cols);
            }
            *dirty = false;
            if let Some(upload) = self.upload_rows.get_mut(usize::from(y)) {
                *upload = true;
            }
        }

        &self.frame
    }

    fn apply_scrolls<'a>(
        &mut self,
        width: usize,
        cols: u16,
        rows: u16,
        row_at: &mut impl FnMut(u16) -> Option<&'a [Cell]>,
    ) {
        let scrolls = std::mem::take(&mut self.scrolls);
        for scroll in scrolls {
            let top = scroll.top.min(rows.saturating_sub(1));
            let bottom = scroll.bottom.min(rows.saturating_sub(1));
            if top > bottom {
                continue;
            }
            let row_count = bottom - top + 1;
            let count = scroll.count.min(row_count);
            if count == 0 {
                continue;
            }

            let row_bytes = self.metrics.cell_height as usize * width * 4;
            let start = usize::from(top) * row_bytes;
            let end = (usize::from(bottom) + 1) * row_bytes;
            let count_bytes = usize::from(count) * row_bytes;
            if usize::from(count) >= usize::from(row_count) {
                for y in top..=bottom {
                    self.redraw_uploaded_row(width, cols, y, row_at);
                }
                continue;
            }

            if scroll.down {
                self.frame
                    .copy_within(start..end - count_bytes, start + count_bytes);
                for y in top..top + count {
                    self.redraw_uploaded_row(width, cols, y, row_at);
                }
            } else {
                self.frame.copy_within(start + count_bytes..end, start);
                for y in bottom + 1 - count..=bottom {
                    self.redraw_uploaded_row(width, cols, y, row_at);
                }
            }
            self.upload_scrolls.push(ScrollDamage {
                top,
                bottom,
                count,
                down: scroll.down,
            });
        }
    }

    fn redraw_uploaded_row<'a>(
        &mut self,
        width: usize,
        cols: u16,
        y: u16,
        row_at: &mut impl FnMut(u16) -> Option<&'a [Cell]>,
    ) {
        clear_grid_row(&mut self.frame, width, y, self.metrics);
        if let Some(row) = row_at(y) {
            self.text
                .draw_row_to_frame(row, &mut self.frame, width, y, cols);
        }
        if let Some(upload) = self.upload_rows.get_mut(usize::from(y)) {
            *upload = true;
        }
    }

    pub(super) fn take_texture_update(&mut self, rows: u16) -> TextureUpdate {
        if self.upload_full {
            self.upload_full = false;
            self.upload_rows.fill(false);
            self.upload_scrolls.clear();
            return TextureUpdate::full();
        }

        let mut update = TextureUpdate {
            scrolls: std::mem::take(&mut self.upload_scrolls),
            ..Default::default()
        };
        let row_limit = usize::from(rows).min(self.upload_rows.len());
        let mut y = 0;
        while y < row_limit {
            if !self.upload_rows[y] {
                y += 1;
                continue;
            }
            let start = y;
            while y < row_limit && self.upload_rows[y] {
                self.upload_rows[y] = false;
                y += 1;
            }
            update.rows.push(RowBand {
                start: start as u16,
                end: y as u16,
            });
        }
        update
    }

    fn ensure_shape(&mut self, cols: u16, rows: u16) {
        let expected_len = frame_len(cols, rows, self.metrics);
        if self.frame.len() != expected_len {
            self.frame.resize(expected_len, 0);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.dirty_rows.len() != usize::from(rows) {
            self.dirty_rows.resize(usize::from(rows), true);
            self.dirty = true;
            self.upload_full = true;
        }
        if self.upload_rows.len() != usize::from(rows) {
            self.upload_rows.resize(usize::from(rows), false);
            self.upload_full = true;
        }
    }
}

pub(super) fn frame_len(cols: u16, rows: u16, metrics: TerminalMetrics) -> usize {
    usize::from(cols)
        * metrics.cell_width as usize
        * usize::from(rows)
        * metrics.cell_height as usize
        * 4
}

fn scrollback_row_at(terminal: &TerminalCore, row: usize) -> Option<&[Cell]> {
    let grid = terminal.grid();
    let history_len = terminal.scrollback_len();
    if row < history_len {
        terminal.scrollback_row(row)
    } else {
        grid.row((row - history_len) as u16)
    }
}

fn clear_grid_row(frame: &mut [u8], width: usize, y: u16, metrics: TerminalMetrics) {
    let row_start = usize::from(y) * metrics.cell_height as usize * width * 4;
    let row_len = metrics.cell_height as usize * width * 4;
    if let Some(row) = frame.get_mut(row_start..row_start + row_len) {
        row.fill(0);
    }
}
