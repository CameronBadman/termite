#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParserAction {
    Print(char),
    Tab,
    LineFeed,
    NextLine,
    CarriageReturn,
    Backspace,
    Reset,
    Repeat(u16),
    MoveCursor { x: u16, y: u16 },
    MoveCursorRelative { dx: i16, dy: i16 },
    SaveCursor,
    RestoreCursor,
    ReverseIndex,
    SetScrollRegion { top: u16, bottom: u16 },
    ClearScreen(EraseMode),
    ClearScrollback,
    ClearLine(EraseMode),
    EraseChars(u16),
    DeleteChars(u16),
    InsertBlankChars(u16),
    DeleteLines(u16),
    InsertBlankLines(u16),
    ScrollUp(u16),
    ScrollDown(u16),
    SetMode { mode: TerminalMode, enabled: bool },
    SetCursorShape(crate::CursorShape),
    SetStyle(StyleUpdate),
    ClipboardStore { clipboard: u8, base64: Vec<u8> },
    ReportMode(u16),
    Respond(Vec<u8>),
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
    MouseTracking(MouseTracking),
    SgrMouse,
    BracketedPaste,
    SynchronizedUpdate,
    Wrap,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MouseTracking {
    #[default]
    None,
    Click,
    Drag,
    Any,
}

#[derive(Default)]
pub struct SimpleParser {
    parser: vte::Parser<262_144>,
    g0_line_drawing: bool,
    g1_line_drawing: bool,
    use_g1: bool,
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
        let mut performer = ActionPerformer {
            actions,
            g0_line_drawing: &mut self.g0_line_drawing,
            g1_line_drawing: &mut self.g1_line_drawing,
            use_g1: &mut self.use_g1,
        };
        self.parser.advance(&mut performer, input);
    }
}

struct ActionPerformer<'a> {
    actions: &'a mut Vec<ParserAction>,
    g0_line_drawing: &'a mut bool,
    g1_line_drawing: &'a mut bool,
    use_g1: &'a mut bool,
}

impl vte::Perform for ActionPerformer<'_> {
    fn print(&mut self, ch: char) {
        let line_drawing = if *self.use_g1 {
            *self.g1_line_drawing
        } else {
            *self.g0_line_drawing
        };
        self.actions
            .push(ParserAction::Print(map_printed_char(ch, line_drawing)));
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.actions.push(ParserAction::LineFeed),
            b'\r' => self.actions.push(ParserAction::CarriageReturn),
            b'\t' => self.actions.push(ParserAction::Tab),
            0x08 => self.actions.push(ParserAction::Backspace),
            0x0e => *self.use_g1 = true,
            0x0f => *self.use_g1 = false,
            _ => {}
        }
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

        let private = is_private(intermediates);
        if private && matches!(action, 'h' | 'l') {
            self.mode(params, action == 'h');
            return;
        }
        if private && action == 'u' {
            self.actions
                .push(ParserAction::Respond(b"\x1b[?0u".to_vec()));
            return;
        }
        if intermediates == b" " && action == 'q' {
            self.cursor_shape(params);
            return;
        }
        if action == 'p' && intermediates.contains(&b'$') {
            self.actions
                .push(ParserAction::ReportMode(param(params, 0, 0)));
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
            'E' => {
                self.relative(0, amount(params, 0, 1));
                self.actions
                    .push(ParserAction::MoveCursor { x: 0, y: u16::MAX });
            }
            'F' => {
                self.relative(0, -amount(params, 0, 1));
                self.actions
                    .push(ParserAction::MoveCursor { x: 0, y: u16::MAX });
            }
            'G' => self.actions.push(ParserAction::MoveCursor {
                x: param(params, 0, 1).saturating_sub(1),
                y: u16::MAX,
            }),
            'H' | 'f' => self.actions.push(ParserAction::MoveCursor {
                x: param(params, 1, 1).saturating_sub(1),
                y: param(params, 0, 1).saturating_sub(1),
            }),
            'J' => {
                self.actions
                    .push(ParserAction::ClearScreen(erase_mode(params)));
                if param(params, 0, 0) == 3 {
                    self.actions.push(ParserAction::ClearScrollback);
                }
            }
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
            'b' => self.actions.push(ParserAction::Repeat(param(params, 0, 1))),
            'c' => self
                .actions
                .push(ParserAction::Respond(b"\x1b[?1;2c".to_vec())),
            'd' => self.actions.push(ParserAction::MoveCursor {
                x: u16::MAX,
                y: param(params, 0, 1).saturating_sub(1),
            }),
            'e' => self.relative(0, amount(params, 0, 1)),
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
        if ignore {
            return;
        }
        if matches!(intermediates, b"(" | b")") && matches!(byte, b'0' | b'B') {
            let line_drawing = byte == b'0';
            if intermediates == b"(" {
                *self.g0_line_drawing = line_drawing;
            } else {
                *self.g1_line_drawing = line_drawing;
            }
            return;
        }
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            b'c' => self.actions.push(ParserAction::Reset),
            b'7' => self.actions.push(ParserAction::SaveCursor),
            b'8' => self.actions.push(ParserAction::RestoreCursor),
            b'E' => self.actions.push(ParserAction::NextLine),
            b'M' => self.actions.push(ParserAction::ReverseIndex),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if matches!(params, [b"11", b"?"]) {
            self.actions.push(ParserAction::Respond(
                b"\x1b]11;rgb:1010/1212/1818\x1b\\".to_vec(),
            ));
        }
        if let [b"52", clipboard, base64] = params
            && base64 != b"?"
        {
            self.actions.push(ParserAction::ClipboardStore {
                clipboard: clipboard.first().copied().unwrap_or(b'c'),
                base64: base64.to_vec(),
            });
        }
    }
}

