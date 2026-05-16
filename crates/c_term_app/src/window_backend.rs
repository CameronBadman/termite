use std::{
    env,
    error::Error,
    fs,
    io::{Read, Write},
    os::fd::AsRawFd,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use c_term_core::{
    ClipboardStore, CursorShape, DamageBatch, Grid, MouseState, MouseTracking, TerminalCore,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};
use wl_clipboard_rs::{copy as wl_copy, paste as wl_paste};

use crate::{PtyChild, set_pty_winsize, spawn_shell};
use crate::{
    plugins::{PluginFrame, PluginHost},
    runner::{FontConfig, Runner, TerminalMetrics, TextRenderConfig, ZoomConfig},
    theme::Theme,
};

mod gpu;
mod input;
mod pty_events;
mod render_cache;
mod selection;
mod text;

use gpu::GpuRenderer;
use input::{
    active_mouse_button, encode_mouse_event, encode_window_key, mouse_button_code, mouse_cell,
    shortcut_key, wheel_codes, wheel_scroll_lines,
};
use pty_events::{UserEvent, spawn_pty_reader};
use render_cache::RenderCache;
use selection::Selection;

const ANIMATION_FRAME_MS: u64 = 8;
const FRAME_INTERVAL_MS: u64 = 8;
const DELAYED_RENDER_LOWER_US: u64 = 150;
const DELAYED_RENDER_UPPER_NS: u64 = 4_000_000;
const APP_SYNC_TIMEOUT_MS: u64 = 1_000;
const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 540;
const MIN_ZOOM_STEPS: i16 = -4;
const MAX_ZOOM_STEPS: i16 = 8;

pub(crate) fn run(runner: Runner) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let (shell, plugins, font, metrics, theme, zoom, text_render) = runner.into_parts();
    let mut state = WindowBackend::new(
        shell,
        event_loop.create_proxy(),
        plugins,
        font,
        metrics,
        theme,
        zoom,
        text_render,
    );
    event_loop.run_app(&mut state)?;
    Ok(())
}

struct WindowBackend {
    shell: String,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<GpuRenderer>,
    terminal: Option<TerminalCore>,
    plugins: PluginHost,
    child: Option<PtyChild>,
    modifiers: ModifiersState,
    cols: u16,
    rows: u16,
    font: FontConfig,
    base_font: FontConfig,
    metrics: TerminalMetrics,
    base_metrics: TerminalMetrics,
    zoom_steps: i16,
    default_zoom_steps: i16,
    persist_zoom: bool,
    text_render: TextRenderConfig,
    theme: Theme,
    render_cache: RenderCache,
    selection: Option<Selection>,
    mouse_buttons: u8,
    mouse_position: Option<(u16, u16)>,
    animation_deadline: Option<Instant>,
    render_lower_deadline: Option<Instant>,
    render_upper_deadline: Option<Instant>,
    app_sync_deadline: Option<Instant>,
    frame_deadline: Option<Instant>,
    last_render: Option<Instant>,
    redraw_pending: bool,
    scroll_offset: usize,
    perf: PerfStats,
}

