use std::{env, fs, os::fd::AsRawFd, path::PathBuf};

use winit::keyboard::{Key, ModifiersState};

use crate::{
    runner::{FontConfig, TerminalMetrics},
    set_pty_winsize,
};

use super::{WindowBackend, buffer_height, buffer_width, grid_size, render_cache::RenderCache};

pub(super) const MIN_ZOOM_STEPS: i16 = -4;
pub(super) const MAX_ZOOM_STEPS: i16 = 8;

impl WindowBackend {
    pub(super) fn handle_zoom_key(&mut self, key: &Key) -> bool {
        match zoom_key_action(key, self.modifiers) {
            Some(ZoomAction::Adjust(delta)) => self.adjust_zoom(delta),
            Some(ZoomAction::Reset) => self.reset_zoom(),
            None => return false,
        }
        true
    }

    fn adjust_zoom(&mut self, delta: i16) {
        let next = normalize_zoom_steps(self.zoom_steps + delta);
        if next == self.zoom_steps {
            return;
        }
        self.zoom_steps = next;
        self.store_zoom();
        self.apply_zoom();
    }

    fn reset_zoom(&mut self) {
        if self.zoom_steps == self.default_zoom_steps {
            return;
        }
        self.zoom_steps = self.default_zoom_steps;
        self.store_zoom();
        self.apply_zoom();
    }

    fn store_zoom(&self) {
        if self.persist_zoom
            && let Err(error) = store_zoom_steps(self.zoom_steps)
        {
            eprintln!("termite: failed to persist zoom setting: {error}");
        }
    }

    fn apply_zoom(&mut self) {
        self.metrics = scaled_metrics(self.base_metrics, self.zoom_steps);
        self.font = scaled_font(&self.base_font, self.zoom_steps);
        self.render_cache = RenderCache::new(
            self.font.clone(),
            self.theme,
            self.metrics,
            self.text_render,
        );

        let Some(window) = &self.window else {
            return;
        };
        let (cols, rows) = grid_size(window.inner_size(), self.metrics);

        self.selection = None;
        self.scroll_offset = 0;
        if let Some(child) = &mut self.child
            && let Err(error) = set_pty_winsize(child.master.as_raw_fd(), cols, rows)
        {
            eprintln!("termite: failed to resize PTY after zoom: {error}");
        }
        if let Some(terminal) = &mut self.terminal {
            let tick = terminal.resize_reflow(cols, rows);
            self.apply_damage(&tick.damage);
        }
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.resize_texture(
                buffer_width(cols, self.metrics),
                buffer_height(rows, self.metrics),
                self.metrics,
            );
        }
        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.mark_dirty();
    }
}

pub(super) fn scaled_metrics(base: TerminalMetrics, zoom_steps: i16) -> TerminalMetrics {
    let step = i32::from(zoom_steps);
    TerminalMetrics {
        cell_width: (base.cell_width as i32 + step).max(6) as u32,
        cell_height: (base.cell_height as i32 + step * 2).max(10) as u32,
    }
}

pub(super) fn scaled_font(font: &FontConfig, zoom_steps: i16) -> FontConfig {
    match font {
        FontConfig::GlyphAtlas { paths, size } => FontConfig::GlyphAtlas {
            paths: paths.clone(),
            size: (*size + f32::from(zoom_steps)).clamp(8.0, 32.0),
        },
        FontConfig::Bitmap8x8 => FontConfig::Bitmap8x8,
    }
}

pub(super) fn normalize_zoom_steps(steps: i16) -> i16 {
    steps.clamp(MIN_ZOOM_STEPS, MAX_ZOOM_STEPS)
}

pub(super) fn load_zoom_steps() -> Option<i16> {
    let text = fs::read_to_string(zoom_state_path()).ok()?;
    parse_zoom_steps(&text)
}

fn store_zoom_steps(steps: i16) -> std::io::Result<()> {
    let path = zoom_state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, normalize_zoom_steps(steps).to_string())
}

pub(super) fn parse_zoom_steps(text: &str) -> Option<i16> {
    text.trim().parse::<i16>().ok().map(normalize_zoom_steps)
}

fn zoom_state_path() -> PathBuf {
    if let Some(state_home) = env::var_os("XDG_STATE_HOME")
        && !state_home.is_empty()
    {
        return PathBuf::from(state_home).join("termite").join("zoom");
    }
    if let Some(home) = env::var_os("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("termite")
            .join("zoom");
    }
    PathBuf::from(".termite-zoom")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ZoomAction {
    Adjust(i16),
    Reset,
}

pub(super) fn zoom_key_action(key: &Key, modifiers: ModifiersState) -> Option<ZoomAction> {
    if !modifiers.control_key() {
        return None;
    }

    match key.as_ref() {
        Key::Character("+") | Key::Character("=") => Some(ZoomAction::Adjust(1)),
        Key::Character("-") | Key::Character("_") => Some(ZoomAction::Adjust(-1)),
        Key::Character("0") => Some(ZoomAction::Reset),
        _ => None,
    }
}
