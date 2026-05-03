use std::time::Instant;

use c_term_core::Grid;

use crate::window_backend::{CELL_HEIGHT, CELL_WIDTH};

type Point = (f32, f32);

mod cursor_line;
mod cursor_trail;
mod screen_tint;
pub(crate) use cursor_line::{CursorLine, CursorLineConfig};
pub(crate) use cursor_trail::{CursorTrail, CursorTrailColor, CursorTrailConfig};
pub(crate) use screen_tint::{ScreenTint, ScreenTintConfig};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum OverlayKind {
    Rect,
    Quad,
    QuadRing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OverlayCommand {
    pub(crate) kind: OverlayKind,
    pub(crate) color: [u8; 3],
    pub(crate) alpha: u8,
    pub(crate) corners: [Point; 4],
}

pub(crate) struct PluginFrame<'a> {
    pub(crate) grid: &'a Grid,
    pub(crate) now: Instant,
    pub(crate) overlays: Vec<OverlayCommand>,
}

impl PluginFrame<'_> {
    pub(crate) fn overlay_cell(&mut self, x: u16, y: u16, color: [u8; 3], alpha: u8) {
        self.push_rect(
            usize::from(x) * cell_width(),
            usize::from(y) * cell_height(),
            cell_width(),
            cell_height(),
            color,
            alpha,
        );
    }

    pub(crate) fn overlay_quad(&mut self, corners: [Point; 4], color: [u8; 3], alpha: u8) {
        self.overlays.push(OverlayCommand {
            kind: OverlayKind::Quad,
            color,
            alpha,
            corners,
        });
    }

    pub(crate) fn overlay_quad_ring(&mut self, corners: [Point; 4], color: [u8; 3], alpha: u8) {
        self.overlays.push(OverlayCommand {
            kind: OverlayKind::QuadRing,
            color,
            alpha,
            corners,
        });
    }

    pub(crate) fn overlay_row(&mut self, y: u16, color: [u8; 3], alpha: u8) {
        self.push_rect(
            0,
            usize::from(y) * cell_height(),
            usize::from(self.grid.width()) * cell_width(),
            cell_height(),
            color,
            alpha,
        );
    }

    pub(crate) fn overlay_screen(&mut self, color: [u8; 3], alpha: u8) {
        self.push_rect(
            0,
            0,
            usize::from(self.grid.width()) * cell_width(),
            usize::from(self.grid.height()) * cell_height(),
            color,
            alpha,
        );
    }

    fn push_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        color: [u8; 3],
        alpha: u8,
    ) {
        let left = x as f32;
        let top = y as f32;
        let right = (x + width) as f32;
        let bottom = (y + height) as f32;
        self.overlays.push(OverlayCommand {
            kind: OverlayKind::Rect,
            color,
            alpha,
            corners: [(right, top), (right, bottom), (left, bottom), (left, top)],
        });
    }
}

pub(crate) trait Plugin {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool;
}

pub(crate) struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginHost {
    pub(crate) fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, plugin: impl Plugin + 'static) {
        self.plugins.push(Box::new(plugin));
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.plugins.len()
    }

    pub(crate) fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        let mut active = false;
        for plugin in &mut self.plugins {
            active |= plugin.draw(frame);
        }
        active
    }
}

fn cell_width() -> usize {
    CELL_WIDTH as usize
}

fn cell_height() -> usize {
    CELL_HEIGHT as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;

    fn frame_for(terminal: &TerminalCore, now: Instant) -> PluginFrame<'_> {
        PluginFrame {
            grid: terminal.grid(),
            now,
            overlays: Vec::new(),
        }
    }

    #[test]
    fn configured_plugins_emit_overlays() {
        let terminal = TerminalCore::new(4, 1);
        let mut host = PluginHost::new();
        host.add(ScreenTint::default());
        host.add(CursorLine::default());
        host.add(CursorTrail::new(CursorTrailConfig::default()));
        let mut frame = frame_for(&terminal, Instant::now());

        host.draw(&mut frame);

        assert!(!frame.overlays.is_empty());
    }
}