fn is_private(intermediates: &[u8]) -> bool {
    intermediates.contains(&b'?')
}

fn map_printed_char(ch: char, line_drawing: bool) -> char {
    if !line_drawing {
        return ch;
    }
    match ch {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => ch,
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

        let mut codes = params.iter().flat_map(|param| param.iter()).copied();
        while let Some(code) = codes.next() {
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
                    if let Some(color) = extended_color(&mut codes) {
                        let update = if code == 38 {
                            StyleUpdate::Foreground(color)
                        } else {
                            StyleUpdate::Background(color)
                        };
                        self.actions.push(ParserAction::SetStyle(update));
                    }
                }
                _ => {}
            }
        }
    }

    fn mode(&mut self, params: &vte::Params, enabled: bool) {
        for code in params.iter().filter_map(|param| param.first()).copied() {
            let mode = match code {
                7 => TerminalMode::Wrap,
                25 => TerminalMode::CursorVisible,
                1000 => TerminalMode::MouseTracking(MouseTracking::Click),
                1002 => TerminalMode::MouseTracking(MouseTracking::Drag),
                1003 => TerminalMode::MouseTracking(MouseTracking::Any),
                1006 => TerminalMode::SgrMouse,
                2004 => TerminalMode::BracketedPaste,
                2026 => TerminalMode::SynchronizedUpdate,
                47 | 1047 | 1049 => TerminalMode::AlternateScreen,
                _ => continue,
            };
            self.actions.push(ParserAction::SetMode { mode, enabled });
        }
    }

    fn cursor_shape(&mut self, params: &vte::Params) {
        let shape = match param(params, 0, 1) {
            3 | 4 => crate::CursorShape::Underline,
            5 | 6 => crate::CursorShape::Beam,
            _ => crate::CursorShape::Block,
        };
        self.actions.push(ParserAction::SetCursorShape(shape));
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

fn extended_color(codes: &mut impl Iterator<Item = u16>) -> Option<crate::Color> {
    match codes.next()? {
        5 => Some(crate::Color::Indexed(
            codes.next()?.min(u8::MAX as u16) as u8
        )),
        2 => Some(crate::Color::Rgb(
            codes.next()?.min(u8::MAX as u16) as u8,
            codes.next()?.min(u8::MAX as u16) as u8,
            codes.next()?.min(u8::MAX as u16) as u8,
        )),
        _ => None,
    }
}
