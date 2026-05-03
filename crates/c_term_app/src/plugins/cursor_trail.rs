use std::time::{Duration, Instant};

use c_term_core::{Color, Cursor, CursorShape, Grid};

use crate::theme::Theme;
use crate::window_backend::{CELL_HEIGHT, CELL_WIDTH};

use super::{Plugin, PluginFrame, Point};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CursorTrailConfig {
    pub(crate) hold_ms: u64,
    pub(crate) decay_ms: u64,
    pub(crate) fast_decay_ratio: f32,
    pub(crate) threshold: u16,
    pub(crate) color: CursorTrailColor,
}

impl Default for CursorTrailConfig {
    fn default() -> Self {
        Self {
            hold_ms: 20,
            decay_ms: 280,
            fast_decay_ratio: 0.38,
            threshold: 2,
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

    fn observe_cursor(&mut self, grid: &Grid, theme: Theme, now: Instant) {
        let cursor = grid.cursor();
        let generation = grid.generation();
        if grid.is_synchronized() {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            if !self.needs_render {
                self.snap_to_cursor(grid, theme, cursor, now);
            }
            return;
        }
        let Some(last) = self.last_cursor else {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            self.snap_to_cursor(grid, theme, cursor, now);
            return;
        };
        if self.is_large_redraw(grid, last, cursor) {
            self.last_cursor = Some(cursor);
            self.last_generation = Some(generation);
            self.last_change = now;
            self.snap_to_cursor(grid, theme, cursor, now);
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
            self.snap_to_cursor(grid, theme, cursor, now);
            return;
        }

        let stable = now.duration_since(self.last_change);
        if last.visible && cursor.visible && stable >= Duration::from_millis(self.config.hold_ms) {
            if cursor_distance(last, cursor) >= self.config.threshold || self.needs_render {
                self.set_target(grid, theme, cursor, now);
            } else {
                self.snap_to_cursor(grid, theme, cursor, now);
            }
        } else if !cursor.visible || !self.needs_render {
            self.snap_to_cursor(grid, theme, cursor, now);
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

    fn snap_to_cursor(&mut self, grid: &Grid, theme: Theme, cursor: Cursor, now: Instant) {
        if cursor.visible {
            let rect = CursorRect::from_cursor(cursor);
            self.target = Some(rect);
            self.corners = rect.corners();
            self.color = trail_color(self.config, theme, grid, cursor);
        } else {
            self.target = None;
        }
        self.last_frame = now;
        self.needs_render = false;
    }

    fn set_target(&mut self, grid: &Grid, theme: Theme, cursor: Cursor, now: Instant) {
        let target = CursorRect::from_cursor(cursor);
        let was_idle = !self.needs_render;
        if self.target.is_none() {
            self.corners = target.corners();
        }
        self.target = Some(target);
        self.color = trail_color(self.config, theme, grid, cursor);
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
        frame.overlay_quad(self.corners, [4, 8, 12], 82);
        frame.overlay_quad_ring(self.corners, edge, 235);
        frame.overlay_quad(self.corners, self.color, 205);
        frame.overlay_quad_ring(self.corners, hot, 132);
    }
}

impl Plugin for CursorTrail {
    fn draw(&mut self, frame: &mut PluginFrame<'_>) -> bool {
        self.observe_cursor(frame.grid, *frame.theme, frame.now);
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

fn trail_color(config: CursorTrailConfig, theme: Theme, grid: &Grid, cursor: Cursor) -> [u8; 3] {
    if let CursorTrailColor::Rgb(color) = config.color {
        return color;
    }
    grid.cell(cursor.x, cursor.y)
        .map(|cell| {
            let foreground = theme.color(cell.style.foreground);
            let background = theme.color(cell.style.background);
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

fn point_length(point: Point) -> f32 {
    (point.0 * point.0 + point.1 * point.1).sqrt()
}

#[cfg(test)]
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

#[cfg(test)]
fn distance_to_quad_edge(point: Point, corners: [Point; 4]) -> f32 {
    (0..4)
        .map(|i| distance_to_segment(point.0, point.1, corners[i], corners[(i + 1) % 4]))
        .fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;

    fn frame_for<'a>(
        terminal: &'a TerminalCore,
        theme: &'a Theme,
        now: Instant,
    ) -> PluginFrame<'a> {
        PluginFrame {
            grid: terminal.grid(),
            now,
            theme,
            overlays: Vec::new(),
            screen_opacity: 1.0,
        }
    }

    fn trail_config() -> CursorTrailConfig {
        CursorTrailConfig {
            hold_ms: 0,
            decay_ms: 300,
            fast_decay_ratio: 0.38,
            threshold: 1,
            color: CursorTrailColor::Rgb([255, 0, 0]),
        }
    }

    #[test]
    fn cursor_trail_requests_animation_after_large_stable_move() {
        let config = CursorTrailConfig {
            threshold: 2,
            ..trail_config()
        };
        let mut terminal = TerminalCore::new(4, 1);
        let mut plugin = CursorTrail::new(config);
        let theme = Theme::default();
        let start = Instant::now();

        assert!(!plugin.draw(&mut frame_for(&terminal, &theme, start)));
        let _ = terminal.process_pty_input(b"\x1b[4G");
        assert!(plugin.draw(&mut frame_for(
            &terminal,
            &theme,
            start + Duration::from_millis(1),
        )));
    }

    #[test]
    fn cursor_trail_ignores_large_redraw_jump() {
        let config = trail_config();
        let mut terminal = TerminalCore::new(20, 5);
        let mut plugin = CursorTrail::new(config);
        let theme = Theme::default();
        let start = Instant::now();

        assert!(!plugin.draw(&mut frame_for(&terminal, &theme, start)));

        let _ = terminal.process_pty_input(
            b"aaaaaaaaaaaaaaaaaaa\x1b[2;1Hbbbbbbbbbbbbbbbbbbb\x1b[3;1Hccccccccccccccccccc\x1b[5;20H",
        );

        assert!(!plugin.draw(&mut frame_for(
            &terminal,
            &theme,
            start + Duration::from_millis(1),
        )));
    }

    #[test]
    fn cursor_trail_ignores_fresh_grid_cursor_jump() {
        let config = trail_config();
        let mut terminal = TerminalCore::new(20, 5);
        let mut plugin = CursorTrail::new(config);
        let theme = Theme::default();
        let start = Instant::now();

        let _ = terminal.process_pty_input(b"\x1b[1;10H");
        assert!(!plugin.draw(&mut frame_for(&terminal, &theme, start)));

        let _ = terminal.process_pty_input(b"\x1b[?1049h");
        assert!(!plugin.draw(&mut frame_for(
            &terminal,
            &theme,
            start + Duration::from_millis(1),
        )));
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
            trail_color(
                config,
                Theme::default(),
                terminal.grid(),
                terminal.grid().cursor()
            ),
            [197, 15, 31]
        );
    }

    #[test]
    fn cursor_trail_ignores_synchronized_cursor_motion() {
        let config = trail_config();
        let mut terminal = TerminalCore::new(8, 1);
        let mut plugin = CursorTrail::new(config);
        let theme = Theme::default();
        let start = Instant::now();

        assert!(!plugin.draw(&mut frame_for(&terminal, &theme, start)));

        let _ = terminal.process_pty_input(b"\x1b[?2026h\x1b[8G");

        assert!(!plugin.draw(&mut frame_for(
            &terminal,
            &theme,
            start + Duration::from_millis(1),
        )));
    }

    #[test]
    fn cursor_trail_snaps_across_wrap_edges() {
        let config = trail_config();
        let mut terminal = TerminalCore::new(8, 2);
        let mut plugin = CursorTrail::new(config);
        let theme = Theme::default();
        let start = Instant::now();

        let _ = terminal.process_pty_input(b"\x1b[1;8H");
        assert!(!plugin.draw(&mut frame_for(&terminal, &theme, start)));

        let _ = terminal.process_pty_input(b"\x1b[2;1H");

        assert!(!plugin.draw(&mut frame_for(
            &terminal,
            &theme,
            start + Duration::from_millis(30),
        )));
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
            Theme::default(),
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
            Theme::default(),
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
