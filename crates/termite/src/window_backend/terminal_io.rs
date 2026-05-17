use std::{
    io::Write,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use termite_core::ClipboardStore;
use winit::dpi::PhysicalSize;
use wl_clipboard_rs::copy as wl_copy;

use crate::set_pty_winsize;

use super::{WindowBackend, buffer_height, buffer_width, grid_size};

const APP_SYNC_TIMEOUT_MS: u64 = 1_000;

impl WindowBackend {
    fn handle_clipboard_store(store: ClipboardStore) {
        let Ok(bytes) = STANDARD.decode(store.base64) else {
            return;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return;
        };
        if text.is_empty() {
            return;
        }

        let clipboard = match store.clipboard {
            b'p' => wl_copy::ClipboardType::Primary,
            _ => wl_copy::ClipboardType::Regular,
        };
        let source = wl_copy::Source::Bytes(text.into_bytes().into_boxed_slice());
        let mut options = wl_copy::Options::new();
        options.clipboard(clipboard);
        if let Err(error) = options.copy(source, wl_copy::MimeType::Text) {
            eprintln!("termite: failed to store OSC 52 clipboard text: {error}");
        }
    }

    pub(super) fn handle_pty_bytes(&mut self, bytes: Vec<u8>) {
        let input_len = bytes.len();
        let started = Instant::now();
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        let tick = terminal.process_pty_input(&bytes);
        let core_profile = terminal.drain_profile();
        for store in tick.clipboard {
            Self::handle_clipboard_store(store);
        }
        let synchronized = terminal.grid().is_synchronized();
        if terminal.is_alternate_screen() && self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.render_cache.invalidate();
        }
        let max_scroll_offset = terminal.scrollback_len();
        if self.scroll_offset > max_scroll_offset {
            self.scroll_offset = max_scroll_offset;
            self.render_cache.invalidate();
        }
        let output = tick.output;
        let damage_regions = tick.damage.regions.len();
        self.apply_damage(&tick.damage);
        self.perf
            .record_pty(input_len, damage_regions, started.elapsed(), core_profile);
        let now = Instant::now();
        if synchronized {
            self.disarm_delayed_render();
            self.app_sync_deadline = Some(now + Duration::from_millis(APP_SYNC_TIMEOUT_MS));
        } else {
            self.app_sync_deadline = None;
            self.schedule_delayed_render(now);
        }
        if !output.is_empty()
            && let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&output)
        {
            eprintln!("termite: failed to write terminal response to PTY: {error}");
        }
    }

    pub(super) fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };

        let width = size.width.max(1);
        let height = size.height.max(1);
        renderer.resize_surface(width, height);

        let (cols, rows) = grid_size(size, self.metrics);
        if cols == self.cols && rows == self.rows {
            self.mark_dirty();
            return;
        }

        self.selection = None;
        if let Some(child) = &mut self.child
            && let Err(error) = set_pty_winsize(child.master.as_raw_fd(), cols, rows)
        {
            eprintln!("termite: failed to resize PTY: {error}");
        }

        if let Some(terminal) = &mut self.terminal {
            let tick = terminal.resize(cols, rows);
            self.apply_damage(&tick.damage);
        }

        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.resize_texture(
            buffer_width(cols, self.metrics),
            buffer_height(rows, self.metrics),
            self.metrics,
        );

        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.mark_dirty();
    }
}
