use crate::{
    CoreEvent, CursorPosition, DamageBatch, Grid, KeyPress, ParserAction, ParserAdapter,
    SimpleParser, Style, StyleUpdate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTick {
    pub damage: DamageBatch,
    pub events: Vec<CoreEvent>,
}

impl CoreTick {
    pub fn is_idle(&self) -> bool {
        self.damage.is_empty() && self.events.is_empty()
    }
}

#[derive(Debug)]
pub struct TerminalCore<P = SimpleParser> {
    grid: Grid,
    parser: P,
    style: Style,
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
            parser,
            style: Style::default(),
        }
    }

    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    pub fn resize(&mut self, width: u16, height: u16) -> CoreTick {
        let mut events = Vec::new();
        if self.grid.resize(width, height) {
            events.push(CoreEvent::ViewportChanged);
        }
        self.tick(events)
    }

    pub fn handle_keypress(&mut self, keypress: KeyPress) -> CoreTick {
        self.tick(vec![CoreEvent::KeyPress(keypress)])
    }

    pub fn process_pty_input(&mut self, input: &[u8]) -> CoreTick {
        if input.is_empty() {
            return self.tick(Vec::new());
        }

        let mut actions = Vec::new();
        self.parser.parse(input, &mut actions);

        let mut events = Vec::new();
        for action in actions {
            self.apply_action(action, &mut events);
        }
        self.tick(events)
    }

    fn apply_action(&mut self, action: ParserAction, events: &mut Vec<CoreEvent>) {
        match action {
            ParserAction::Print(ch) => {
                let old_cursor = self.grid.cursor();
                if let Some((x, y, cell)) = self.grid.put_char(ch, self.style) {
                    events.push(CoreEvent::CellChanged { x, y, cell });
                }
                let new_cursor = self.grid.cursor();
                if old_cursor != new_cursor {
                    events.push(CoreEvent::CursorMoved {
                        old: CursorPosition::from(old_cursor),
                        new: CursorPosition::from(new_cursor),
                    });
                }
            }
            ParserAction::Tab => {
                let old_cursor = self.grid.cursor();
                self.grid.put_tab(self.style);
                let new_cursor = self.grid.cursor();
                if old_cursor != new_cursor {
                    events.push(CoreEvent::CursorMoved {
                        old: CursorPosition::from(old_cursor),
                        new: CursorPosition::from(new_cursor),
                    });
                }
            }
            ParserAction::LineFeed => {
                let old_cursor = self.grid.cursor();
                self.grid.put_char('\n', self.style);
                let new_cursor = self.grid.cursor();
                if old_cursor != new_cursor {
                    events.push(CoreEvent::CursorMoved {
                        old: CursorPosition::from(old_cursor),
                        new: CursorPosition::from(new_cursor),
                    });
                }
            }
            ParserAction::CarriageReturn => {
                if let Some((old, new)) = self.grid.move_cursor(0, self.grid.cursor().y) {
                    events.push(CoreEvent::CursorMoved {
                        old: CursorPosition::from(old),
                        new: CursorPosition::from(new),
                    });
                }
            }
            ParserAction::Backspace => {
                let cursor = self.grid.cursor();
                if cursor.x > 0 {
                    if let Some((old, new)) = self.grid.move_cursor(cursor.x - 1, cursor.y) {
                        events.push(CoreEvent::CursorMoved {
                            old: CursorPosition::from(old),
                            new: CursorPosition::from(new),
                        });
                    }
                }
            }
            ParserAction::Bell => events.push(CoreEvent::Bell),
            ParserAction::SetTitle(title) => events.push(CoreEvent::TitleChanged(title)),
            ParserAction::MoveCursor { x, y } => {
                let cursor = self.grid.cursor();
                let y = if y == u16::MAX { cursor.y } else { y };
                if let Some((old, new)) = self.grid.move_cursor(x, y) {
                    events.push(CoreEvent::CursorMoved {
                        old: CursorPosition::from(old),
                        new: CursorPosition::from(new),
                    });
                }
            }
            ParserAction::MoveCursorRelative { dx, dy } => {
                if let Some((old, new)) = self.grid.move_cursor_relative(dx, dy) {
                    events.push(CoreEvent::CursorMoved {
                        old: CursorPosition::from(old),
                        new: CursorPosition::from(new),
                    });
                }
            }
            ParserAction::ClearScreen => self.grid.clear_screen(),
            ParserAction::ClearLineFromCursor => self.grid.clear_line_from_cursor(),
            ParserAction::SetStyle(update) => self.apply_style(update),
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

    fn tick(&mut self, events: Vec<CoreEvent>) -> CoreTick {
        CoreTick {
            damage: self.grid.drain_damage(),
            events,
        }
    }
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
        assert!(
            tick.events
                .iter()
                .any(|event| matches!(event, CoreEvent::CellChanged { x: 0, y: 0, .. }))
        );
        assert!(
            tick.events
                .iter()
                .any(|event| matches!(event, CoreEvent::CursorMoved { .. }))
        );
        assert!(!tick.damage.is_empty());
    }

    #[test]
    fn resize_emits_viewport_damage() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"");
        let tick = terminal.resize(4, 4);

        assert_eq!(terminal.grid().width(), 4);
        assert!(tick.events.contains(&CoreEvent::ViewportChanged));
        assert!(
            tick.damage
                .regions
                .iter()
                .any(|region| matches!(region, crate::DamageRegion::Viewport))
        );
    }

    #[test]
    fn csi_cursor_position_writes_at_requested_cell() {
        let mut terminal = TerminalCore::new(4, 3);
        let _ = terminal.process_pty_input(b"\x1b[2;3Hx");

        assert_eq!(terminal.grid().cell(2, 1).unwrap().ch, 'x');
    }

    #[test]
    fn line_feed_scrolls_at_bottom() {
        let mut terminal = TerminalCore::new(2, 2);
        let _ = terminal.process_pty_input(b"ab\ncd\nef");

        assert_eq!(terminal.grid().cell(0, 0).unwrap().ch, 'c');
        assert_eq!(terminal.grid().cell(1, 0).unwrap().ch, 'd');
        assert_eq!(terminal.grid().cell(0, 1).unwrap().ch, 'e');
        assert_eq!(terminal.grid().cell(1, 1).unwrap().ch, 'f');
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
    fn osc_title_emits_title_event() {
        let mut terminal = TerminalCore::new(2, 1);
        let tick = terminal.process_pty_input(b"\x1b]2;c-term\x07");

        assert!(
            tick.events
                .contains(&CoreEvent::TitleChanged("c-term".into()))
        );
    }
}
