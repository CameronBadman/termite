mod damage;
mod grid;
mod parser;
mod terminal;

pub use damage::{DamageBatch, DamageRegion, DamageTracker, Generation};
pub use grid::{Cell, Color, Cursor, CursorShape, Grid, Style};
pub use parser::{
    EraseMode, MouseTracking, ParserAction, ParserAdapter, SimpleParser, StyleUpdate, TerminalMode,
};
pub use terminal::{CoreTick, MouseState, TerminalCore};
