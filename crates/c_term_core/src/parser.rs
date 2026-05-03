#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParserAction {
    Print(char),
    Tab,
    LineFeed,
    CarriageReturn,
    Backspace,
    Bell,
    SetTitle(String),
    MoveCursor { x: u16, y: u16 },
    MoveCursorRelative { dx: i16, dy: i16 },
    SaveCursor,
    RestoreCursor,
    ReverseIndex,
    SetScrollRegion { top: u16, bottom: u16 },
    ClearScreen(EraseMode),
    ClearLine(EraseMode),
    EraseChars(u16),
    DeleteChars(u16),
    InsertBlankChars(u16),
    DeleteLines(u16),
    InsertBlankLines(u16),
    ScrollUp(u16),
    ScrollDown(u16),
    SetMode { mode: TerminalMode, enabled: bool },
    SetStyle(StyleUpdate),
    ResetStyle,
}

pub trait ParserAdapter {
    fn parse(&mut self, input: &[u8], actions: &mut Vec<ParserAction>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleUpdate {
    Foreground(crate::Color),
    Background(crate::Color),
    Bold(bool),
    Italic(bool),
    Underline(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraseMode {
    FromCursor,
    ToCursor,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMode {
    AlternateScreen,
    CursorVisible,
    Wrap,
}

pub struct SimpleParser {
    parser: vte::Parser,
}

impl Default for SimpleParser {
    fn default() -> Self {
        Self {
            parser: vte::Parser::new(),
        }
    }
}

impl std::fmt::Debug for SimpleParser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SimpleParser")
            .finish_non_exhaustive()
    }
}

impl ParserAdapter for SimpleParser {
    fn parse(&mut self, input: &[u8], actions: &mut Vec<ParserAction>) {
        let mut performer = ActionPerformer { actions };
        self.parser.advance(&mut performer, input);
    }
}

struct ActionPerformer<'a> {
    actions: &'a mut Vec<ParserAction>,
}

impl vte::Perform for ActionPerformer<'_> {
    fn print(&mut self, ch: char) {
        self.actions.push(ParserAction::Print(ch));
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.actions.push(ParserAction::LineFeed),
            b'\r' => self.actions.push(ParserAction::CarriageReturn),
            b'\t' => self.actions.push(ParserAction::Tab),
            0x08 => self.actions.push(ParserAction::Backspace),
            0x07 => self.actions.push(ParserAction::Bell),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(command) = params
            .first()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
        else {
            return;
        };
        if !matches!(command, "0" | "2") {
            return;
        }
        let Some(title) = params
            .get(1)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
        else {
            return;
        };
        self.actions.push(ParserAction::SetTitle(title.to_owned()));
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore {
            return;
        }

        let private = intermediates == b"?";
        if private && matches!(action, 'h' | 'l') {
            self.mode(params, action == 'h');
            return;
        }
        if !intermediates.is_empty() {
            return;
        }

        match action {
            '@' => self
                .actions
                .push(ParserAction::InsertBlankChars(param(params, 0, 1))),
            'A' => self.relative(0, -amount(params, 0, 1)),
            'B' => self.relative(0, amount(params, 0, 1)),
            'C' => self.relative(amount(params, 0, 1), 0),
            'D' => self.relative(-amount(params, 0, 1), 0),
            'G' => self.actions.push(ParserAction::MoveCursor {
                x: param(params, 0, 1).saturating_sub(1),
                y: u16::MAX,
            }),
            'H' | 'f' => self.actions.push(ParserAction::MoveCursor {
                x: param(params, 1, 1).saturating_sub(1),
                y: param(params, 0, 1).saturating_sub(1),
            }),
            'J' => self
                .actions
                .push(ParserAction::ClearScreen(erase_mode(params))),
            'K' => self
                .actions
                .push(ParserAction::ClearLine(erase_mode(params))),
            'L' => self
                .actions
                .push(ParserAction::InsertBlankLines(param(params, 0, 1))),
            'M' => self
                .actions
                .push(ParserAction::DeleteLines(param(params, 0, 1))),
            'P' => self
                .actions
                .push(ParserAction::DeleteChars(param(params, 0, 1))),
            'S' => self
                .actions
                .push(ParserAction::ScrollUp(param(params, 0, 1))),
            'T' => self
                .actions
                .push(ParserAction::ScrollDown(param(params, 0, 1))),
            'X' => self
                .actions
                .push(ParserAction::EraseChars(param(params, 0, 1))),
            'm' => self.sgr(params),
            'r' => self.actions.push(ParserAction::SetScrollRegion {
                top: param(params, 0, 1).saturating_sub(1),
                bottom: param(params, 1, u16::MAX).saturating_sub(1),
            }),
            's' => self.actions.push(ParserAction::SaveCursor),
            'u' => self.actions.push(ParserAction::RestoreCursor),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore || !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => self.actions.push(ParserAction::SaveCursor),
            b'8' => self.actions.push(ParserAction::RestoreCursor),
            b'M' => self.actions.push(ParserAction::ReverseIndex),
            _ => {}
        }
    }
}

impl ActionPerformer<'_> {
    fn relative(&mut self, dx: i16, dy: i16) {
        self.actions
            .push(ParserAction::MoveCursorRelative { dx, dy });
    }

