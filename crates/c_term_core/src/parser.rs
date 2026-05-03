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
    ClearScreen,
    ClearLineFromCursor,
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
        if ignore || !intermediates.is_empty() {
            return;
        }

        match action {
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
            'J' if param(params, 0, 0) == 2 => self.actions.push(ParserAction::ClearScreen),
            'K' if matches!(param(params, 0, 0), 0 | 2) => {
                self.actions.push(ParserAction::ClearLineFromCursor)
            }
            'm' => self.sgr(params),
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

        for code in params.iter().filter_map(|param| param.first()).copied() {
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
                _ => {}
            }
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