impl WindowBackend {
    #[allow(clippy::too_many_arguments)]
    fn new(
        shell: String,
        proxy: EventLoopProxy<UserEvent>,
        plugins: PluginHost,
        font: FontConfig,
        metrics: TerminalMetrics,
        theme: Theme,
        zoom: ZoomConfig,
        text_render: TextRenderConfig,
    ) -> Self {
        let base_metrics = metrics;
        let default_zoom_steps = normalize_zoom_steps(zoom.default_steps);
        let zoom_steps = if zoom.persist {
            load_zoom_steps().unwrap_or(default_zoom_steps)
        } else {
            default_zoom_steps
        };
        let metrics = scaled_metrics(base_metrics, zoom_steps);
        let scaled_font = scaled_font(&font, zoom_steps);
        Self {
            shell,
            proxy,
            window: None,
            renderer: None,
            terminal: None,
            plugins,
            child: None,
            modifiers: ModifiersState::empty(),
            cols: 1,
            rows: 1,
            font: scaled_font.clone(),
            base_font: font.clone(),
            metrics,
            base_metrics,
            zoom_steps,
            default_zoom_steps,
            persist_zoom: zoom.persist,
            text_render,
            theme,
            render_cache: RenderCache::new(scaled_font, theme, metrics, text_render),
            selection: None,
            mouse_buttons: 0,
            mouse_position: None,
            animation_deadline: None,
            render_lower_deadline: None,
            render_upper_deadline: None,
            app_sync_deadline: None,
            frame_deadline: None,
            last_render: None,
            redraw_pending: false,
            scroll_offset: 0,
            perf: PerfStats::from_env(),
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        if self.window.is_some() {
            return Ok(());
        }

        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("c-term")
                    .with_transparent(true)
                    .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
                    .with_min_inner_size(LogicalSize::new(320, 200)),
            )?,
        );

