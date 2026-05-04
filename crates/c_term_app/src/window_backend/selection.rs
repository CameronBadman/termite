use c_term_core::{Cell, TerminalCore};

use crate::plugins::{OverlayCommand, OverlayKind};

use super::{CELL_HEIGHT, CELL_WIDTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    anchor: (u16, u16),
    focus: (u16, u16),
    dragging: bool,
}

impl Selection {
    pub(super) fn start(cell: (u16, u16)) -> Self {
        Self {
            anchor: cell,
            focus: cell,
            dragging: true,
        }
    }

    pub(super) fn update(&mut self, cell: (u16, u16)) -> bool {
        if self.focus == cell {
            return false;
        }
        self.focus = cell;
        true
    }

    pub(super) fn finish(&mut self) {
        self.dragging = false;
    }

    pub(super) fn is_dragging(self) -> bool {
        self.dragging
    }

    pub(super) fn text(self, terminal: &TerminalCore, scroll_offset: usize) -> String {
        let (start, end) = self.normalized();
        let mut text = String::new();
        for y in start.1..=end.1 {
            let x_start = if y == start.1 { start.0 } else { 0 };
            let x_end = if y == end.1 {
                end.0
            } else {
                terminal.grid().width().saturating_sub(1)
            };
            if let Some(row) = visible_row(terminal, scroll_offset, y) {
                push_row_text(&mut text, row, x_start, x_end);
            }
            if y != end.1 {
                text.push('\n');
            }
        }
        text
    }

    pub(super) fn overlays(self, width: u16, color: [u8; 3]) -> Vec<OverlayCommand> {
        let (start, end) = self.normalized();
        if start.1 == end.1 {
            return vec![selection_rect(
                start.0,
                start.1,
                end.0 - start.0 + 1,
                1,
                color,
            )];
        }

        let mut overlays = vec![
            selection_rect(start.0, start.1, width.saturating_sub(start.0), 1, color),
            selection_rect(0, end.1, end.0 + 1, 1, color),
        ];
        let middle_start = start.1 + 1;
        if middle_start < end.1 {
            overlays.push(selection_rect(
                0,
                middle_start,
                width,
                end.1 - middle_start,
                color,
            ));
        }
        overlays
    }

    fn normalized(self) -> ((u16, u16), (u16, u16)) {
        if self.anchor.1 < self.focus.1
            || (self.anchor.1 == self.focus.1 && self.anchor.0 <= self.focus.0)
        {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

fn visible_row(terminal: &TerminalCore, scroll_offset: usize, y: u16) -> Option<&[Cell]> {
    if scroll_offset == 0 || terminal.is_alternate_screen() {
        return terminal.grid().row(y);
    }

    let grid = terminal.grid();
    let history_len = terminal.scrollback_len();
    let height = usize::from(grid.height());
    let total_rows = history_len + height;
    let start = total_rows
        .saturating_sub(height)
        .saturating_sub(scroll_offset.min(history_len));
    let row = start + usize::from(y);
    if row < history_len {
        terminal.scrollback_row(row)
    } else {
        grid.row((row - history_len) as u16)
    }
}

fn push_row_text(text: &mut String, row: &[Cell], x_start: u16, x_end: u16) {
    let start = usize::from(x_start).min(row.len());
    let end = usize::from(x_end).saturating_add(1).min(row.len());
    let before = text.len();
    for cell in &row[start..end] {
        if !cell.spacer {
            text.push(cell.ch);
        }
    }
    let trimmed = text[before..].trim_end().len();
    text.truncate(before + trimmed);
}

fn selection_rect(x: u16, y: u16, width: u16, height: u16, color: [u8; 3]) -> OverlayCommand {
    let left = usize::from(x) * CELL_WIDTH as usize;
    let top = usize::from(y) * CELL_HEIGHT as usize;
    let right = left + usize::from(width) * CELL_WIDTH as usize;
    let bottom = top + usize::from(height) * CELL_HEIGHT as usize;
    OverlayCommand {
        kind: OverlayKind::Rect,
        color,
        alpha: 96,
        corners: [
            (right as f32, top as f32),
            (right as f32, bottom as f32),
            (left as f32, bottom as f32),
            (left as f32, top as f32),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_text_trims_row_edges() {
        let mut terminal = TerminalCore::new(5, 2);
        let _ = terminal.process_pty_input(b"abc  \r\nxy   ");
        let selection = Selection {
            anchor: (0, 0),
            focus: (4, 1),
            dragging: false,
        };

        assert_eq!(selection.text(&terminal, 0), "abc\nxy");
    }

    #[test]
    fn selected_scrollback_uses_visible_offset() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"aa\r\nbb\r\ncc\r\ndd");
        let selection = Selection {
            anchor: (0, 0),
            focus: (1, 0),
            dragging: false,
        };

        assert_eq!(selection.text(&terminal, 1), "bb");
    }

    #[test]
    fn selection_overlays_cover_rows() {
        let overlays = Selection {
            anchor: (1, 0),
            focus: (2, 1),
            dragging: false,
        }
        .overlays(4, [80, 130, 220]);

        assert_eq!(overlays.len(), 2);
        assert_eq!(overlays[0].kind, OverlayKind::Rect);
    }

    #[test]
    fn large_selection_uses_middle_rect() {
        let overlays = Selection {
            anchor: (1, 0),
            focus: (2, 4),
            dragging: false,
        }
        .overlays(5, [80, 130, 220]);

        assert_eq!(overlays.len(), 3);
    }
}
