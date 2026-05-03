mod damage;
mod event;
mod grid;
mod parser;
mod terminal;

pub use damage::{DamageBatch, DamageRegion, DamageTracker, Generation};
pub use event::{CoreEvent, CoreEventKind, CursorPosition, KeyModifiers, KeyPress};
pub use grid::{Cell, Color, Cursor, Grid, Style};
pub use parser::{ParserAction, ParserAdapter, SimpleParser, StyleUpdate};
pub use terminal::{CoreTick, TerminalCore};
