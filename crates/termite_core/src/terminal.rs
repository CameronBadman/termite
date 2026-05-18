use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::{
    Cursor, DamageBatch, Grid, MouseTracking, ParserAction, ParserAdapter, SimpleParser, Style,
    StyleUpdate, TerminalMode,
};

mod fast_path;
mod scrollback;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoreProfile {
    pub fast_sgr_time: Duration,
    pub fast_text_time: Duration,
    pub parser_time: Duration,
    pub apply_time: Duration,
    pub tick_time: Duration,
    pub fast_sgr_calls: u64,
    pub fast_text_calls: u64,
    pub parser_bytes: u64,
    pub actions: u64,
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
    actions: Vec<ParserAction>,
    fast_width1_chars: Vec<char>,
    profile_enabled: bool,
    profile: CoreProfile,
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
            actions: Vec::new(),
            fast_width1_chars: Vec::new(),
            profile_enabled: false,
            profile: CoreProfile::default(),
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

    pub fn set_profile_enabled(&mut self, enabled: bool) {
        self.profile_enabled = enabled;
    }

    pub fn drain_profile(&mut self) -> CoreProfile {
        std::mem::take(&mut self.profile)
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
        if self.profile_enabled {
            return self.process_pty_input_profiled(input);
        }

        if input.is_empty() {
            return self.tick();
        }
        while self.parser.can_process_ascii_fast_path() && !input.is_empty() {
            if input.starts_with(b"\x1b[")
                && let Some(sgr_len) = self.process_fast_sgr(input)
            {
                input = &input[sgr_len..];
                continue;
            }
            let consumed = self.process_fast_text(input);
            if consumed > 0 {
                if consumed == input.len() {
                    return self.tick();
                }
                input = &input[consumed..];
                continue;
            }
            break;
        }

        let mut actions = std::mem::take(&mut self.actions);
        actions.clear();
        self.parser.parse(input, &mut actions);

        for action in actions.drain(..) {
            self.apply_action(action);
        }
        self.actions = actions;
        self.tick()
    }

    fn process_pty_input_profiled(&mut self, mut input: &[u8]) -> CoreTick {
        if input.is_empty() {
            return self.profiled_tick();
        }
        while self.parser.can_process_ascii_fast_path() && !input.is_empty() {
            if input.starts_with(b"\x1b[")
                && let Some(sgr_len) = self.profiled_fast_sgr(input)
            {
                input = &input[sgr_len..];
                continue;
            }
            let consumed = self.profiled_fast_text(input);
            if consumed > 0 {
                if consumed == input.len() {
                    return self.profiled_tick();
                }
                input = &input[consumed..];
                continue;
            }
            break;
        }

        let mut actions = std::mem::take(&mut self.actions);
        actions.clear();
        let started = Instant::now();
        self.parser.parse(input, &mut actions);
        self.profile.parser_time += started.elapsed();
        self.profile.parser_bytes += input.len() as u64;

        let action_count = actions.len();
        let started = Instant::now();
        for action in actions.drain(..) {
            self.apply_action(action);
        }
        self.profile.apply_time += started.elapsed();
        self.profile.actions += action_count as u64;
        self.actions = actions;
        self.profiled_tick()
    }

    fn profiled_fast_sgr(&mut self, input: &[u8]) -> Option<usize> {
        let started = Instant::now();
        let result = self.process_fast_sgr(input);
        self.profile.fast_sgr_time += started.elapsed();
        self.profile.fast_sgr_calls += 1;
        result
    }

    fn profiled_fast_text(&mut self, input: &[u8]) -> usize {
        let started = Instant::now();
        let consumed = self.process_fast_text(input);
        self.profile.fast_text_time += started.elapsed();
        self.profile.fast_text_calls += 1;
        consumed
    }

    fn profiled_tick(&mut self) -> CoreTick {
        let started = Instant::now();
        let tick = self.tick();
        self.profile.tick_time += started.elapsed();
        tick
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
            ParserAction::PrintText(text) => self.process_print_text(&text),
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
                self.grid.carriage_return_line_feed();
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
            ParserAction::ScreenAlignment => self.grid.screen_alignment(self.style),
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
            StyleUpdate::Bold(enabled) => self.style.set_bold(enabled),
            StyleUpdate::Italic(enabled) => self.style.set_italic(enabled),
            StyleUpdate::Underline(enabled) => self.style.set_underline(enabled),
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
            let scrolled_rows = self.grid.drain_scrolled_rows();
            let recycled_rows = self.append_scrollback_rows(scrolled_rows);
            self.grid.recycle_scrolled_rows(recycled_rows);
        } else {
            let scrolled_rows = self.grid.drain_scrolled_rows();
            self.grid.recycle_scrolled_rows(scrolled_rows);
        }

        CoreTick {
            damage: self.grid.drain_damage(),
            output: std::mem::take(&mut self.output),
            clipboard: std::mem::take(&mut self.clipboard),
        }
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests;
