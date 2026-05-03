use super::{Plugin, PluginFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorLineConfig {
    pub(crate) row_color: [u8; 3],
    pub(crate) row_alpha: u8,
    pub(crate) cell_color: [u8; 3],
    pub(crate) cell_alpha: u8,
}

impl Default for CursorLineConfig {
    fn default() -> Self {
        Self {
            row_color: [32, 80, 96],
            row_alpha: 48,
            cell_color: [255, 205, 96],
            cell_alpha: 64,
        }
    }
}

pub(crate) struct CursorLine {
    config: CursorLineConfig,
}

impl CursorLine {
    pub(crate) fn new(config: CursorLineConfig) -> Self {
        Self { config }
    }
}

impl Default for CursorLine {
    fn default() -> Self {
        Self::new(CursorLineConfig::default())
    }
}

impl Plugin for CursorLine {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        let cursor = frame.grid.cursor();
        if cursor.visible {
            frame.overlay_row(cursor.y, self.config.row_color, self.config.row_alpha);
            frame.overlay_cell(
                cursor.x,
                cursor.y,
                self.config.cell_color,
                self.config.cell_alpha,
            );
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use c_term_core::TerminalCore;

    use super::*;

    #[test]
    fn cursor_line_emits_row_and_cell_overlays() {
        let terminal = TerminalCore::new(4, 1);
        let mut plugin = CursorLine::default();
        let mut frame = PluginFrame {
            grid: terminal.grid(),
            now: Instant::now(),
            overlays: Vec::new(),
            screen_opacity: 1.0,
        };

        assert!(!plugin.draw(&mut frame));

        assert_eq!(frame.overlays.len(), 2);
    }
}
