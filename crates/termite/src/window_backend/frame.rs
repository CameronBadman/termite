use std::time::{Duration, Instant};

use termite_core::{CursorShape, DamageBatch, Grid};
use winit::event_loop::{ActiveEventLoop, ControlFlow};

use crate::{plugins::PluginFrame, runner::TerminalMetrics};

use super::{WindowBackend, perf::PerfFrameUpdate};

const ANIMATION_FRAME_MS: u64 = 8;
const FRAME_INTERVAL_MS: u64 = 8;
const DELAYED_RENDER_LOWER_US: u64 = 150;
const DELAYED_RENDER_UPPER_NS: u64 = 4_000_000;

impl WindowBackend {
    pub(super) fn mark_dirty(&mut self) {
        self.request_frame(Instant::now());
    }

    pub(super) fn request_frame(&mut self, now: Instant) {
        if self.redraw_pending {
            return;
        }

        if let Some(last_render) = self.last_render {
            let next_frame = last_render + Duration::from_millis(FRAME_INTERVAL_MS);
            if now < next_frame {
                self.frame_deadline = Some(
                    self.frame_deadline
                        .map_or(next_frame, |deadline| deadline.min(next_frame)),
                );
                return;
            }
        }

        self.request_redraw_now();
    }

    pub(super) fn request_redraw_now(&mut self) {
        if self.redraw_pending {
            return;
        }
        if let Some(window) = &self.window {
            self.redraw_pending = true;
            self.frame_deadline = None;
            window.request_redraw();
        }
    }

    pub(super) fn schedule_animation(&mut self, now: Instant) {
        self.animation_deadline = Some(now + Duration::from_millis(ANIMATION_FRAME_MS));
    }

    pub(super) fn schedule_delayed_render(&mut self, now: Instant) {
        self.render_lower_deadline = Some(now + Duration::from_micros(DELAYED_RENDER_LOWER_US));
        if self.render_upper_deadline.is_none() {
            self.render_upper_deadline = Some(now + Duration::from_nanos(DELAYED_RENDER_UPPER_NS));
        }
    }

    pub(super) fn disarm_delayed_render(&mut self) {
        self.render_lower_deadline = None;
        self.render_upper_deadline = None;
    }

    pub(super) fn apply_damage(&mut self, damage: &DamageBatch) {
        if self.scroll_offset == 0 {
            self.render_cache.apply_damage(damage, self.rows);
        } else {
            self.render_cache.invalidate();
        }
    }

    pub(super) fn render(&mut self) {
        self.redraw_pending = false;
        self.frame_deadline = None;
        let render_started = Instant::now();
        self.disarm_delayed_render();
        let Some(terminal) = &self.terminal else {
            return;
        };

        let viewing_history = self.scroll_offset > 0 && !terminal.is_alternate_screen();
        let cache_started = Instant::now();
        if viewing_history {
            self.render_cache
                .update_scrollback(terminal, self.scroll_offset);
        } else {
            self.render_cache.update(terminal.grid());
        }
        let texture_update = self.render_cache.take_texture_update(self.rows);
        let cache_elapsed = cache_started.elapsed();

        let Some(renderer) = &mut self.renderer else {
            return;
        };

        let plugin_started = Instant::now();
        let (plugin_active, mut overlays, screen_opacity) = if !viewing_history {
            let mut plugin_frame = PluginFrame {
                grid: terminal.grid(),
                now: render_started,
                theme: &self.theme,
                metrics: self.metrics,
                overlays: Vec::new(),
                screen_opacity: 1.0,
            };
            let active = self.plugins.draw(&mut plugin_frame);
            (active, plugin_frame.overlays, plugin_frame.screen_opacity)
        } else {
            (false, Vec::new(), 1.0)
        };
        if let Some(selection) = self.selection {
            overlays.extend(selection.overlays(self.cols, self.theme.ansi[4], self.metrics));
        }
        let plugin_elapsed = plugin_started.elapsed();
        let cursor = if viewing_history {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            cursor_uniform(terminal.grid(), self.metrics)
        };
        let upload_full = texture_update.full;
        let upload_row_bands = texture_update.rows.len();
        let upload_scrolls = texture_update.scrolls.len();
        let gpu_started = Instant::now();
        if let Err(error) = renderer.render(
            self.render_cache.frame.as_slice(),
            &texture_update,
            cursor,
            &overlays,
            screen_opacity,
        ) {
            eprintln!("termite: GPU render failed: {error}");
        }
        let gpu_elapsed = gpu_started.elapsed();
        let render_finished = Instant::now();
        self.perf.record_frame(
            cache_elapsed,
            plugin_elapsed,
            gpu_elapsed,
            render_finished.duration_since(render_started),
            PerfFrameUpdate {
                upload_full,
                upload_row_bands,
                upload_scrolls,
                overlays: overlays.len(),
            },
        );
        self.last_render = Some(render_finished);
        if plugin_active {
            self.schedule_animation(render_started);
        } else {
            self.animation_deadline = None;
        }
    }

    pub(super) fn timers_if_due(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();

        if self
            .app_sync_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            self.app_sync_deadline = None;
            if let Some(terminal) = &mut self.terminal {
                terminal.disable_synchronized_update();
            }
            self.render_cache.invalidate();
            self.request_frame(now);
        }

        let render_due = self
            .render_lower_deadline
            .is_some_and(|deadline| deadline <= now)
            || self
                .render_upper_deadline
                .is_some_and(|deadline| deadline <= now);
        if render_due {
            self.disarm_delayed_render();
            self.request_frame(now);
        }

        if self
            .animation_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            self.animation_deadline = None;
            self.request_frame(now);
        }

        if self.frame_deadline.is_some_and(|deadline| deadline <= now) {
            self.request_redraw_now();
        }

        if let Some(deadline) = self.next_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [
            self.animation_deadline,
            self.render_lower_deadline,
            self.render_upper_deadline,
            self.app_sync_deadline,
            self.frame_deadline,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

pub(super) fn cursor_uniform(grid: &Grid, metrics: TerminalMetrics) -> [f32; 4] {
    let cursor = grid.cursor();
    if !cursor.visible {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let (x_start, y_start, cursor_width, cursor_height) = match cursor.shape {
        CursorShape::Block => (
            0,
            0,
            metrics.cell_width as usize,
            metrics.cell_height as usize,
        ),
        CursorShape::Beam => (
            0,
            0,
            (metrics.cell_width as usize / 4).max(1),
            metrics.cell_height as usize,
        ),
        CursorShape::Underline => (
            0,
            metrics.cell_height as usize - (metrics.cell_height as usize / 5).max(1),
            metrics.cell_width as usize,
            (metrics.cell_height as usize / 5).max(1),
        ),
    };
    [
        (usize::from(cursor.x) * metrics.cell_width as usize + x_start) as f32,
        (usize::from(cursor.y) * metrics.cell_height as usize + y_start) as f32,
        cursor_width as f32,
        cursor_height as f32,
    ]
}
