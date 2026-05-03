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
        self.blend_ellipse(
            x,
            y,
            CELL_WIDTH as f32 * 0.55 * scale,
            CELL_HEIGHT as f32 * 0.58 * scale,
            color,
            alpha,
        )
    }

    pub(crate) fn blend_cursor_glow(
        &mut self,
        x: f32,
        y: f32,
        scale: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        self.blend_ellipse(
            x,
            y,
            CELL_WIDTH as f32 * scale,
            CELL_HEIGHT as f32 * scale,
            color,
            alpha,
        )
    }

    pub(crate) fn blend_cursor_edge(
        &mut self,
        x: f32,
        y: f32,
        scale: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        self.blend_ellipse_ring(
            x,
            y,
            CELL_WIDTH as f32 * 0.68 * scale,
            CELL_HEIGHT as f32 * 0.72 * scale,
            color,
            alpha,
        )
    }

    pub(crate) fn blend_capsule(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        radius: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        self.blend_capsule_with(from, to, radius, color, alpha, |distance| {
            1.0 - smoothstep(0.58, 1.0, distance)
        });
    }

    pub(crate) fn blend_capsule_ring(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        radius: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        self.blend_capsule_with(from, to, radius, color, alpha, |distance| {
            smoothstep(0.48, 0.72, distance) * (1.0 - smoothstep(0.84, 1.0, distance))
        });
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

    fn blend_ellipse(
        &mut self,
        x: f32,
        y: f32,
        radius_x: f32,
        radius_y: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        if self.width_px == 0 || radius_x <= 0.0 || radius_y <= 0.0 {
            return;
        }
        let height_px = self.frame.len() / self.width_px / 4;
        let x_start = (x - radius_x).floor().max(0.0) as usize;
        let y_start = (y - radius_y).floor().max(0.0) as usize;
        let x_end = (x + radius_x).ceil().min(self.width_px as f32) as usize;
        let y_end = (y + radius_y).ceil().min(height_px as f32) as usize;

        for py in y_start..y_end {
            let dy = (py as f32 + 0.5 - y) / radius_y;
            for px in x_start..x_end {
                let dx = (px as f32 + 0.5 - x) / radius_x;
                let distance = dx * dx + dy * dy;
                if distance > 1.0 {
                    continue;
                }
                let local_alpha = (alpha as f32 * (1.0 - distance).powf(1.7)) as u8;
                if local_alpha == 0 {
                    continue;
                }
                blend_pixel(self.frame, self.width_px, px, py, color, local_alpha);
            }
        }
    }

    fn blend_ellipse_ring(
        &mut self,
        x: f32,
        y: f32,
        radius_x: f32,
        radius_y: f32,
        color: [u8; 3],
        alpha: u8,
    ) {
        if self.width_px == 0 || radius_x <= 0.0 || radius_y <= 0.0 {
            return;
        }
        let height_px = self.frame.len() / self.width_px / 4;
        let x_start = (x - radius_x).floor().max(0.0) as usize;
        let y_start = (y - radius_y).floor().max(0.0) as usize;
        let x_end = (x + radius_x).ceil().min(self.width_px as f32) as usize;
        let y_end = (y + radius_y).ceil().min(height_px as f32) as usize;

        for py in y_start..y_end {
            let dy = (py as f32 + 0.5 - y) / radius_y;
            for px in x_start..x_end {
                let dx = (px as f32 + 0.5 - x) / radius_x;
                let distance = (dx * dx + dy * dy).sqrt();
                if distance > 1.0 {
                    continue;
                }
                let inner = smoothstep(0.42, 0.74, distance);
                let outer = 1.0 - smoothstep(0.86, 1.0, distance);
                let local_alpha = (alpha as f32 * inner * outer) as u8;
                if local_alpha == 0 {
                    continue;
                }
                blend_pixel(self.frame, self.width_px, px, py, color, local_alpha);
            }
        }
    }

    fn blend_capsule_with(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
        radius: f32,
        color: [u8; 3],
        alpha: u8,
        opacity: impl Fn(f32) -> f32,
    ) {
        if self.width_px == 0 || radius <= 0.0 {
            return;
        }
        let height_px = self.frame.len() / self.width_px / 4;
        let x_start = (from.0.min(to.0) - radius).floor().max(0.0) as usize;
        let y_start = (from.1.min(to.1) - radius).floor().max(0.0) as usize;
        let x_end = (from.0.max(to.0) + radius).ceil().min(self.width_px as f32) as usize;
        let y_end = (from.1.max(to.1) + radius).ceil().min(height_px as f32) as usize;

        for py in y_start..y_end {
            for px in x_start..x_end {
                let distance =
                    distance_to_segment(px as f32 + 0.5, py as f32 + 0.5, from, to) / radius;
                if distance > 1.0 {
                    continue;
                }
                let local_alpha = (alpha as f32 * opacity(distance).clamp(0.0, 1.0)) as u8;
                if local_alpha == 0 {
                    continue;
                }
                blend_pixel(self.frame, self.width_px, px, py, color, local_alpha);
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
    last_generation: Option<u64>,
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
            last_generation: None,
            trails: Vec::new(),
        }
    }

    fn observe_cursor(&mut self, grid: &Grid, now: Instant) {
        let cursor = grid.cursor();
        let generation = grid.generation();
        let Some(last) = self.last_cursor else {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            return;
        };
        if self.is_large_redraw(grid, last, cursor) {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            self.trails.clear();
            return;
        }
        if last == cursor {
            self.last_generation = Some(generation);
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
        self.last_generation = Some(generation);
        self.last_change = now;
    }

    fn is_large_redraw(&self, grid: &Grid, last_cursor: Cursor, cursor: Cursor) -> bool {
        let Some(last_generation) = self.last_generation else {
            return false;
        };
        if grid.generation() < last_generation {
            return true;
        }
        if grid.generation() == last_generation && last_cursor != cursor {
            return true;
        }
        let cells = u64::from(grid.width()) * u64::from(grid.height());
        let threshold = (cells / 6).max(64);
        grid.generation().saturating_sub(last_generation) > threshold
    }

    fn draw_trails(&mut self, frame: &mut PluginFrame<'_>) {
        let decay = Duration::from_millis(self.config.decay_ms.max(1));
        self.trails
            .retain(|trail| frame.now.duration_since(trail.started) < decay);

        let edge = lift_color(self.config.color, 1.25, 28);
        let hot = lift_color(self.config.color, 1.8, 64);
        for trail in &self.trails {
            let age = frame.now.duration_since(trail.started);
            let raw = (age.as_secs_f32() / decay.as_secs_f32()).clamp(0.0, 1.0);
            let progress = ease_out_quart(raw);
            let fade = 1.0 - raw;
            let start = (progress - self.config.length).max(0.0);
            let tail = cursor_point(trail.from, trail.to, start);
            let head = cursor_point(trail.from, trail.to, progress);
            if progress > start {
                let dark_alpha = (74.0 * fade) as u8;
                let rim_alpha = (225.0 * fade) as u8;
                let core_alpha = (235.0 * fade) as u8;
                frame.blend_capsule(
                    tail,
                    head,
                    CELL_HEIGHT as f32 * 0.24,
                    [4, 8, 12],
                    dark_alpha,
                );
                frame.blend_capsule_ring(tail, head, CELL_HEIGHT as f32 * 0.22, edge, rim_alpha);
                frame.blend_capsule(
                    tail,
                    head,
                    CELL_HEIGHT as f32 * 0.12,
                    self.config.color,
                    core_alpha,
                );
                frame.blend_capsule(
                    tail,
                    head,
                    CELL_HEIGHT as f32 * 0.04,
                    hot,
                    (150.0 * fade) as u8,
                );
            }
            let (x, y) = cursor_point(trail.from, trail.to, progress);
            frame.blend_cursor_glow(x, y, 0.92, self.config.color, (24.0 * fade) as u8);
            frame.blend_cursor_edge(x, y, 0.98, [4, 8, 12], (72.0 * fade) as u8);
            frame.blend_cursor_edge(x, y, 0.84, edge, (180.0 * fade) as u8);
            frame.blend_cursor_at(x, y, 0.78, self.config.color, (250.0 * fade) as u8);
            frame.blend_cursor_at(x, y, 0.30, hot, (220.0 * fade) as u8);
        }
    }
}

impl Plugin for CursorTrail {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        self.observe_cursor(frame.grid, frame.now);
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

fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let value = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn distance_to_segment(px: f32, py: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return ((px - from.0).powi(2) + (py - from.1).powi(2)).sqrt();
    }
    let t = (((px - from.0) * dx + (py - from.1) * dy) / length_squared).clamp(0.0, 1.0);
    let nearest_x = from.0 + dx * t;
    let nearest_y = from.1 + dy * t;
    ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt()
}

fn lift_color(color: [u8; 3], scale: f32, offset: u8) -> [u8; 3] {
    color.map(|channel| ((f32::from(channel) * scale) as u16 + u16::from(offset)).min(255) as u8)
}

fn cell_width() -> usize {
    CELL_WIDTH as usize
}

fn blend_pixel(frame: &mut [u8], width_px: usize, x: usize, y: usize, color: [u8; 3], alpha: u8) {
    let index = (y * width_px + x) * 4;
    let alpha = u16::from(alpha);
    for (channel, target) in color.iter().zip(&mut frame[index..index + 3]) {
        *target =
            (((u16::from(*target) * (255 - alpha)) + (u16::from(*channel) * alpha)) / 255) as u8;
    }
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
            length: 1.0,
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
    fn cursor_trail_ignores_large_redraw_jump() {
        let config = CursorTrailConfig {
            hold_ms: 0,
            decay_ms: 300,
            threshold: 1,
            length: 1.0,
            color: [255, 0, 0],
        };
        let mut terminal = TerminalCore::new(20, 5);
        let mut plugin = CursorTrail::new(config);
        let start = Instant::now();
        let mut bytes = vec![10; 20 * CELL_WIDTH as usize * 5 * CELL_HEIGHT as usize * 4];

        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 20 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start,
        }));

        let _ = terminal.process_pty_input(
            b"aaaaaaaaaaaaaaaaaaa\x1b[2;1Hbbbbbbbbbbbbbbbbbbb\x1b[3;1Hccccccccccccccccccc\x1b[5;20H",
        );

        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 20 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start + Duration::from_millis(1),
        }));
    }

    #[test]
    fn cursor_trail_ignores_fresh_grid_cursor_jump() {
        let config = CursorTrailConfig {
            hold_ms: 0,
            decay_ms: 300,
            threshold: 1,
            length: 1.0,
            color: [255, 0, 0],
        };
        let mut terminal = TerminalCore::new(20, 5);
        let mut plugin = CursorTrail::new(config);
        let start = Instant::now();
        let mut bytes = vec![10; 20 * CELL_WIDTH as usize * 5 * CELL_HEIGHT as usize * 4];

        let _ = terminal.process_pty_input(b"\x1b[1;10H");
        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 20 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start,
        }));

        let _ = terminal.process_pty_input(b"\x1b[?1049h");
        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 20 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start + Duration::from_millis(1),
        }));
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

    #[test]
    fn distance_to_segment_clamps_to_segment_endpoints() {
        assert_eq!(distance_to_segment(5.0, 3.0, (0.0, 0.0), (10.0, 0.0)), 3.0);
        assert_eq!(distance_to_segment(13.0, 4.0, (0.0, 0.0), (10.0, 0.0)), 5.0);
    }
}
