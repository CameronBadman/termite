use std::time::{Duration, Instant};

use c_term_core::{Color, Cursor, CursorShape, Grid};

use crate::window_backend::{CELL_HEIGHT, CELL_WIDTH, rgb};

type Point = (f32, f32);

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

    pub(crate) fn blend_quad(&mut self, corners: [Point; 4], color: [u8; 3], alpha: u8) {
        self.blend_quad_with(corners, color, alpha, |edge_distance| {
            smoothstep(0.0, 1.25, edge_distance)
        });
    }

    pub(crate) fn blend_quad_ring(&mut self, corners: [Point; 4], color: [u8; 3], alpha: u8) {
        self.blend_quad_with(corners, color, alpha, |edge_distance| {
            1.0 - smoothstep(1.0, 3.0, edge_distance)
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

    fn blend_quad_with(
        &mut self,
        corners: [Point; 4],
        color: [u8; 3],
        alpha: u8,
        opacity: impl Fn(f32) -> f32,
    ) {
        if self.width_px == 0 {
            return;
        }
        let height_px = self.frame.len() / self.width_px / 4;
        let min_x = corners
            .iter()
            .map(|corner| corner.0)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_x = corners
            .iter()
            .map(|corner| corner.0)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(self.width_px as f32) as usize;
        let min_y = corners
            .iter()
            .map(|corner| corner.1)
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as usize;
        let max_y = corners
            .iter()
            .map(|corner| corner.1)
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(height_px as f32) as usize;

        for py in min_y..max_y {
            for px in min_x..max_x {
                let point = (px as f32 + 0.5, py as f32 + 0.5);
                if !point_in_quad(point, corners) {
                    continue;
                }
                let edge_distance = distance_to_quad_edge(point, corners);
                let local_alpha = (alpha as f32 * opacity(edge_distance).clamp(0.0, 1.0)) as u8;
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
            frame.blend_row(cursor.y, self.config.row_color, self.config.row_alpha);
            frame.blend_cell(
                cursor.x,
                cursor.y,
                self.config.cell_color,
                self.config.cell_alpha,
            );
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CursorTrailConfig {
    pub(crate) hold_ms: u64,
    pub(crate) decay_ms: u64,
    pub(crate) fast_decay_ratio: f32,
    pub(crate) threshold: u16,
    pub(crate) length: f32,
    pub(crate) color: CursorTrailColor,
}

impl Default for CursorTrailConfig {
    fn default() -> Self {
        Self {
            hold_ms: 20,
            decay_ms: 280,
            fast_decay_ratio: 0.38,
            threshold: 2,
            length: 1.0,
            color: CursorTrailColor::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CursorTrailColor {
    Auto,
    Rgb([u8; 3]),
}

pub(crate) struct CursorTrail {
    config: CursorTrailConfig,
    last_cursor: Option<Cursor>,
    last_change: Instant,
    last_generation: Option<u64>,
    target: Option<CursorRect>,
    corners: [Point; 4],
    last_frame: Instant,
    color: [u8; 3],
    needs_render: bool,
}

#[derive(Clone, Copy)]
struct CursorRect {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

impl CursorRect {
    fn from_cursor(cursor: Cursor) -> Self {
        let left = f32::from(cursor.x) * CELL_WIDTH as f32;
        let top = f32::from(cursor.y) * CELL_HEIGHT as f32;
        match cursor.shape {
            CursorShape::Block => Self {
                left,
                right: left + CELL_WIDTH as f32,
                top,
                bottom: top + CELL_HEIGHT as f32,
            },
            CursorShape::Beam => Self {
                left,
                right: left + (CELL_WIDTH as f32 * 0.25).max(1.0),
                top,
                bottom: top + CELL_HEIGHT as f32,
            },
            CursorShape::Underline => Self {
                left,
                right: left + CELL_WIDTH as f32,
                top: top + CELL_HEIGHT as f32 - (CELL_HEIGHT as f32 * 0.18).max(1.0),
                bottom: top + CELL_HEIGHT as f32,
            },
        }
    }

    fn corners(self) -> [Point; 4] {
        [
            (self.right, self.top),
            (self.right, self.bottom),
            (self.left, self.bottom),
            (self.left, self.top),
        ]
    }

    fn center(self) -> Point {
        (
            (self.left + self.right) * 0.5,
            (self.top + self.bottom) * 0.5,
        )
    }

    fn half_diagonal(self) -> f32 {
        ((self.right - self.left).powi(2) + (self.bottom - self.top).powi(2)).sqrt() * 0.5
    }
}

impl CursorTrail {
    pub(crate) fn new(config: CursorTrailConfig) -> Self {
        let now = Instant::now();
        Self {
            config,
            last_cursor: None,
            last_change: now,
            last_generation: None,
            target: None,
            corners: [(0.0, 0.0); 4],
            last_frame: now,
            color: match config.color {
                CursorTrailColor::Auto => [104, 247, 255],
                CursorTrailColor::Rgb(color) => color,
            },
            needs_render: false,
        }
    }

    fn observe_cursor(&mut self, grid: &Grid, now: Instant) {
        let cursor = grid.cursor();
        let generation = grid.generation();
        if grid.is_synchronized() {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            if !self.needs_render {
                self.snap_to_cursor(grid, cursor, now);
            }
            return;
        }
        let Some(last) = self.last_cursor else {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            self.snap_to_cursor(grid, cursor, now);
            return;
        };
        if self.is_large_redraw(grid, last, cursor) {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            self.snap_to_cursor(grid, cursor, now);
            return;
        }
        if last == cursor {
            self.last_generation = Some(generation);
            return;
        }

        if is_wrap_hop(grid, last, cursor) {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            self.snap_to_cursor(grid, cursor, now);
            return;
        }

        let stable = now.duration_since(self.last_change);
        if last.visible && cursor.visible && stable >= Duration::from_millis(self.config.hold_ms) {
            if cursor_distance(last, cursor) >= self.config.threshold || self.needs_render {
                self.set_target(grid, cursor, now);
            } else {
                self.snap_to_cursor(grid, cursor, now);
            }
        } else if !cursor.visible || !self.needs_render {
            self.snap_to_cursor(grid, cursor, now);
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

    fn snap_to_cursor(&mut self, grid: &Grid, cursor: Cursor, now: Instant) {
        if cursor.visible {
            let rect = CursorRect::from_cursor(cursor);
            self.target = Some(rect);
            self.corners = rect.corners();
            self.color = trail_color(self.config, grid, cursor);
        } else {
            self.target = None;
        }
        self.last_frame = now;
        self.needs_render = false;
    }

    fn set_target(&mut self, grid: &Grid, cursor: Cursor, now: Instant) {
        let target = CursorRect::from_cursor(cursor);
        let was_idle = !self.needs_render;
        if self.target.is_none() {
            self.corners = target.corners();
        }
        self.target = Some(target);
        self.color = trail_color(self.config, grid, cursor);
        if was_idle {
            self.last_frame = now;
        }
        self.needs_render = true;
    }

    fn update_corners(&mut self, now: Instant) {
        let Some(target) = self.target else {
            self.last_frame = now;
            self.needs_render = false;
            return;
        };
        if !self.needs_render {
            self.corners = target.corners();
            self.last_frame = now;
            return;
        }

        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        if dt <= 0.0 {
            return;
        }

        let target_corners = target.corners();
        let center = target.center();
        let half_diagonal = target.half_diagonal().max(0.001);
        let mut deltas = [(0.0, 0.0); 4];
        let mut dots = [0.0; 4];
        for i in 0..4 {
            let dx = target_corners[i].0 - self.corners[i].0;
            let dy = target_corners[i].1 - self.corners[i].1;
            deltas[i] = if dx.abs() < 0.001 && dy.abs() < 0.001 {
                (0.0, 0.0)
            } else {
                (dx, dy)
            };
            let length = point_length(deltas[i]);
            if length > 0.0 {
                dots[i] = (dx * (target_corners[i].0 - center.0)
                    + dy * (target_corners[i].1 - center.1))
                    / half_diagonal
                    / length;
            }
        }

        let (min_dot, max_dot) = dots
            .iter()
            .copied()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), dot| {
                (min.min(dot), max.max(dot))
            });
        let slow_decay = (self.config.decay_ms.max(1) as f32) / 1000.0;
        let fast_decay = (slow_decay * self.config.fast_decay_ratio).max(0.001);

        for i in 0..4 {
            if deltas[i] == (0.0, 0.0) {
                continue;
            }
            let decay = if (max_dot - min_dot).abs() < f32::EPSILON {
                slow_decay
            } else {
                slow_decay + (fast_decay - slow_decay) * (dots[i] - min_dot) / (max_dot - min_dot)
            };
            let step = exponential_step(dt, decay);
            self.corners[i].0 += deltas[i].0 * step;
            self.corners[i].1 += deltas[i].1 * step;
        }

        self.needs_render = target_corners
            .iter()
            .zip(self.corners)
            .any(|(target, corner)| {
                (target.0 - corner.0).abs() >= 0.5 || (target.1 - corner.1).abs() >= 0.5
            });
    }

    fn draw_trail(&self, frame: &mut PluginFrame<'_>) {
        if self.target.is_none() || !self.needs_render {
            return;
        }

        let edge = lift_color(self.color, 1.25, 28);
        let hot = lift_color(self.color, 1.8, 64);
        frame.blend_quad(self.corners, [4, 8, 12], 82);
        frame.blend_quad_ring(self.corners, edge, 235);
        frame.blend_quad(self.corners, self.color, 205);
        frame.blend_quad_ring(self.corners, hot, 132);
    }
}

impl Plugin for CursorTrail {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        self.observe_cursor(frame.grid, frame.now);
        self.update_corners(frame.now);
        self.draw_trail(frame);
        self.needs_render
    }
}

fn cursor_distance(a: Cursor, b: Cursor) -> u16 {
    a.x.abs_diff(b.x).max(a.y.abs_diff(b.y))
}

fn is_wrap_hop(grid: &Grid, from: Cursor, to: Cursor) -> bool {
    if grid.width() < 2 || !from.visible || !to.visible {
        return false;
    }
    let right_to_left = from.x >= grid.width().saturating_sub(2) && to.x <= 1;
    let left_to_right = from.x <= 1 && to.x >= grid.width().saturating_sub(2);
    let wrapped_down = right_to_left && to.y == from.y.saturating_add(1);
    let wrapped_up = left_to_right && from.y == to.y.saturating_add(1);
    wrapped_down || wrapped_up
}

fn trail_color(config: CursorTrailConfig, grid: &Grid, cursor: Cursor) -> [u8; 3] {
    if let CursorTrailColor::Rgb(color) = config.color {
        return color;
    }
    grid.cell(cursor.x, cursor.y)
        .map(|cell| {
            let foreground = rgb(cell.style.foreground, [220, 224, 232]);
            let background = rgb(cell.style.background, [16, 18, 24]);
            if matches!(cell.style.foreground, Color::DefaultForeground) {
                lift_color(background, 1.6, 32)
            } else {
                foreground
            }
        })
        .unwrap_or([104, 247, 255])
}

fn exponential_step(dt: f32, decay: f32) -> f32 {
    1.0 - 2.0_f32.powf(-10.0 * dt / decay.max(0.001))
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let value = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn point_length(point: Point) -> f32 {
    (point.0 * point.0 + point.1 * point.1).sqrt()
}

fn point_in_quad(point: Point, corners: [Point; 4]) -> bool {
    let mut sign = 0.0_f32;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let cross = (b.0 - a.0) * (point.1 - a.1) - (b.1 - a.1) * (point.0 - a.0);
        if cross.abs() < 0.001 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if sign * cross < 0.0 {
            return false;
        }
    }
    true
}

fn distance_to_quad_edge(point: Point, corners: [Point; 4]) -> f32 {
    (0..4)
        .map(|i| distance_to_segment(point.0, point.1, corners[i], corners[(i + 1) % 4]))
        .fold(f32::INFINITY, f32::min)
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

    fn trail_config() -> CursorTrailConfig {
        CursorTrailConfig {
            hold_ms: 0,
            decay_ms: 300,
            fast_decay_ratio: 0.38,
            threshold: 1,
            length: 1.0,
            color: CursorTrailColor::Rgb([255, 0, 0]),
        }
    }

    #[test]
    fn configured_plugins_tint_frame() {
        let terminal = TerminalCore::new(4, 1);
        let mut host = PluginHost::new();
        host.add(CursorLine::default());
        host.add(CursorTrail::new(CursorTrailConfig::default()));
        let mut bytes = vec![10; 4 * CELL_WIDTH as usize * CELL_HEIGHT as usize * 4];

        host.draw(&mut frame_for(&terminal, &mut bytes, Instant::now()));

        assert_ne!(bytes[0], 10);
        assert_eq!(bytes[3], 10);
    }

    #[test]
    fn cursor_trail_requests_animation_after_large_stable_move() {
        let config = CursorTrailConfig {
            threshold: 2,
            ..trail_config()
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
        let config = trail_config();
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
        let config = trail_config();
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
    fn cursor_rect_uses_cell_bounds() {
        let rect = CursorRect::from_cursor(Cursor {
            x: 2,
            y: 1,
            visible: true,
            shape: CursorShape::Block,
        });

        assert_eq!(
            rect.corners(),
            [(24.0, 16.0), (24.0, 32.0), (16.0, 32.0), (16.0, 16.0)]
        );
    }

    #[test]
    fn cursor_rect_tracks_cursor_shape() {
        let beam = CursorRect::from_cursor(Cursor {
            x: 2,
            y: 1,
            visible: true,
            shape: CursorShape::Beam,
        });
        let underline = CursorRect::from_cursor(Cursor {
            x: 2,
            y: 1,
            visible: true,
            shape: CursorShape::Underline,
        });

        assert!(beam.right - beam.left < CELL_WIDTH as f32);
        assert_eq!(beam.bottom - beam.top, CELL_HEIGHT as f32);
        assert_eq!(underline.right - underline.left, CELL_WIDTH as f32);
        assert!(underline.bottom - underline.top < CELL_HEIGHT as f32);
    }

    #[test]
    fn cursor_trail_auto_color_uses_cursor_cell_foreground() {
        let mut config = trail_config();
        config.color = CursorTrailColor::Auto;
        let mut terminal = TerminalCore::new(4, 1);
        let _ = terminal.process_pty_input(b"\x1b[31mA\x1b[1G");

        assert_eq!(
            trail_color(config, terminal.grid(), terminal.grid().cursor()),
            [197, 15, 31]
        );
    }

    #[test]
    fn cursor_trail_ignores_synchronized_cursor_motion() {
        let config = trail_config();
        let mut terminal = TerminalCore::new(8, 1);
        let mut plugin = CursorTrail::new(config);
        let start = Instant::now();
        let mut bytes = vec![10; 8 * CELL_WIDTH as usize * CELL_HEIGHT as usize * 4];

        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 8 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start,
        }));

        let _ = terminal.process_pty_input(b"\x1b[?2026h\x1b[8G");

        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 8 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start + Duration::from_millis(1),
        }));
    }

    #[test]
    fn cursor_trail_snaps_across_wrap_edges() {
        let config = trail_config();
        let mut terminal = TerminalCore::new(8, 2);
        let mut plugin = CursorTrail::new(config);
        let start = Instant::now();
        let mut bytes = vec![10; 8 * CELL_WIDTH as usize * 2 * CELL_HEIGHT as usize * 4];

        let _ = terminal.process_pty_input(b"\x1b[1;8H");
        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 8 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start,
        }));

        let _ = terminal.process_pty_input(b"\x1b[2;1H");

        assert!(!plugin.draw(&mut PluginFrame {
            frame: &mut bytes,
            width_px: 8 * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now: start + Duration::from_millis(30),
        }));
    }

    #[test]
    fn cursor_trail_step_moves_fast_then_settles() {
        assert_eq!(exponential_step(0.0, 0.3), 0.0);

        let first_step = exponential_step(0.01, 0.3);
        let second_step = exponential_step(0.01, 0.3) * (1.0 - first_step);
        assert!(first_step > second_step);
        assert!(exponential_step(0.3, 0.3) > 0.99);
    }

    #[test]
    fn cursor_trail_corners_chase_target() {
        let config = trail_config();
        let terminal = TerminalCore::new(4, 1);
        let mut plugin = CursorTrail::new(config);
        let start = Instant::now();
        plugin.snap_to_cursor(
            terminal.grid(),
            Cursor {
                x: 0,
                y: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            start,
        );
        plugin.set_target(
            terminal.grid(),
            Cursor {
                x: 3,
                y: 0,
                visible: true,
                shape: CursorShape::Block,
            },
            start,
        );
        let before = plugin.corners[0].0;

        plugin.update_corners(start + Duration::from_millis(16));

        assert!(plugin.corners[0].0 > before);
        assert!(
            plugin.corners[0].0
                < CursorRect::from_cursor(Cursor {
                    x: 3,
                    y: 0,
                    visible: true,
                    shape: CursorShape::Block,
                })
                .corners()[0]
                    .0
        );
        assert!(plugin.needs_render);
    }

    #[test]
    fn quad_geometry_checks_inside_points_and_edges() {
        let corners = [(8.0, 0.0), (8.0, 16.0), (0.0, 16.0), (0.0, 0.0)];

        assert!(point_in_quad((4.0, 8.0), corners));
        assert!(!point_in_quad((10.0, 8.0), corners));
        assert_eq!(distance_to_quad_edge((4.0, 8.0), corners), 4.0);
    }

    #[test]
    fn distance_to_segment_clamps_to_segment_endpoints() {
        assert_eq!(distance_to_segment(5.0, 3.0, (0.0, 0.0), (10.0, 0.0)), 3.0);
        assert_eq!(distance_to_segment(13.0, 4.0, (0.0, 0.0), (10.0, 0.0)), 5.0);
    }
}
