use crate::{Cell, Cursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreEventKind {
    KeyPress,
    CursorMoved,
    CellChanged,
    LineChanged,
    ViewportChanged,
    ModeChanged,
    SelectionChanged,
    TitleChanged,
    Bell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPosition {
    pub x: u16,
    pub y: u16,
}

impl From<Cursor> for CursorPosition {
    fn from(value: Cursor) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub logical_key: String,
    pub text: Option<String>,
    pub modifiers: KeyModifiers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    KeyPress(KeyPress),
    CursorMoved {
        old: CursorPosition,
        new: CursorPosition,
    },
    CellChanged {
        x: u16,
        y: u16,
        cell: Cell,
    },
    LineChanged {
        y: u16,
    },
    ViewportChanged,
    ModeChanged {
        name: &'static str,
        enabled: bool,
    },
    SelectionChanged,
    TitleChanged(String),
    Bell,
}

impl CoreEvent {
    pub fn kind(&self) -> CoreEventKind {
        match self {
            Self::KeyPress(_) => CoreEventKind::KeyPress,
            Self::CursorMoved { .. } => CoreEventKind::CursorMoved,
            Self::CellChanged { .. } => CoreEventKind::CellChanged,
            Self::LineChanged { .. } => CoreEventKind::LineChanged,
            Self::ViewportChanged => CoreEventKind::ViewportChanged,
            Self::ModeChanged { .. } => CoreEventKind::ModeChanged,
            Self::SelectionChanged => CoreEventKind::SelectionChanged,
            Self::TitleChanged(_) => CoreEventKind::TitleChanged,
            Self::Bell => CoreEventKind::Bell,
        }
    }
}
