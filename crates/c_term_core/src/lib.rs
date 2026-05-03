mod damage;
mod grid;
mod parser;
mod terminal;

pub use damage::{DamageBatch, DamageRegion, DamageTracker, Generation};
pub use grid::{Cell, Color, Cursor, Grid, Style};
pub use parser::{EraseMode, ParserAction, ParserAdapter, SimpleParser, StyleUpdate, TerminalMode};
pub use terminal::{CoreTick, TerminalCore};
