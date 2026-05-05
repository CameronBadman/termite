use std::collections::VecDeque;

use crate::{
    Cursor, DamageBatch, Grid, MouseTracking, ParserAction, ParserAdapter, SimpleParser, Style,
    StyleUpdate, TerminalMode,
};

const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTick {
    pub damage: DamageBatch,
    pub output: Vec<u8>,
    pub clipboard: Vec<ClipboardStore>,
}

impl CoreTick {
    pub fn is_idle(&self) -> bool {
        self.damage.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardStore {
    pub clipboard: u8,
    pub base64: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseState {
    pub tracking: MouseTracking,
    pub sgr: bool,
}

#[derive(Debug)]
pub struct TerminalCore<P = SimpleParser> {
    grid: Grid,
    alternate_grid: Option<Grid>,
    scrollback: VecDeque<Vec<crate::Cell>>,
    scrollback_capacity: usize,
    parser: P,
    style: Style,
    saved_cursor: Option<Cursor>,
    last_printed: char,
    mouse: MouseState,
    bracketed_paste: bool,
    clipboard: Vec<ClipboardStore>,
    output: Vec<u8>,
}

impl TerminalCore<SimpleParser> {
    pub fn new(width: u16, height: u16) -> Self {
        Self::with_parser(width, height, SimpleParser::default())
    }
}

impl<P> TerminalCore<P>
where
    P: ParserAdapter,
{
    pub fn with_parser(width: u16, height: u16, parser: P) -> Self {
        Self {
            grid: Grid::new(width, height),
            alternate_grid: None,
            scrollback: VecDeque::new(),
            scrollback_capacity: DEFAULT_SCROLLBACK_LINES,
            parser,
            style: Style::default(),
            saved_cursor: None,
            last_printed: ' ',
            mouse: MouseState::default(),
            bracketed_paste: false,
            clipboard: Vec::new(),
            output: Vec::new(),
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn mouse(&self) -> MouseState {
        self.mouse
    }

    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    pub fn is_alternate_screen(&self) -> bool {
        self.alternate_grid.is_some()
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scrollback_row(&self, index: usize) -> Option<&[crate::Cell]> {
        self.scrollback.get(index).map(Vec::as_slice)
    }

    pub fn disable_synchronized_update(&mut self) {
        if self.grid.is_synchronized() {
            self.grid.set_synchronized(false);
            self.grid.invalidate();
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) -> CoreTick {
        let _ = self.grid.resize(width, height);
        if let Some(grid) = &mut self.alternate_grid {
            let _ = grid.resize(width, height);
        }
        self.tick()
    }

    pub fn resize_reflow(&mut self, width: u16, height: u16) -> CoreTick {
        if self.alternate_grid.is_some() {
            return self.resize(width, height);
        }

        let _ = self.grid.resize_reflow(width, height);
        self.tick()
    }

    pub fn process_pty_input(&mut self, mut input: &[u8]) -> CoreTick {
        if input.is_empty() {
            return self.tick();
        }
        if self.parser.can_process_ascii_fast_path() {
            let prefix_len = fast_ascii_prefix_len(input);
            if prefix_len > 0 {
                self.process_fast_ascii(&input[..prefix_len]);
                if prefix_len == input.len() {
                    return self.tick();
                }
                input = &input[prefix_len..];
            }
        }

        let mut actions = Vec::new();
        self.parser.parse(input, &mut actions);

        for action in actions {
            self.apply_action(action);
        }
        self.tick()
    }

    fn process_fast_ascii(&mut self, input: &[u8]) {
        let mut index = 0;
        while index < input.len() {
            if input[index].is_ascii_graphic() || input[index] == b' ' {
                let start = index;
                while index < input.len()
                    && (input[index].is_ascii_graphic() || input[index] == b' ')
                {
                    index += 1;
                }
                self.grid.put_ascii_run(&input[start..index], self.style);
                self.last_printed = char::from(input[index - 1]);
                continue;
            }

            match input[index] {
                b'\n' | 0x0b | 0x0c => {
                    let _ = self.grid.put_char('\n', self.style);
                }
                b'\r' => {
                    let _ = self.grid.move_cursor(0, self.grid.cursor().y);
                }
                b'\t' => self.grid.put_tab(self.style),
                0x08 => {
                    let cursor = self.grid.cursor();
                    if cursor.x > 0 {
                        let _ = self.grid.move_cursor(cursor.x - 1, cursor.y);
                    }
                }
                0x20..=0x7e => {
                    let ch = char::from(input[index]);
                    let _ = self.grid.put_char(ch, self.style);
                    self.last_printed = ch;
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn apply_action(&mut self, action: ParserAction) {
        match action {
            ParserAction::Print(ch) => {
                let _ = self.grid.put_char(ch, self.style);
                self.last_printed = ch;
            }
            ParserAction::PrintAscii(bytes) => {
                self.grid.put_ascii_run(&bytes, self.style);
                if let Some(&last) = bytes.last() {
                    self.last_printed = char::from(last);
                }
            }
            ParserAction::Repeat(count) => {
                for _ in 0..count {
                    let _ = self.grid.put_char(self.last_printed, self.style);
                }
            }
            ParserAction::Tab => self.grid.put_tab(self.style),
            ParserAction::LineFeed => {
                let _ = self.grid.put_char('\n', self.style);
            }
            ParserAction::NextLine => {
                let _ = self.grid.move_cursor(0, self.grid.cursor().y);
                let _ = self.grid.put_char('\n', self.style);
            }
            ParserAction::CarriageReturn => {
                let _ = self.grid.move_cursor(0, self.grid.cursor().y);
            }
            ParserAction::Backspace => {
                let cursor = self.grid.cursor();
                if cursor.x > 0 {
                    let _ = self.grid.move_cursor(cursor.x - 1, cursor.y);
                }
            }
            ParserAction::Reset => self.reset(),
            ParserAction::SaveCursor => self.saved_cursor = Some(self.grid.cursor()),
            ParserAction::RestoreCursor => {
                if let Some(cursor) = self.saved_cursor {
                    let _ = self.grid.move_cursor(cursor.x, cursor.y);
                }
            }
            ParserAction::ReverseIndex => self.grid.reverse_index(),
            ParserAction::SetScrollRegion { top, bottom } => {
                self.grid.set_scroll_region(top, bottom);
            }
            ParserAction::MoveCursor { x, y } => {
                let cursor = self.grid.cursor();
                let x = if x == u16::MAX { cursor.x } else { x };
                let y = if y == u16::MAX { cursor.y } else { y };
                let _ = self.grid.move_cursor(x, y);
            }
            ParserAction::MoveCursorRelative { dx, dy } => {
                let _ = self.grid.move_cursor_relative(dx, dy);
            }
            ParserAction::ClearScreen(mode) => self.grid.clear_screen(mode),
            ParserAction::ClearScrollback => self.scrollback.clear(),
            ParserAction::ClearLine(mode) => self.grid.clear_line(mode),
            ParserAction::EraseChars(count) => self.grid.erase_chars(count),
            ParserAction::DeleteChars(count) => self.grid.delete_chars(count),
            ParserAction::InsertBlankChars(count) => self.grid.insert_blank_chars(count),
            ParserAction::DeleteLines(count) => self.grid.delete_lines(count),
            ParserAction::InsertBlankLines(count) => self.grid.insert_blank_lines(count),
            ParserAction::ScrollUp(count) => self.grid.scroll_up(count),
            ParserAction::ScrollDown(count) => self.grid.scroll_down(count),
            ParserAction::SetMode { mode, enabled } => self.set_mode(mode, enabled),
            ParserAction::SetCursorShape(shape) => self.grid.set_cursor_shape(shape),
            ParserAction::SetStyle(update) => self.apply_style(update),
            ParserAction::ClipboardStore { clipboard, base64 } => {
                self.clipboard.push(ClipboardStore { clipboard, base64 });
            }
            ParserAction::ReportMode(mode) => self.report_mode(mode),
            ParserAction::Respond(bytes) => self.output.extend(bytes),
            ParserAction::ResetStyle => self.style = Style::default(),
        }
    }

    fn apply_style(&mut self, update: StyleUpdate) {
        match update {
            StyleUpdate::Foreground(color) => self.style.foreground = color,
            StyleUpdate::Background(color) => self.style.background = color,
            StyleUpdate::Bold(enabled) => self.style.bold = enabled,
            StyleUpdate::Italic(enabled) => self.style.italic = enabled,
            StyleUpdate::Underline(enabled) => self.style.underline = enabled,
        }
    }

    fn set_mode(&mut self, mode: TerminalMode, enabled: bool) {
        match mode {
            TerminalMode::AlternateScreen => {
                if enabled && self.alternate_grid.is_none() {
                    self.saved_cursor = Some(self.grid.cursor());
                    let replacement = Grid::new(self.grid.width(), self.grid.height());
                    self.alternate_grid = Some(std::mem::replace(&mut self.grid, replacement));
                } else if !enabled && self.alternate_grid.is_some() {
                    if let Some(primary) = self.alternate_grid.take() {
                        self.grid = primary;
                    }
                    if let Some(cursor) = self.saved_cursor {
                        let _ = self.grid.move_cursor(cursor.x, cursor.y);
                    }
                    self.grid.invalidate();
                }
            }
            TerminalMode::CursorVisible => self.grid.set_cursor_visible(enabled),
            TerminalMode::MouseTracking(tracking) => {
                self.mouse.tracking = if enabled {
                    tracking
                } else if self.mouse.tracking == tracking {
                    MouseTracking::None
                } else {
                    self.mouse.tracking
                };
            }
            TerminalMode::SgrMouse => self.mouse.sgr = enabled,
            TerminalMode::BracketedPaste => self.bracketed_paste = enabled,
            TerminalMode::SynchronizedUpdate => self.grid.set_synchronized(enabled),
            TerminalMode::Wrap => self.grid.set_wrap(enabled),
        }
    }

    fn reset(&mut self) {
        let width = self.grid.width();
        let height = self.grid.height();
        self.grid = Grid::new(width, height);
        self.alternate_grid = None;
        self.scrollback.clear();
        self.style = Style::default();
        self.saved_cursor = None;
        self.last_printed = ' ';
        self.mouse = MouseState::default();
        self.bracketed_paste = false;
    }

    fn report_mode(&mut self, mode: u16) {
        let status = match mode {
            2026 => {
                if self.grid.is_synchronized() {
                    1
                } else {
                    2
                }
            }
            _ => 0,
        };
        self.output
            .extend(format!("\x1b[?{mode};{status}$y").as_bytes());
    }

    fn tick(&mut self) -> CoreTick {
        if self.alternate_grid.is_none() {
            for row in self.grid.drain_scrolled_rows() {
                self.scrollback.push_back(row);
                while self.scrollback.len() > self.scrollback_capacity {
                    self.scrollback.pop_front();
                }
            }
        } else {
            let _ = self.grid.drain_scrolled_rows();
        }

        CoreTick {
            damage: self.grid.drain_damage(),
            output: std::mem::take(&mut self.output),
            clipboard: std::mem::take(&mut self.clipboard),
        }
    }
}

fn is_fast_ascii(byte: u8) -> bool {
    matches!(
        byte,
        b'\n' | b'\r' | b'\t' | 0x08 | 0x0b | 0x0c | 0x20..=0x7e
    )
}

fn fast_ascii_prefix_len(input: &[u8]) -> usize {
    input
        .iter()
        .position(|byte| !is_fast_ascii(*byte))
        .unwrap_or(input.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_idle_after_initial_damage_drained() {
        let mut terminal = TerminalCore::new(2, 2);
        let _ = terminal.process_pty_input(b"");

        assert!(terminal.process_pty_input(b"").is_idle());
    }

    #[test]
    fn printable_input_changes_cells_and_cursor() {
        let mut terminal = TerminalCore::new(3, 2);
        let tick = terminal.process_pty_input(b"ab");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
        assert!(!tick.damage.is_empty());
    }

    #[test]
    fn fast_ascii_path_handles_plain_controls() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"ab\rc\nD");

        assert_eq!(row_text(terminal.grid(), 0), "cb  ");
        assert_eq!(row_text(terminal.grid(), 1), " D  ");
    }

    #[test]
    fn clear_command_sequence_clears_visible_screen() {
        let mut terminal = TerminalCore::new(4, 2);
        let tick = terminal.process_pty_input(b"ab\x1b[2;1Hcd\x1b[H\x1b[J");

        assert_eq!(row_text(terminal.grid(), 0), "    ");
        assert_eq!(row_text(terminal.grid(), 1), "    ");
        assert!(!tick.damage.is_empty());
    }

    #[test]
    fn full_clear_sequence_clears_visible_screen() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"ab\x1b[2;1Hcd\x1b[H\x1b[2J\x1b[3J");

        assert_eq!(row_text(terminal.grid(), 0), "    ");
        assert_eq!(row_text(terminal.grid(), 1), "    ");
    }

    #[test]
    fn clear_scrollback_sequence_removes_history() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");
        assert_eq!(terminal.scrollback_len(), 1);

        let _ = terminal.process_pty_input(b"\x1b[3J");

        assert_eq!(terminal.scrollback_len(), 0);
    }

    #[test]
    fn reset_sequence_resets_screen_and_cursor() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"ab\x1b[2;3Hcd\x1bc");

        assert_eq!(row_text(terminal.grid(), 0), "    ");
        assert_eq!(row_text(terminal.grid(), 1), "    ");
        assert_eq!(terminal.grid().cursor().x, 0);
        assert_eq!(terminal.grid().cursor().y, 0);
    }

    #[test]
    fn resize_emits_viewport_damage() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"");
        let tick = terminal.resize(4, 4);

        assert_eq!(terminal.grid().width(), 4);
        assert!(
            tick.damage
                .regions
                .iter()
                .any(|region| matches!(region, crate::DamageRegion::Viewport))
        );
    }

    #[test]
    fn resize_preserves_cell_coordinates() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"ab\x1b[2;1Hcd");

        let _ = terminal.resize(6, 3);

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
        assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'c');
        assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'd');
    }

    #[test]
    fn resize_truncates_rows_by_coordinates() {
        let mut terminal = TerminalCore::new(5, 2);
        let _ = terminal.process_pty_input(b"abcd\x1b[2;1Hefgh");

        let _ = terminal.resize(2, 2);

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
        assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'e');
        assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'f');
    }

    #[test]
    fn resize_reflow_wraps_existing_rows_to_new_width() {
        let mut terminal = TerminalCore::new(6, 3);
        let _ = terminal.process_pty_input(b"abcdef");

        let _ = terminal.resize_reflow(3, 3);

        assert_eq!(row_text(terminal.grid(), 0), "abc");
        assert_eq!(row_text(terminal.grid(), 1), "def");
        assert_eq!(row_text(terminal.grid(), 2), "   ");
    }

    #[test]
    fn resize_reflow_moves_overflow_into_scrollback() {
        let mut terminal = TerminalCore::new(6, 2);
        let _ = terminal.process_pty_input(b"abcdef\x1b[2;1Hghijkl");

        let _ = terminal.resize_reflow(3, 2);

        assert_eq!(terminal.scrollback_len(), 2);
        assert_eq!(row_slice_text(terminal.scrollback_row(0).unwrap()), "abc");
        assert_eq!(row_slice_text(terminal.scrollback_row(1).unwrap()), "def");
        assert_eq!(row_text(terminal.grid(), 0), "ghi");
        assert_eq!(row_text(terminal.grid(), 1), "jkl");
    }

    #[test]
    fn erase_chars_removes_stale_cells() {
        let mut terminal = TerminalCore::new(6, 1);
        let _ = terminal.process_pty_input(b"abcde\x1b[1;2H\x1b[2X");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, ' ');
        assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, ' ');
        assert_eq!(terminal.grid().cell(3, 0).unwrap().ch, 'd');
    }

    #[test]
    fn delete_chars_shifts_line_left() {
        let mut terminal = TerminalCore::new(7, 1);
        let _ = terminal.process_pty_input(b"abcdef\x1b[1;3H\x1b[2P");

        assert_eq!(row_text(terminal.grid(), 0), "abef   ");
    }

    #[test]
    fn insert_blank_chars_shifts_line_right() {
        let mut terminal = TerminalCore::new(7, 1);
        let _ = terminal.process_pty_input(b"abcdef\x1b[1;3H\x1b[2@");

        assert_eq!(row_text(terminal.grid(), 0), "ab  cde");
    }

    #[test]
    fn delete_and_insert_lines_clear_scrolled_rows() {
        let mut terminal = TerminalCore::new(4, 3);
        let _ = terminal.process_pty_input(b"aaa\x1b[2;1Hbbb\x1b[3;1Hccc\x1b[2;1H\x1b[1M");

        assert_eq!(row_text(terminal.grid(), 0), "aaa ");
        assert_eq!(row_text(terminal.grid(), 1), "ccc ");
        assert_eq!(row_text(terminal.grid(), 2), "    ");

        let _ = terminal.process_pty_input(b"\x1b[2;1H\x1b[1L");
        assert_eq!(row_text(terminal.grid(), 1), "    ");
        assert_eq!(row_text(terminal.grid(), 2), "ccc ");
    }

    #[test]
    fn scroll_up_and_down_apply_to_scroll_region() {
        let mut terminal = TerminalCore::new(4, 4);
        let _ = terminal
            .process_pty_input(b"aaa\x1b[2;1Hbbb\x1b[3;1Hccc\x1b[4;1Hddd\x1b[2;4r\x1b[1;1H\x1b[1S");

        assert_eq!(row_text(terminal.grid(), 0), "aaa ");
        assert_eq!(row_text(terminal.grid(), 1), "ccc ");
        assert_eq!(row_text(terminal.grid(), 2), "ddd ");
        assert_eq!(row_text(terminal.grid(), 3), "    ");

        let _ = terminal.process_pty_input(b"\x1b[1T");
        assert_eq!(row_text(terminal.grid(), 1), "    ");
        assert_eq!(row_text(terminal.grid(), 2), "ccc ");
        assert_eq!(row_text(terminal.grid(), 3), "ddd ");
    }

    #[test]
    fn csi_cursor_position_writes_at_requested_cell() {
        let mut terminal = TerminalCore::new(4, 3);
        let _ = terminal.process_pty_input(b"\x1b[2;3Hx");

        assert_eq!(terminal.grid().cell(2, 1).unwrap().ch, 'x');
    }

    #[test]
    fn line_feed_scrolls_at_bottom() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'c');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'd');
        assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'e');
        assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'f');
    }

    #[test]
    fn primary_full_screen_scroll_adds_scrollback() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");

        assert_eq!(terminal.scrollback_len(), 1);
        assert_eq!(row_slice_text(terminal.scrollback_row(0).unwrap()), "ab");
    }

    #[test]
    fn alternate_screen_scroll_does_not_add_scrollback() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"\x1b[?1049hab\r\ncd\r\nef\x1b[?1049l");

        assert_eq!(terminal.scrollback_len(), 0);
    }

    #[test]
    fn line_feed_preserves_column_without_carriage_return() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"ab\nc");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
        assert_eq!(terminal.grid().cell(2, 1).unwrap().ch, 'c');
    }

    #[test]
    fn next_line_moves_to_column_zero() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"ab\x1bEc");

        assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'c');
    }

    #[test]
    fn last_column_wrap_is_deferred_until_next_print() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"abc");

        assert_eq!(row_text(terminal.grid(), 0), "abc");
        assert_eq!(terminal.grid().cursor().x, 2);
        assert_eq!(terminal.grid().cursor().y, 0);

        let _ = terminal.process_pty_input(b"d");

        assert_eq!(row_text(terminal.grid(), 0), "abc");
        assert_eq!(row_text(terminal.grid(), 1), "d  ");
        assert_eq!(terminal.grid().cursor().x, 1);
        assert_eq!(terminal.grid().cursor().y, 1);
    }

    #[test]
    fn wide_characters_leave_spacer_cells() {
        let mut terminal = TerminalCore::new(4, 1);
        let _ = terminal.process_pty_input("表x".as_bytes());

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, '表');
        assert!(terminal.grid().cell(0, 0).unwrap().wide);
        assert!(terminal.grid().cell(1, 0).unwrap().spacer);
        assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, 'x');
    }

    #[test]
    fn writing_over_wide_spacer_clears_the_leading_cell() {
        let mut terminal = TerminalCore::new(4, 1);
        let _ = terminal.process_pty_input("表\x1b[1;2Hx".as_bytes());

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, ' ');
        assert!(!terminal.grid().cell(0, 0).unwrap().wide);
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'x');
        assert!(!terminal.grid().cell(1, 0).unwrap().spacer);
    }

    #[test]
    fn sgr_applies_style_to_later_cells() {
        let mut terminal = TerminalCore::new(2, 1);
        let _ = terminal.process_pty_input(b"\x1b[31;1ma");
        let cell = terminal.grid().cell(0, 0).unwrap();

        assert_eq!(cell.style.foreground, crate::Color::Indexed(1));
        assert!(cell.style.bold);
    }

    #[test]
    fn alternate_screen_restores_primary_grid() {
        let mut terminal = TerminalCore::new(4, 1);
        let tick = terminal.process_pty_input(b"abc\x1b[?1049hxyz\x1b[?1049l");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'b');
        assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, 'c');
        assert!(
            tick.damage
                .regions
                .iter()
                .any(|region| matches!(region, crate::DamageRegion::Viewport))
        );
    }

    #[test]
    fn scroll_region_limits_line_feed_scrolling() {
        let mut terminal = TerminalCore::new(3, 3);
        let _ = terminal.process_pty_input(b"aa\r\nbb\r\ncc\x1b[2;3r\x1b[3;1H\r\nDD");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'a');
        assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'c');
        assert_eq!(terminal.grid().cell(0, 2).unwrap().ch, 'D');
    }

    #[test]
    fn extended_sgr_colors_are_applied() {
        let mut terminal = TerminalCore::new(3, 1);
        let _ = terminal.process_pty_input(b"\x1b[38;5;196mA\x1b[48;2;1;2;3mB");

        assert_eq!(
            terminal.grid().cell(0, 0).unwrap().style.foreground,
            crate::Color::Indexed(196)
        );
        assert_eq!(
            terminal.grid().cell(1, 0).unwrap().style.background,
            crate::Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn cursor_visibility_mode_updates_grid_cursor() {
        let mut terminal = TerminalCore::new(2, 1);
        let _ = terminal.process_pty_input(b"\x1b[?25l");

        assert!(!terminal.grid().cursor().visible);
    }

    #[test]
    fn cursor_shape_sequence_updates_grid_cursor() {
        let mut terminal = TerminalCore::new(2, 1);

        let _ = terminal.process_pty_input(b"\x1b[6 q");
        assert_eq!(terminal.grid().cursor().shape, crate::CursorShape::Beam);

        let _ = terminal.process_pty_input(b"\x1b[4 q");
        assert_eq!(
            terminal.grid().cursor().shape,
            crate::CursorShape::Underline
        );

        let _ = terminal.process_pty_input(b"\x1b[2 q");
        assert_eq!(terminal.grid().cursor().shape, crate::CursorShape::Block);
    }

    #[test]
    fn synchronized_update_mode_is_tracked() {
        let mut terminal = TerminalCore::new(2, 1);

        let _ = terminal.process_pty_input(b"\x1b[?2026h");
        assert!(terminal.grid().is_synchronized());

        let _ = terminal.process_pty_input(b"\x1b[?2026l");
        assert!(!terminal.grid().is_synchronized());
    }

    #[test]
    fn mouse_modes_are_tracked() {
        let mut terminal = TerminalCore::new(2, 1);

        let _ = terminal.process_pty_input(b"\x1b[?1000;1006h");
        assert_eq!(terminal.mouse().tracking, MouseTracking::Click);
        assert!(terminal.mouse().sgr);

        let _ = terminal.process_pty_input(b"\x1b[?1000l");
        assert_eq!(terminal.mouse().tracking, MouseTracking::None);
        assert!(terminal.mouse().sgr);

        let _ = terminal.process_pty_input(b"\x1b[?1006l");
        assert!(!terminal.mouse().sgr);
    }

    #[test]
    fn bracketed_paste_mode_is_tracked() {
        let mut terminal = TerminalCore::new(2, 1);

        let _ = terminal.process_pty_input(b"\x1b[?2004h");
        assert!(terminal.bracketed_paste());

        let _ = terminal.process_pty_input(b"\x1b[?2004l");
        assert!(!terminal.bracketed_paste());
    }

    #[test]
    fn repeat_sequence_reprints_previous_character() {
        let mut terminal = TerminalCore::new(6, 1);
        let _ = terminal.process_pty_input(b"A\x1b[3b");

        assert_eq!(row_text(terminal.grid(), 0), "AAAA  ");
    }

    #[test]
    fn terminal_queries_emit_pty_responses() {
        let mut terminal = TerminalCore::new(2, 1);

        assert_eq!(
            terminal.process_pty_input(b"\x1b[c").output,
            crate::PRIMARY_DEVICE_ATTRIBUTES
        );
        assert_eq!(
            terminal.process_pty_input(b"\x1b[>c").output,
            crate::SECONDARY_DEVICE_ATTRIBUTES
        );
        assert_eq!(
            terminal.process_pty_input(b"\x1b[>q").output,
            crate::version_reply()
        );
        assert_eq!(
            terminal.process_pty_input(b"\x1b[?u").output,
            crate::KEYBOARD_PROTOCOL_QUERY
        );
        assert_eq!(
            terminal.process_pty_input(b"\x1b[?2026$p").output,
            b"\x1b[?2026;2$y"
        );
        assert_eq!(
            terminal.process_pty_input(b"\x1b]11;?\x07").output,
            crate::DEFAULT_BACKGROUND_REPLY
        );
    }

    #[test]
    fn osc52_clipboard_store_is_reported() {
        let mut terminal = TerminalCore::new(2, 1);
        let tick = terminal.process_pty_input(b"\x1b]52;c;aGVsbG8=\x07");

        assert_eq!(tick.clipboard.len(), 1);
        assert_eq!(tick.clipboard[0].clipboard, b'c');
        assert_eq!(tick.clipboard[0].base64, b"aGVsbG8=");
    }

    #[test]
    fn osc52_empty_selection_defaults_to_clipboard() {
        let mut terminal = TerminalCore::new(2, 1);
        let tick = terminal.process_pty_input(b"\x1b]52;;aGVsbG8=\x07");

        assert_eq!(tick.clipboard[0].clipboard, b'c');
        assert_eq!(tick.clipboard[0].base64, b"aGVsbG8=");
    }

    #[test]
    fn dec_special_graphics_map_tmux_line_drawing() {
        let mut terminal = TerminalCore::new(8, 1);
        let _ = terminal.process_pty_input(b"\x1b(0lqkxmj\x1b(Bq");

        assert_eq!(row_text(terminal.grid(), 0), "┌─┐│└┘q ");
    }

    #[test]
    fn dec_g1_special_graphics_shift_in_and_out() {
        let mut terminal = TerminalCore::new(5, 1);
        let _ = terminal.process_pty_input(b"\x1b)0\x0ex\x0fq");

        assert_eq!(row_text(terminal.grid(), 0), "│q   ");
    }

    #[test]
    fn save_and_restore_cursor_moves_back() {
        let mut terminal = TerminalCore::new(4, 1);
        let _ = terminal.process_pty_input(b"\x1b[3G\x1b[sA\x1b[1G\x1b[uB");

        assert_eq!(terminal.grid().cell(2, 0).unwrap().ch, 'B');
    }

    fn row_text(grid: &Grid, y: u16) -> String {
        (0..grid.width())
            .map(|x| grid.cell(x, y).unwrap().ch)
            .collect()
    }

    fn row_slice_text(row: &[crate::Cell]) -> String {
        row.iter().map(|cell| cell.ch).collect()
    }
}