        let size = window.inner_size();
        let (cols, rows) = grid_size(size, self.metrics);
        let renderer = GpuRenderer::new(
            window.clone(),
            size,
            buffer_width(cols, self.metrics),
            buffer_height(rows, self.metrics),
            self.metrics,
            self.theme.background,
            self.theme.cursor,
        )?;
        let mut child = spawn_shell(&self.shell, cols, rows)?;
        spawn_pty_reader(&mut child, self.proxy.clone())?;

        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.terminal = Some(TerminalCore::new(cols, rows));
        self.renderer = Some(renderer);
        self.child = Some(child);
        self.window = Some(window);
        self.schedule_delayed_render(Instant::now());
        Ok(())
    }

    fn mark_dirty(&mut self) {
        self.request_frame(Instant::now());
    }

    fn request_frame(&mut self, now: Instant) {
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

    fn request_redraw_now(&mut self) {
        if self.redraw_pending {
            return;
        }
        if let Some(window) = &self.window {
            self.redraw_pending = true;
            self.frame_deadline = None;
            window.request_redraw();
        }
    }

    fn schedule_animation(&mut self, now: Instant) {
        self.animation_deadline = Some(now + Duration::from_millis(ANIMATION_FRAME_MS));
    }

    fn schedule_delayed_render(&mut self, now: Instant) {
        self.render_lower_deadline = Some(now + Duration::from_micros(DELAYED_RENDER_LOWER_US));
        if self.render_upper_deadline.is_none() {
            self.render_upper_deadline = Some(now + Duration::from_nanos(DELAYED_RENDER_UPPER_NS));
        }
    }

    fn disarm_delayed_render(&mut self) {
        self.render_lower_deadline = None;
        self.render_upper_deadline = None;
    }

    fn apply_damage(&mut self, damage: &DamageBatch) {
        if self.scroll_offset == 0 {
            self.render_cache.apply_damage(damage, self.rows);
        } else {
            self.render_cache.invalidate();
        }
    }

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
            eprintln!("c-term: failed to store OSC 52 clipboard text: {error}");
        }
    }

    fn handle_pty_bytes(&mut self, bytes: Vec<u8>) {
        let input_len = bytes.len();
        let started = Instant::now();
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        let tick = terminal.process_pty_input(&bytes);
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
            .record_pty(input_len, damage_regions, started.elapsed());
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
            eprintln!("c-term: failed to write terminal response to PTY: {error}");
        }
    }

    fn handle_resize(&mut self, size: PhysicalSize<u32>) {
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
            eprintln!("c-term: failed to resize PTY: {error}");
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

    fn handle_zoom_key(&mut self, key: &Key) -> bool {
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
            eprintln!("c-term: failed to persist zoom setting: {error}");
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
            eprintln!("c-term: failed to resize PTY after zoom: {error}");
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

    fn scroll_view(&mut self, delta: isize) -> bool {
        let Some(terminal) = &self.terminal else {
            return false;
        };
        if terminal.is_alternate_screen() {
            return false;
        }

        let max_offset = terminal.scrollback_len();
        let next = if delta >= 0 {
            self.scroll_offset.saturating_add(delta as usize)
        } else {
            self.scroll_offset.saturating_sub(delta.unsigned_abs())
        }
        .min(max_offset);

        if next == self.scroll_offset {
            return false;
        }
        self.scroll_offset = next;
        self.selection = None;
        self.mark_dirty();
        true
    }

    fn snap_to_live(&mut self) {
        if self.scroll_offset == 0 {
            return;
        }
        self.scroll_offset = 0;
        self.selection = None;
        self.render_cache.invalidate();
        self.mark_dirty();
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        if shortcut_key(&event.logical_key, self.modifiers, 'c') {
            self.copy_selection();
            return;
        }
        if shortcut_key(&event.logical_key, self.modifiers, 'v') {
            self.paste_clipboard();
            return;
        }
        if self.handle_zoom_key(&event.logical_key) {
            return;
        }

        if self.modifiers.control_key() && matches!(event.logical_key.as_ref(), Key::Character("q"))
        {
            event_loop.exit();
            return;
        }

        if self.modifiers.shift_key() {
            match event.logical_key.as_ref() {
                Key::Named(NamedKey::PageUp) => {
                    let lines = self.rows.saturating_sub(1).max(1) as isize;
                    let _ = self.scroll_view(lines);
                    return;
                }
                Key::Named(NamedKey::PageDown) => {
                    let lines = self.rows.saturating_sub(1).max(1) as isize;
                    let _ = self.scroll_view(-lines);
                    return;
                }
                _ => {}
            }
        }

        let Some(bytes) = encode_window_key(&event, self.modifiers) else {
            return;
        };

        self.selection = None;
        self.snap_to_live();
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("c-term: failed to write key to PTY: {error}");
        }
    }

    fn handle_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        let Some(cell) = self.mouse_position else {
            return;
        };
        let Some(button_code) = mouse_button_code(button) else {
            return;
        };

        match state {
            ElementState::Pressed => self.mouse_buttons |= 1 << button_code,
            ElementState::Released => self.mouse_buttons &= !(1 << button_code),
        }

        if button == MouseButton::Left && self.selection_mouse_mode() {
            match state {
                ElementState::Pressed => self.selection = Some(Selection::start(cell)),
                ElementState::Released => {
                    if let Some(selection) = &mut self.selection {
                        selection.finish();
                    }
                }
            }
            self.mark_dirty();
            return;
        }

        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            return;
        }

        let code = if state == ElementState::Pressed {
            button_code
        } else {
            3
        };
        self.write_mouse_event(mouse, code, cell, state == ElementState::Released);
    }

    fn handle_mouse_move(&mut self, position: PhysicalPosition<f64>) {
        let Some(cell) = mouse_cell(position, self.cols, self.rows, self.metrics) else {
            self.mouse_position = None;
            return;
        };
        if self.mouse_position == Some(cell) {
            return;
        }
        self.mouse_position = Some(cell);

        if let Some(selection) = &mut self.selection
            && selection.is_dragging()
        {
            if selection.update(cell) {
                self.mark_dirty();
            }
            return;
        }

        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        let code = match mouse.tracking {
            MouseTracking::Any => 35,
            MouseTracking::Drag if self.mouse_buttons != 0 => {
                active_mouse_button(self.mouse_buttons) + 32
            }
            _ => return,
        };
        self.write_mouse_event(mouse, code, cell, false);
    }

    fn handle_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            for lines in wheel_scroll_lines(delta, self.metrics) {
                let _ = self.scroll_view(lines);
            }
            return;
        }

        let Some(cell) = self.mouse_position else {
            return;
        };
        for code in wheel_codes(delta) {
            self.write_mouse_event(mouse, code, cell, false);
        }
    }

    fn write_mouse_event(&mut self, mouse: MouseState, code: u8, cell: (u16, u16), release: bool) {
        let bytes = encode_mouse_event(mouse, code, cell.0, cell.1, self.modifiers, release);
        if bytes.is_empty() {
            return;
        }
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("c-term: failed to write mouse event to PTY: {error}");
        }
    }

    fn selection_mouse_mode(&self) -> bool {
        self.modifiers.shift_key()
            || self
                .terminal
                .as_ref()
                .is_some_and(|terminal| terminal.mouse().tracking == MouseTracking::None)
    }

    fn copy_selection(&self) {
        let (Some(selection), Some(terminal)) = (self.selection, &self.terminal) else {
            return;
        };
        let text = selection.text(terminal, self.scroll_offset);
        if text.is_empty() {
            return;
        }
        let source = wl_copy::Source::Bytes(text.into_bytes().into_boxed_slice());
        if let Err(error) = wl_copy::Options::new().copy(source, wl_copy::MimeType::Text) {
            eprintln!("c-term: failed to copy selection: {error}");
        }
    }

    fn paste_clipboard(&mut self) {
        let (mut pipe, _) = match wl_paste::get_contents(
            wl_paste::ClipboardType::Regular,
            wl_paste::Seat::Unspecified,
            wl_paste::MimeType::Text,
        ) {
            Ok(contents) => contents,
            Err(
                wl_paste::Error::NoSeats
                | wl_paste::Error::ClipboardEmpty
                | wl_paste::Error::NoMimeType,
            ) => return,
            Err(error) => {
                eprintln!("c-term: failed to read clipboard: {error}");
                return;
            }
        };
        let mut text = String::new();
        if let Err(error) = pipe.read_to_string(&mut text) {
            eprintln!("c-term: failed to read clipboard text: {error}");
            return;
        }
        self.write_paste(&text);
    }

    fn write_paste(&mut self, text: &str) {
        let bracketed = self
            .terminal
            .as_ref()
            .is_some_and(TerminalCore::bracketed_paste);
        let mut bytes = Vec::with_capacity(text.len() + if bracketed { 12 } else { 0 });
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend(text.bytes().filter(|byte| *byte != 0));
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }

        self.selection = None;
        self.snap_to_live();
        if let Some(child) = &mut self.child
            && let Err(error) = child.master.write_all(&bytes)
        {
            eprintln!("c-term: failed to paste clipboard text to PTY: {error}");
        }
    }

    fn render(&mut self) {
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
            eprintln!("c-term: GPU render failed: {error}");
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

    fn timers_if_due(&mut self, event_loop: &ActiveEventLoop) {
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

#[derive(Debug, Clone)]
struct PerfStats {
    enabled: bool,
    interval_start: Instant,
    pty_events: u64,
    pty_bytes: u64,
    pty_core_time: Duration,
    damage_regions: u64,
    frames: u64,
    full_uploads: u64,
    row_bands: u64,
    scroll_uploads: u64,
    overlays: u64,
    cache_time: Duration,
    plugin_time: Duration,
    gpu_time: Duration,
    render_time: Duration,
}

#[derive(Debug, Clone, Copy)]
struct PerfFrameUpdate {
    upload_full: bool,
    upload_row_bands: usize,
    upload_scrolls: usize,
    overlays: usize,
}

impl PerfStats {
    fn from_env() -> Self {
        Self {
            enabled: env::var_os("TERMITE_PERF").is_some(),
            interval_start: Instant::now(),
            pty_events: 0,
            pty_bytes: 0,
            pty_core_time: Duration::ZERO,
            damage_regions: 0,
            frames: 0,
            full_uploads: 0,
            row_bands: 0,
            scroll_uploads: 0,
            overlays: 0,
            cache_time: Duration::ZERO,
            plugin_time: Duration::ZERO,
            gpu_time: Duration::ZERO,
            render_time: Duration::ZERO,
        }
    }

    fn record_pty(&mut self, bytes: usize, damage_regions: usize, core_time: Duration) {
        if !self.enabled {
            return;
        }
        self.pty_events += 1;
        self.pty_bytes += bytes as u64;
        self.damage_regions += damage_regions as u64;
        self.pty_core_time += core_time;
        self.report_if_due();
    }

    fn record_frame(
        &mut self,
        cache_time: Duration,
        plugin_time: Duration,
        gpu_time: Duration,
        render_time: Duration,
        update: PerfFrameUpdate,
    ) {
        if !self.enabled {
            return;
        }
        self.frames += 1;
        self.full_uploads += u64::from(update.upload_full);
        self.row_bands += update.upload_row_bands as u64;
        self.scroll_uploads += update.upload_scrolls as u64;
        self.overlays += update.overlays as u64;
        self.cache_time += cache_time;
        self.plugin_time += plugin_time;
        self.gpu_time += gpu_time;
        self.render_time += render_time;
        self.report_if_due();
    }

    fn report_if_due(&mut self) {
        let elapsed = self.interval_start.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        self.report_elapsed(elapsed);
    }

    fn report_final(&mut self) {
        if !self.enabled {
            return;
        }
        if self.pty_events == 0 && self.frames == 0 {
            return;
        }
        self.report_elapsed(self.interval_start.elapsed());
    }

    fn report_elapsed(&mut self, elapsed: Duration) {
        if elapsed.is_zero() {
            return;
        }

        let seconds = elapsed.as_secs_f64();
        let mib = self.pty_bytes as f64 / (1024.0 * 1024.0);
        eprintln!(
            concat!(
                "termite-perf ",
                "pty={:.2}MiB/s events={} damage={} ",
                "frames={} full={} rows={} scrolls={} overlays={} ",
                "core={:.2}ms cache={:.2}ms plugins={:.2}ms gpu={:.2}ms render={:.2}ms"
            ),
            mib / seconds,
            self.pty_events,
            self.damage_regions,
            self.frames,
            self.full_uploads,
            self.row_bands,
            self.scroll_uploads,
            self.overlays,
            duration_ms(self.pty_core_time),
            duration_ms(self.cache_time),
            duration_ms(self.plugin_time),
            duration_ms(self.gpu_time),
            duration_ms(self.render_time),
        );
        *self = Self {
            enabled: true,
            ..Self::from_env()
        };
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

impl ApplicationHandler<UserEvent> for WindowBackend {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.initialize(event_loop) {
            eprintln!("c-term: failed to initialize window backend: {error}");
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyBytes(bytes) => self.handle_pty_bytes(bytes),
            UserEvent::ChildExited => {
                self.perf.report_final();
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.timers_if_due(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.handle_resize(size),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => self.handle_mouse_move(position),
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => self.handle_mouse_wheel(delta),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => self.handle_key(event_loop, event),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

fn grid_size(size: PhysicalSize<u32>, metrics: TerminalMetrics) -> (u16, u16) {
    let cols = (size.width.max(metrics.cell_width) / metrics.cell_width)
        .clamp(1, u32::from(u16::MAX)) as u16;
    let rows = (size.height.max(metrics.cell_height) / metrics.cell_height)
        .clamp(1, u32::from(u16::MAX)) as u16;
    (cols, rows)
}

fn buffer_width(cols: u16, metrics: TerminalMetrics) -> u32 {
    u32::from(cols) * metrics.cell_width
}

fn buffer_height(rows: u16, metrics: TerminalMetrics) -> u32 {
    u32::from(rows) * metrics.cell_height
}

fn scaled_metrics(base: TerminalMetrics, zoom_steps: i16) -> TerminalMetrics {
    let step = i32::from(zoom_steps);
    TerminalMetrics {
        cell_width: (base.cell_width as i32 + step).max(6) as u32,
        cell_height: (base.cell_height as i32 + step * 2).max(10) as u32,
    }
}

fn scaled_font(font: &FontConfig, zoom_steps: i16) -> FontConfig {
    match font {
        FontConfig::GlyphAtlas { paths, size } => FontConfig::GlyphAtlas {
            paths: paths.clone(),
            size: (*size + f32::from(zoom_steps)).clamp(8.0, 32.0),
        },
        FontConfig::Bitmap8x8 => FontConfig::Bitmap8x8,
    }
}

fn normalize_zoom_steps(steps: i16) -> i16 {
    steps.clamp(MIN_ZOOM_STEPS, MAX_ZOOM_STEPS)
}

fn load_zoom_steps() -> Option<i16> {
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

fn parse_zoom_steps(text: &str) -> Option<i16> {
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
enum ZoomAction {
    Adjust(i16),
    Reset,
}

fn zoom_key_action(key: &Key, modifiers: ModifiersState) -> Option<ZoomAction> {
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

fn cursor_uniform(grid: &Grid, metrics: TerminalMetrics) -> [f32; 4] {
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

#[cfg(test)]
mod tests {
    use std::{
        hint::black_box,
        time::{Duration, Instant},
    };

    use super::text::{
        CellPaint, ascii_glyph_fallback, bitmap_glyph, box_segments, draw_block_cell,
        draw_box_cell, draw_shade_cell, sample_bitmap_axis,
    };
    use super::*;
    use crate::window_backend::render_cache::frame_len;
    use c_term_core::TerminalCore;

    const BENCH_COLS: u16 = 120;
    const BENCH_ROWS: u16 = 36;
    const BENCH_CHUNK: usize = 8192;

    #[test]
    fn box_drawing_characters_are_drawn_without_font_fallback() {
        let metrics = TerminalMetrics::default();
        let width = metrics.cell_width as usize;
        let mut frame = vec![0; width * metrics.cell_height as usize * 4];

        assert!(draw_box_cell(
            &mut frame,
            width,
            0,
            0,
            '─',
            CellPaint {
                fg: [220, 224, 232],
                bg: [16, 18, 24],
                background_opaque: true,
                metrics,
            },
        ));

        let center_y = metrics.cell_height as usize / 2;
        let center_index = (center_y * width + width / 2) * 4;
        assert_eq!(&frame[center_index..center_index + 3], &[220, 224, 232]);
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [16, 18, 24])
        );
    }

    #[test]
    fn non_box_characters_use_font_path() {
        assert_eq!(box_segments('A'), None);
    }

    #[test]
    fn block_characters_are_drawn_geometrically() {
        let metrics = TerminalMetrics::default();
        let width = metrics.cell_width as usize;
        let mut frame = vec![0; width * metrics.cell_height as usize * 4];

        assert!(draw_block_cell(
            &mut frame,
            width,
            0,
            0,
            '▄',
            CellPaint {
                fg: [220, 224, 232],
                bg: [16, 18, 24],
                background_opaque: true,
                metrics,
            },
        ));

        let top_index = (metrics.cell_height as usize / 4 * width + width / 2) * 4;
        let bottom_index = (metrics.cell_height as usize * 3 / 4 * width + width / 2) * 4;
        assert_eq!(&frame[top_index..top_index + 3], &[16, 18, 24]);
        assert_eq!(&frame[bottom_index..bottom_index + 3], &[220, 224, 232]);
    }

    #[test]
    fn shade_characters_are_drawn_as_mixed_pixels() {
        let metrics = TerminalMetrics::default();
        let width = metrics.cell_width as usize;
        let mut frame = vec![0; width * metrics.cell_height as usize * 4];

        assert!(draw_shade_cell(
            &mut frame,
            width,
            0,
            0,
            '▒',
            CellPaint {
                fg: [220, 224, 232],
                bg: [16, 18, 24],
                background_opaque: true,
                metrics,
            },
        ));

        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [220, 224, 232])
        );
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [16, 18, 24])
        );
    }

    #[test]
    fn bitmap_font_covers_extended_terminal_glyphs() {
        assert!(bitmap_glyph('A').is_some());
        assert!(bitmap_glyph('é').is_some());
        assert!(bitmap_glyph('═').is_some());
        assert!(bitmap_glyph('█').is_some());
        assert!(bitmap_glyph('░').is_some());
    }

    #[test]
    fn smart_punctuation_has_ascii_glyph_fallbacks() {
        assert_eq!(ascii_glyph_fallback('’'), Some('\''));
        assert_eq!(ascii_glyph_fallback('“'), Some('"'));
        assert_eq!(ascii_glyph_fallback('—'), Some('-'));
        assert_eq!(ascii_glyph_fallback('…'), Some('.'));
    }

    #[test]
    fn bitmap_fallback_sampling_is_centered_for_larger_cells() {
        let metrics = TerminalMetrics::default();
        let samples: Vec<_> = (0..metrics.cell_width as usize)
            .map(|x| sample_bitmap_axis(x, metrics.cell_width as usize))
            .collect();

        assert_eq!(samples.first(), Some(&0));
        assert_eq!(samples.last(), Some(&7));
        assert!(samples.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn zoom_scales_metrics_and_ttf_size_from_base_config() {
        assert_eq!(
            scaled_metrics(
                TerminalMetrics {
                    cell_width: 10,
                    cell_height: 18,
                },
                2,
            ),
            TerminalMetrics {
                cell_width: 12,
                cell_height: 22,
            }
        );

        assert_eq!(
            scaled_font(
                &FontConfig::GlyphAtlas {
                    paths: vec!["/tmp/a.ttf".to_owned()],
                    size: 15.0,
                },
                2,
            ),
            FontConfig::GlyphAtlas {
                paths: vec!["/tmp/a.ttf".to_owned()],
                size: 17.0,
            }
        );
    }

    #[test]
    fn zoom_steps_are_clamped_for_config_and_state() {
        assert_eq!(parse_zoom_steps("3\n"), Some(3));
        assert_eq!(parse_zoom_steps("99"), Some(MAX_ZOOM_STEPS));
        assert_eq!(parse_zoom_steps("-99"), Some(MIN_ZOOM_STEPS));
        assert_eq!(parse_zoom_steps("not zoom"), None);
    }

    #[test]
    fn shifted_minus_character_is_zoom_out_key() {
        assert_eq!(
            zoom_key_action(
                &Key::Character("_".into()),
                ModifiersState::CONTROL | ModifiersState::SHIFT,
            ),
            Some(ZoomAction::Adjust(-1))
        );
    }

    #[test]
    fn base_frame_cache_updates_only_dirty_rows() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"A\x1b[2;1HB");
        let metrics = TerminalMetrics::default();
        let mut cache = RenderCache::new(
            FontConfig::Bitmap8x8,
            Theme::default(),
            metrics,
            TextRenderConfig::default(),
        );

        let frame = cache.update(terminal.grid());
        let first_row =
            frame[..metrics.cell_height as usize * 4 * metrics.cell_width as usize * 4].to_vec();

        let tick = terminal.process_pty_input(b"\x1b[2;2HC");
        cache.apply_damage(&tick.damage, terminal.grid().height());
        let frame = cache.update(terminal.grid());

        assert_eq!(
            &frame[..metrics.cell_height as usize * 4 * metrics.cell_width as usize * 4],
            first_row.as_slice()
        );
    }

    #[test]
    fn scrollback_cache_handles_resized_history_rows() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"ab\r\ncd\r\nef");
        let _ = terminal.resize(5, 2);
        let metrics = TerminalMetrics::default();
        let mut cache = RenderCache::new(
            FontConfig::Bitmap8x8,
            Theme::default(),
            metrics,
            TextRenderConfig::default(),
        );

        let frame = cache.update_scrollback(&terminal, 1);

        assert_eq!(frame.len(), frame_len(5, 2, metrics));
    }

    #[test]
    fn scrollback_cache_keeps_incremental_scrolls_clean() {
        let mut terminal = TerminalCore::new(3, 2);
        let _ = terminal.process_pty_input(b"aa\r\nbb\r\ncc\r\ndd");
        let mut cache = RenderCache::new(
            FontConfig::Bitmap8x8,
            Theme::default(),
            TerminalMetrics::default(),
            TextRenderConfig::default(),
        );

        let _ = cache.update_scrollback(&terminal, 1);
        let _ = cache.update_scrollback(&terminal, 2);

        assert_eq!(cache.scroll_start, Some(0));
        assert!(!cache.dirty);
    }

    #[test]
    #[ignore]
    fn render_cache_bench() {
        let (_, _, font, metrics, theme, _, text_render) = crate::config::runner().into_parts();
        let workloads = [
            ("plain-scroll", bench_payload_plain_scroll()),
            ("color-table", bench_payload_color_table()),
            ("unicode", bench_payload_unicode()),
        ];

        println!(
            "{:<14} {:>9} {:>10} {:>10} {:>10}",
            "workload", "bytes", "core+draw", "draw-only", "updates"
        );
        for (name, payload) in workloads {
            let result = run_render_bench(&payload, font.clone(), metrics, theme, text_render);
            println!(
                "{:<14} {:>9} {:>9.2}ms {:>9.2}ms {:>10}",
                name,
                payload.len(),
                result.total.as_secs_f64() * 1000.0,
                result.draw.as_secs_f64() * 1000.0,
                result.updates
            );
        }
    }

    struct RenderBenchResult {
        total: Duration,
        draw: Duration,
        updates: usize,
    }

    fn run_render_bench(
        payload: &[u8],
        font: FontConfig,
        metrics: TerminalMetrics,
        theme: Theme,
        text_render: TextRenderConfig,
    ) -> RenderBenchResult {
        let mut terminal = TerminalCore::new(BENCH_COLS, BENCH_ROWS);
        let mut cache = RenderCache::new(font, theme, metrics, text_render);
        let started = Instant::now();
        let mut draw = Duration::ZERO;
        let mut updates = 0;

        for chunk in payload.chunks(BENCH_CHUNK) {
            let tick = terminal.process_pty_input(chunk);
            cache.apply_damage(&tick.damage, terminal.grid().height());
            let draw_started = Instant::now();
            let frame = cache.update(terminal.grid());
            black_box(frame.len());
            let update = cache.take_texture_update(terminal.grid().height());
            black_box(update.full);
            black_box(update.rows.len());
            black_box(update.scrolls.len());
            draw += draw_started.elapsed();
            updates += 1;
        }

        RenderBenchResult {
            total: started.elapsed(),
            draw,
            updates,
        }
    }

    fn bench_payload_plain_scroll() -> Vec<u8> {
        let mut bytes = Vec::new();
        for i in 0..30_000 {
            bytes.extend_from_slice(
                format!("render line {i:05} abcdefghijklmnopqrstuvwxyz\r\n").as_bytes(),
            );
        }
        bytes
    }

    fn bench_payload_color_table() -> Vec<u8> {
        let mut bytes = Vec::new();
        for row in 0..12_000 {
            for color in 0..16 {
                bytes.extend_from_slice(
                    format!("\x1b[{}m{:02x} ", 30 + color % 8, row + color).as_bytes(),
                );
            }
            bytes.extend_from_slice(b"\x1b[0m\r\n");
        }
        bytes
    }

    fn bench_payload_unicode() -> Vec<u8> {
        let mut bytes = Vec::new();
        let sample = "unicode 表 λ π ┌─┐ █ ░ we’ll — ok\r\n";
        for _ in 0..25_000 {
            bytes.extend_from_slice(sample.as_bytes());
        }
        bytes
    }
}
