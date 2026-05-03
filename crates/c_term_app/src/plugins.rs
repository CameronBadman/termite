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
            usize::from(x) * cell_width(),
            usize::from(y) * cell_height(),
            cell_width(),
            cell_height(),
            color,
            alpha,
        );
    }

    pub(crate) fn blend_cursor_at(
        &mut self,
        x: f32,
        y: f32,
        scale: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        let width = (CELL_WIDTH as f32 * scale).round().max(2.0) as usize;
        let height = (CELL_HEIGHT as f32 * scale).round().max(4.0) as usize;
        self.blend_rect(
            (x - width as f32 * 0.5).round().max(0.0) as usize,
            (y - height as f32 * 0.5).round().max(0.0) as usize,
            width,
            height,
            color,
            alpha,
        );
    }

    pub(crate) fn blend_cursor_glow(
        &mut self,
        x: f32,
        y: f32,
        scale: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        self.blend_rect(
            (x - CELL_WIDTH as f32 * scale * 0.7).round().max(0.0) as usize,
            (y - CELL_HEIGHT as f32 * scale * 0.7).round().max(0.0) as usize,
            (CELL_WIDTH as f32 * scale * 1.4).round().max(3.0) as usize,
            (CELL_HEIGHT as f32 * scale * 1.4).round().max(5.0) as usize,
            color,
            alpha,
        );
    }

    pub(crate) fn blend_row(&mut self, y: u16, color: [u8; 3], alpha: u8) {
        self.blend_rect(
            0,
            usize::from(y) * cell_height(),
            usize::from(self.grid.width()) * cell_width(),
            cell_height(),
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
            let progress =
                ease_out_cubic((age.as_secs_f32() / decay.as_secs_f32()).clamp(0.0, 1.0));
            let fade = 1.0 - (age.as_secs_f32() / decay.as_secs_f32()).clamp(0.0, 1.0);
            let start = (progress - self.config.length).max(0.0);
            for sample in 0..18 {
                let sample_progress = sample as f32 / 17.0;
                let t = start + (progress - start) * sample_progress;
                let (x, y) = cursor_point(trail.from, trail.to, t);
                let tail = sample_progress.powf(1.8);
                let alpha = (180.0 * fade * tail).clamp(0.0, 180.0) as u8;
                let scale = 0.65 + tail * 0.35;
                frame.blend_cursor_glow(x, y, scale * 1.25, self.config.color, alpha / 4);
                frame.blend_cursor_at(x, y, scale, self.config.color, alpha);
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

fn cursor_point(from: Cursor, to: Cursor, t: f32) -> (f32, f32) {
    let from_x = (f32::from(from.x) + 0.5) * CELL_WIDTH as f32;
    let from_y = (f32::from(from.y) + 0.5) * CELL_HEIGHT as f32;
    let to_x = (f32::from(to.x) + 0.5) * CELL_WIDTH as f32;
    let to_y = (f32::from(to.y) + 0.5) * CELL_HEIGHT as f32;
    (from_x + (to_x - from_x) * t, from_y + (to_y - from_y) * t)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
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
            length: 0.4,
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

    #[test]
    fn cursor_point_interpolates_in_pixel_space() {
        let from = Cursor {
            x: 0,
            y: 0,
            visible: true,
        };
        let to = Cursor {
            x: 2,
            y: 0,
            visible: true,
        };

        assert_eq!(cursor_point(from, to, 0.5), (12.0, 8.0));
    }
}