    fn sgr(&mut self, params: &vte::Params) {
        if params.is_empty() {
            self.actions.push(ParserAction::ResetStyle);
            return;
        }

        let codes: Vec<_> = params
            .iter()
            .flat_map(|param| param.iter())
            .copied()
            .collect();
        let mut index = 0;
        while index < codes.len() {
            let code = codes[index];
            match code {
                0 => self.actions.push(ParserAction::ResetStyle),
                1 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Bold(true))),
                3 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Italic(true))),
                4 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Underline(true))),
                22 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Bold(false))),
                23 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Italic(false))),
                24 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Underline(false))),
                30..=37 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Foreground(
                        crate::Color::Indexed((code - 30) as u8),
                    ))),
                39 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Foreground(
                        crate::Color::DefaultForeground,
                    ))),
                40..=47 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Background(
                        crate::Color::Indexed((code - 40) as u8),
                    ))),
                49 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Background(
                        crate::Color::DefaultBackground,
                    ))),
                90..=97 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Foreground(
                        crate::Color::Indexed((code - 90 + 8) as u8),
                    ))),
                100..=107 => self
                    .actions
                    .push(ParserAction::SetStyle(StyleUpdate::Background(
                        crate::Color::Indexed((code - 100 + 8) as u8),
                    ))),
                38 | 48 => {
                    if let Some((color, consumed)) = extended_color(&codes[index + 1..]) {
                        let update = if code == 38 {
                            StyleUpdate::Foreground(color)
                        } else {
                            StyleUpdate::Background(color)
                        };
                        self.actions.push(ParserAction::SetStyle(update));
                        index += consumed;
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }

    fn mode(&mut self, params: &vte::Params, enabled: bool) {
        for code in params.iter().filter_map(|param| param.first()).copied() {
            let mode = match code {
                7 => TerminalMode::Wrap,
                25 => TerminalMode::CursorVisible,
                47 | 1047 | 1049 => TerminalMode::AlternateScreen,
                _ => continue,
            };
            self.actions.push(ParserAction::SetMode { mode, enabled });
        }
    }
}

fn param(params: &vte::Params, index: usize, default: u16) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|param| param.first())
        .copied()
        .filter(|value| *value != 0)
        .unwrap_or(default)
}

fn amount(params: &vte::Params, index: usize, default: u16) -> i16 {
    param(params, index, default).min(i16::MAX as u16) as i16
}

fn erase_mode(params: &vte::Params) -> EraseMode {
    match param(params, 0, 0) {
        1 => EraseMode::ToCursor,
        2 | 3 => EraseMode::All,
        _ => EraseMode::FromCursor,
    }
}

fn extended_color(codes: &[u16]) -> Option<(crate::Color, usize)> {
    match codes {
        [5, index, ..] => Some((crate::Color::Indexed((*index).min(u8::MAX as u16) as u8), 2)),
        [2, r, g, b, ..] => Some((
            crate::Color::Rgb(
                (*r).min(u8::MAX as u16) as u8,
                (*g).min(u8::MAX as u16) as u8,
                (*b).min(u8::MAX as u16) as u8,
            ),
            4,
        )),
        _ => None,
    }
}
