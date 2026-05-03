use c_term_core::Grid;

use crate::window_backend::{CELL_HEIGHT, CELL_WIDTH};

pub(crate) struct PluginFrame<'a> {
    pub(crate) frame: &'a mut [u8],
    pub(crate) width_px: usize,
    pub(crate) grid: &'a Grid,
}

impl PluginFrame<'_> {
    pub(crate) fn blend_cell(&mut self, x: u16, y: u16, color: [u8; 3], alpha: u8) {
        self.blend_rect(
            usize::from(x) * CELL_WIDTH as usize,
            usize::from(y) * CELL_HEIGHT as usize,
            CELL_WIDTH as usize,
            CELL_HEIGHT as usize,
            color,
            alpha,
        );
    }

    pub(crate) fn blend_row(&mut self, y: u16, color: [u8; 3], alpha: u8) {
        self.blend_rect(
            0,
            usize::from(y) * CELL_HEIGHT as usize,
            usize::from(self.grid.width()) * CELL_WIDTH as usize,
            CELL_HEIGHT as usize,
            color,
            alpha,
        );
    }

    fn blend_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        color: [u8; 3],
        alpha: u8,
    ) {
        let alpha = u16::from(alpha);
        for py in y..y + height {
            for px in x..x + width {
                let index = (py * self.width_px + px) * 4;
                for (channel, target) in color.iter().zip(&mut self.frame[index..index + 3]) {
                    *target = (((u16::from(*target) * (255 - alpha))
                        + (u16::from(*channel) * alpha))
                        / 255) as u8;
                }
            }
        }
    }
}

pub(crate) trait Plugin {
    fn draw(&mut self, frame: &mut PluginFrame<'_>);
}

pub(crate) struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginHost {
    pub(crate) fn default_plugins() -> Self {
        Self {
            plugins: vec![Box::new(CursorLine)],
        }
    }

    pub(crate) fn draw(&mut self, frame: &mut PluginFrame<'_>) {
        for plugin in &mut self.plugins {
            plugin.draw(frame);
        }
    }
}

struct CursorLine;

impl Plugin for CursorLine {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) {
        let cursor = frame.grid.cursor();
        if cursor.visible {
            frame.blend_row(cursor.y, [32, 80, 96], 48);
            frame.blend_cell(cursor.x, cursor.y, [255, 205, 96], 64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;

    #[test]
    fn cursor_line_plugin_tints_frame() {
        let terminal = TerminalCore::new(2, 1);
        let mut host = PluginHost::default_plugins();
        let mut frame = vec![10; 2 * CELL_WIDTH as usize * CELL_HEIGHT as usize * 4];

        host.draw(&mut PluginFrame {
            frame: &mut frame,
            width_px: 2 * CELL_WIDTH as usize,
            grid: terminal.grid(),
        });

        assert_ne!(frame[0], 10);
        assert_eq!(frame[3], 10);
    }
}
