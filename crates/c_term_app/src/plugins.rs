use std::time::{Duration, Instant};

use c_term_core::{Cursor, Grid};

use crate::{
    config::{AppConfig, CursorTrailConfig},
    window_backend::{CELL_HEIGHT, CELL_WIDTH},
};

pub(crate) struct PluginFrame<'a> {
    pub(crate) frame: &'a mut [u8],
    pub(crate) width_px: usize,
    pub(crate) grid: &'a Grid,
    pub(crate) now: Instant,
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
        if self.width_px == 0 {
            return;
        }
        let height_px = self.frame.len() / self.width_px / 4;
        let x_end = (x + width).min(self.width_px);
        let y_end = (y + height).min(height_px);
        let alpha = u16::from(alpha);
        for py in y..y_end {
            for px in x..x_end {
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
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool;
}

pub(crate) struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
}

impl PluginHost {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();
        for name in &config.plugins {
            match name.as_str() {
                "cursor_line" => plugins.push(Box::new(CursorLine)),
                "cursor_trail" => plugins.push(Box::new(CursorTrail::new(config.cursor_trail))),
                unknown => eprintln!("c-term: unknown plugin `{unknown}`"),
            }
        }
        Self { plugins }
    }

    pub(crate) fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        let mut active = false;
        for plugin in &mut self.plugins {
            active |= plugin.draw(frame);
        }
        active
    }
}

struct CursorLine;

impl Plugin for CursorLine {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        let cursor = frame.grid.cursor();
        if cursor.visible {
            frame.blend_row(cursor.y, [32, 80, 96], 48);
            frame.blend_cell(cursor.x, cursor.y, [255, 205, 96], 64);
        }
        false
    }
}

struct CursorTrail {
    config: CursorTrailConfig,
    last_cursor: Option<Cursor>,
    last_change: Instant,
    trails: Vec<Trail>,
}

struct Trail {
    from: Cursor,
    to: Cursor,
    started: Instant,
}

impl CursorTrail {
    fn new(config: CursorTrailConfig) -> Self {
        Self {
            config,
            last_cursor: None,
            last_change: Instant::now(),
            trails: Vec::new(),
        }
    }

    fn observe_cursor(&mut self, cursor: Cursor, now: Instant) {
        let Some(last) = self.last_cursor else {
            self.last_cursor = Some(cursor);
            self.last_change = now;
            return;
        };
        if last == cursor {
            return;
        }

        let stable = now.duration_since(self.last_change);
        if last.visible
            && cursor.visible
            && stable >= Duration::from_millis(self.config.hold_ms)
            && cursor_distance(last, cursor) >= self.config.threshold
        {
            self.trails.push(Trail {
                from: last,
                to: cursor,
                started: now,
            });
        }
        self.last_cursor = Some(cursor);
        self.last_change = now;
    }

    fn draw_trails(&mut self, frame: &mut PluginFrame<'_>) {
        let decay = Duration::from_millis(self.config.decay_ms.max(1));
        self.trails
            .retain(|trail| frame.now.duration_since(trail.started) < decay);

        for trail in &self.trails {
            let age = frame.now.duration_since(trail.started);
            let life = 1.0 - (age.as_secs_f32() / decay.as_secs_f32()).clamp(0.0, 1.0);
            for (step, (x, y)) in trail_cells(trail.from, trail.to).enumerate() {
                let alpha = (150.0 * life * (1.0 - step as f32 * 0.08).max(0.2)) as u8;
                frame.blend_cell(x, y, self.config.color, alpha);
            }
        }
    }
}

impl Plugin for CursorTrail {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        self.observe_cursor(frame.grid.cursor(), frame.now);
        self.draw_trails(frame);
        !self.trails.is_empty()
    }
}

fn cursor_distance(a: Cursor, b: Cursor) -> u16 {
    a.x.abs_diff(b.x).max(a.y.abs_diff(b.y))
}

fn trail_cells(from: Cursor, to: Cursor) -> impl Iterator<Item = (u16, u16)> {
    let dx = i32::from(to.x) - i32::from(from.x);
    let dy = i32::from(to.y) - i32::from(from.y);
    let steps = dx.abs().max(dy.abs()).max(1);
    (0..steps).map(move |index| {
        let x = i32::from(from.x) + dx * index / steps;
        let y = i32::from(from.y) + dy * index / steps;
        (x as u16, y as u16)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;

    fn frame_for<'a>(
        terminal: &'a TerminalCore,
        bytes: &'a mut [u8],
        now: Instant,
    ) -> PluginFrame<'a> {
        PluginFrame {
            frame: bytes,
            width_px: 4 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now,
        }
    }

    #[test]
    fn configured_plugins_tint_frame() {
        let config = AppConfig::default();
        let terminal = TerminalCore::new(4, 1);
        let mut host = PluginHost::from_config(&config);
        let mut bytes = vec![10; 4 * CELL_WIDTH as usize * CELL_HEIGHT as usize * 4];

        host.draw(&mut frame_for(&terminal, &mut bytes, Instant::now()));

        assert_ne!(bytes[0], 10);
        assert_eq!(bytes[3], 10);
    }

    #[test]
    fn cursor_trail_requests_animation_after_large_stable_move() {
        let config = CursorTrailConfig {
            hold_ms: 0,
            decay_ms: 300,
            threshold: 2,
            color: [255, 0, 0],
        };
        let mut terminal = TerminalCore::new(4, 1);
        let mut plugin = CursorTrail::new(config);
        let start = Instant::now();
        let mut bytes = vec![10; 4 * CELL_WIDTH as usize * CELL_HEIGHT as usize * 4];

        assert!(!plugin.draw(&mut frame_for(&terminal, &mut bytes, start)));
        let _ = terminal.process_pty_input(b"\x1b[4G");
        assert!(plugin.draw(&mut frame_for(
            &terminal,
            &mut bytes,
            start + Duration::from_millis(1),
        )));
    }
}
