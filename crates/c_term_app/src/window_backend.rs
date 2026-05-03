use std::{
    collections::HashMap,
    env,
    error::Error,
    fs,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont, point};
use c_term_core::{
    Color, CursorShape, DamageBatch, DamageRegion, Grid, MouseState, MouseTracking, Style,
    TerminalCore,
};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::{PtyChild, set_pty_winsize, spawn_shell};
use crate::{
    plugins::{PluginFrame, PluginHost},
    runner::Runner,
};

pub(crate) const CELL_WIDTH: u32 = 8;
pub(crate) const CELL_HEIGHT: u32 = 16;
const ANIMATION_FRAME_MS: u64 = 8;
const DELAYED_RENDER_LOWER_US: u64 = 150;
const DELAYED_RENDER_UPPER_NS: u64 = 4_000_000;
const APP_SYNC_TIMEOUT_MS: u64 = 1_000;
const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 540;
const FONT_SIZE: f32 = 14.0;

#[derive(Debug)]
enum UserEvent {
    PtyBytes(Vec<u8>),
    ChildExited,
}

pub(crate) fn run(runner: Runner) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let (shell, plugins) = runner.into_parts();
    let mut state = WindowBackend::new(shell, event_loop.create_proxy(), plugins);
    event_loop.run_app(&mut state)?;
    Ok(())
}

struct WindowBackend {
    shell: String,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    terminal: Option<TerminalCore>,
    plugins: PluginHost,
    child: Option<PtyChild>,
    modifiers: ModifiersState,
    cols: u16,
    rows: u16,
    render_cache: RenderCache,
    mouse_buttons: u8,
    mouse_position: Option<(u16, u16)>,
    animation_deadline: Option<Instant>,
    render_lower_deadline: Option<Instant>,
    render_upper_deadline: Option<Instant>,
    app_sync_deadline: Option<Instant>,
    redraw_pending: bool,
}

struct RenderCache {
    frame: Vec<u8>,
    dirty_rows: Vec<bool>,
    dirty: bool,
    text: TextRenderer,
}

impl RenderCache {
    fn new() -> Self {
        Self {
            frame: Vec::new(),
            dirty_rows: Vec::new(),
            dirty: true,
            text: TextRenderer::new(),
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.frame = vec![0; frame_len(cols, rows)];
        self.dirty_rows = vec![false; usize::from(rows)];
        self.dirty = true;
    }

    fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn apply_damage(&mut self, damage: &DamageBatch, rows: u16) {
        for region in &damage.regions {
            match *region {
                DamageRegion::Viewport => self.invalidate(),
                DamageRegion::Cells { y, height, .. } => {
                    let end = y.saturating_add(height).min(rows);
                    for row in y..end {
                        if let Some(dirty) = self.dirty_rows.get_mut(usize::from(row)) {
                            *dirty = true;
                        }
                    }
                }
                DamageRegion::Cursor { .. } => {}
            }
        }
    }

    fn update(&mut self, grid: &Grid) -> &[u8] {
        let expected_len = frame_len(grid.width(), grid.height());
        if self.frame.len() != expected_len {
            self.frame.resize(expected_len, 0);
            self.dirty = true;
        }
        if self.dirty_rows.len() != usize::from(grid.height()) {
            self.dirty_rows.resize(usize::from(grid.height()), true);
            self.dirty = true;
        }

        let width = usize::from(grid.width()) * CELL_WIDTH as usize;
        if self.dirty {
            self.frame.fill(0);
            for y in 0..grid.height() {
                self.text
                    .draw_grid_row_to_frame(grid, &mut self.frame, width, y);
            }
            self.dirty_rows.fill(false);
            self.dirty = false;
            return &self.frame;
        }

        for y in 0..grid.height() {
            let Some(dirty) = self.dirty_rows.get_mut(usize::from(y)) else {
                continue;
            };
            if !*dirty {
                continue;
            }
            clear_grid_row(&mut self.frame, width, y);
            self.text
                .draw_grid_row_to_frame(grid, &mut self.frame, width, y);
            *dirty = false;
        }

        &self.frame
    }

    fn draw_cursor(&mut self, grid: &Grid, frame: &mut [u8]) {
        let cursor = grid.cursor();
        if !cursor.visible {
            return;
        }
        let Some(cell) = grid.cell(cursor.x, cursor.y) else {
            return;
        };
        let width = usize::from(grid.width()) * CELL_WIDTH as usize;
        let ch = if cell.spacer { ' ' } else { cell.ch };
        self.text
            .draw_cell(frame, width, cursor.x, cursor.y, ch, cell.style);
        draw_cursor_shape(frame, width, cursor.x, cursor.y, cursor.shape);
    }
}

impl WindowBackend {
    fn new(shell: String, proxy: EventLoopProxy<UserEvent>, plugins: PluginHost) -> Self {
        Self {
            shell,
            proxy,
            window: None,
            pixels: None,
            terminal: None,
            plugins,
            child: None,
            modifiers: ModifiersState::empty(),
            cols: 1,
            rows: 1,
            render_cache: RenderCache::new(),
            mouse_buttons: 0,
            mouse_position: None,
            animation_deadline: None,
            render_lower_deadline: None,
            render_upper_deadline: None,
            app_sync_deadline: None,
            redraw_pending: false,
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
                    .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
                    .with_min_inner_size(LogicalSize::new(320, 200)),
            )?,
        );

        let size = window.inner_size();
        let (cols, rows) = grid_size(size);
        let surface = SurfaceTexture::new(size.width.max(1), size.height.max(1), window.clone());
        let pixels =
            PixelsBuilder::new(buffer_width(cols), buffer_height(rows), surface).build()?;
        let mut child = spawn_shell(&self.shell, cols, rows)?;
        spawn_pty_reader(&mut child, self.proxy.clone())?;

        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.terminal = Some(TerminalCore::new(cols, rows));
        self.pixels = Some(pixels);
        self.child = Some(child);
        self.window = Some(window);
        self.mark_dirty();
        Ok(())
    }

    fn mark_dirty(&mut self) {
        if !self.redraw_pending
            && let Some(window) = &self.window
        {
            self.redraw_pending = true;
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
        self.render_cache.apply_damage(damage, self.rows);
    }

    fn handle_pty_bytes(&mut self, bytes: Vec<u8>) {
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        let tick = terminal.process_pty_input(&bytes);
        let synchronized = terminal.grid().is_synchronized();
        let output = tick.output;
        self.apply_damage(&tick.damage);
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
        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };

        let width = size.width.max(1);
        let height = size.height.max(1);
        if let Err(error) = pixels.resize_surface(width, height) {
            eprintln!("c-term: failed to resize GPU surface: {error}");
            return;
        }

        let (cols, rows) = grid_size(size);
        if cols == self.cols && rows == self.rows {
            self.mark_dirty();
            return;
        }

        if let Some(child) = &mut self.child
            && let Err(error) = set_pty_winsize(child.master.as_raw_fd(), cols, rows)
        {
            eprintln!("c-term: failed to resize PTY: {error}");
        }

        if let Some(terminal) = &mut self.terminal {
            let tick = terminal.resize(cols, rows);
            self.apply_damage(&tick.damage);
        }

        let Some(pixels) = self.pixels.as_mut() else {
            return;
        };
        if let Err(error) = pixels.resize_buffer(buffer_width(cols), buffer_height(rows)) {
            eprintln!("c-term: failed to resize GPU buffer: {error}");
        }

        self.cols = cols;
        self.rows = rows;
        self.render_cache.resize(cols, rows);
        self.mark_dirty();
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() && matches!(event.logical_key.as_ref(), Key::Character("q"))
        {
            event_loop.exit();
            return;
        }

        let Some(bytes) = encode_window_key(&event, self.modifiers) else {
            return;
        };

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
        let Some(cell) = mouse_cell(position, self.cols, self.rows) else {
            self.mouse_position = None;
            return;
        };
        if self.mouse_position == Some(cell) {
            return;
        }
        self.mouse_position = Some(cell);

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
        let Some(cell) = self.mouse_position else {
            return;
        };
        let Some(terminal) = &self.terminal else {
            return;
        };
        let mouse = terminal.mouse();
        if mouse.tracking == MouseTracking::None {
            return;
        }

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

    fn render(&mut self) {
        self.redraw_pending = false;
        self.disarm_delayed_render();
        let Some(terminal) = &self.terminal else {
            return;
        };
        self.render_cache.update(terminal.grid());

        let Some(pixels) = &mut self.pixels else {
            return;
        };

        let frame = pixels.frame_mut();
        frame.copy_from_slice(&self.render_cache.frame);
        let now = Instant::now();
        let plugin_active = self.plugins.draw(&mut PluginFrame {
            frame,
            width_px: usize::from(terminal.grid().width()) * CELL_WIDTH as usize,
            grid: terminal.grid(),
            now,
        });
        self.render_cache.draw_cursor(terminal.grid(), frame);
        if let Err(error) = pixels.render() {
            eprintln!("c-term: GPU render failed: {error}");
        }
        if plugin_active {
            self.schedule_animation(now);
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
            self.mark_dirty();
        }

        let render_due = self
            .render_lower_deadline
            .is_some_and(|deadline| deadline <= now)
            || self
                .render_upper_deadline
                .is_some_and(|deadline| deadline <= now);
        if render_due {
            self.disarm_delayed_render();
            self.mark_dirty();
        }

        if self
            .animation_deadline
            .is_some_and(|deadline| deadline <= now)
        {
            self.animation_deadline = None;
            self.mark_dirty();
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
        ]
        .into_iter()
        .flatten()
        .min()
    }
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
            UserEvent::ChildExited => event_loop.exit(),
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

fn spawn_pty_reader(child: &mut PtyChild, proxy: EventLoopProxy<UserEvent>) -> io::Result<()> {
    let mut reader = child.master.try_clone()?;
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            let n = match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(0) | Err(_) => {
                    let _ = proxy.send_event(UserEvent::ChildExited);
                    break;
                }
                Ok(n) => n,
            };
            if proxy
                .send_event(UserEvent::PtyBytes(buffer[..n].to_vec()))
                .is_err()
            {
                break;
            }
        }
    });
    Ok(())
}

fn grid_size(size: PhysicalSize<u32>) -> (u16, u16) {
    let cols = (size.width.max(CELL_WIDTH) / CELL_WIDTH).clamp(1, u32::from(u16::MAX)) as u16;
    let rows = (size.height.max(CELL_HEIGHT) / CELL_HEIGHT).clamp(1, u32::from(u16::MAX)) as u16;
    (cols, rows)
}

fn buffer_width(cols: u16) -> u32 {
    u32::from(cols) * CELL_WIDTH
}

fn buffer_height(rows: u16) -> u32 {
    u32::from(rows) * CELL_HEIGHT
}

fn frame_len(cols: u16, rows: u16) -> usize {
    buffer_width(cols) as usize * buffer_height(rows) as usize * 4
}

fn encode_window_key(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    if modifiers.alt_key() {
        bytes.push(0x1b);
    }

    match event.logical_key.as_ref() {
        Key::Character(ch) if modifiers.control_key() => {
            let mut chars = ch.chars();
            if let Some(ch) = chars.next().filter(|_| chars.next().is_none()) {
                bytes.push(ctrl_byte(ch)?);
            } else {
                return None;
            }
        }
        Key::Character(ch) => bytes.extend_from_slice(ch.as_bytes()),
        Key::Named(NamedKey::Space) if modifiers.control_key() => bytes.push(0x00),
        Key::Named(NamedKey::Space) => bytes.push(b' '),
        Key::Named(NamedKey::Enter) => bytes.push(b'\r'),
        Key::Named(NamedKey::Backspace) => bytes.push(0x7f),
        Key::Named(NamedKey::Tab) => bytes.push(b'\t'),
        Key::Named(NamedKey::Escape) => bytes.push(0x1b),
        Key::Named(NamedKey::ArrowLeft) => bytes.extend_from_slice(b"\x1b[D"),
        Key::Named(NamedKey::ArrowRight) => bytes.extend_from_slice(b"\x1b[C"),
        Key::Named(NamedKey::ArrowUp) => bytes.extend_from_slice(b"\x1b[A"),
        Key::Named(NamedKey::ArrowDown) => bytes.extend_from_slice(b"\x1b[B"),
        Key::Named(NamedKey::Home) => bytes.extend_from_slice(b"\x1b[H"),
        Key::Named(NamedKey::End) => bytes.extend_from_slice(b"\x1b[F"),
        Key::Named(NamedKey::PageUp) => bytes.extend_from_slice(b"\x1b[5~"),
        Key::Named(NamedKey::PageDown) => bytes.extend_from_slice(b"\x1b[6~"),
        Key::Named(NamedKey::Delete) => bytes.extend_from_slice(b"\x1b[3~"),
        Key::Named(NamedKey::Insert) => bytes.extend_from_slice(b"\x1b[2~"),
        _ => return None,
    }

    Some(bytes)
}

fn ctrl_byte(ch: char) -> Option<u8> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_alphabetic() {
        Some((lower as u8) - b'a' + 1)
    } else {
        match lower {
            ' ' | '@' | '2' => Some(0x00),
            '[' => Some(0x1b),
            '3' => Some(0x1b),
            '\\' => Some(0x1c),
            '4' => Some(0x1c),
            ']' => Some(0x1d),
            '5' => Some(0x1d),
            '^' => Some(0x1e),
            '6' => Some(0x1e),
            '_' | '/' | '-' | '7' => Some(0x1f),
            '?' => Some(0x7f),
            '8' => Some(0x7f),
            '`' => Some(0x00),
            '~' => Some(0x1e),
            '=' => Some(0x1f),
            _ => None,
        }
    }
}

fn mouse_cell(position: PhysicalPosition<f64>, cols: u16, rows: u16) -> Option<(u16, u16)> {
    if position.x < 0.0 || position.y < 0.0 {
        return None;
    }
    let x = (position.x as u32 / CELL_WIDTH) as u16;
    let y = (position.y as u32 / CELL_HEIGHT) as u16;
    (x < cols && y < rows).then_some((x, y))
}

fn mouse_button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    }
}

fn active_mouse_button(buttons: u8) -> u8 {
    for button in 0..3 {
        if buttons & (1 << button) != 0 {
            return button;
        }
    }
    3
}

fn wheel_codes(delta: MouseScrollDelta) -> Vec<u8> {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => wheel_axis_codes(x, y),
        MouseScrollDelta::PixelDelta(position) => {
            wheel_axis_codes(position.x as f32, position.y as f32)
        }
    }
}

fn wheel_axis_codes(x: f32, y: f32) -> Vec<u8> {
    let mut codes = Vec::new();
    if y > 0.0 {
        codes.push(64);
    } else if y < 0.0 {
        codes.push(65);
    }
    if x > 0.0 {
        codes.push(67);
    } else if x < 0.0 {
        codes.push(66);
    }
    codes
}

fn encode_mouse_event(
    mouse: MouseState,
    code: u8,
    x: u16,
    y: u16,
    modifiers: ModifiersState,
    release: bool,
) -> Vec<u8> {
    if mouse.tracking == MouseTracking::None {
        return Vec::new();
    }

    let code = code + mouse_modifier_bits(modifiers);
    let col = x.saturating_add(1);
    let row = y.saturating_add(1);
    if mouse.sgr {
        let final_byte = if release { 'm' } else { 'M' };
        format!("\x1b[<{code};{col};{row}{final_byte}").into_bytes()
    } else {
        let Ok(code) = u8::try_from(u16::from(code) + 32) else {
            return Vec::new();
        };
        let Ok(col) = u8::try_from(col.saturating_add(32)) else {
            return Vec::new();
        };
        let Ok(row) = u8::try_from(row.saturating_add(32)) else {
            return Vec::new();
        };
        vec![0x1b, b'[', b'M', code, col, row]
    }
}

fn mouse_modifier_bits(modifiers: ModifiersState) -> u8 {
    let mut bits = 0;
    if modifiers.shift_key() {
        bits |= 4;
    }
    if modifiers.alt_key() {
        bits |= 8;
    }
    if modifiers.control_key() {
        bits |= 16;
    }
    bits
}

fn clear_grid_row(frame: &mut [u8], width: usize, y: u16) {
    let row_start = usize::from(y) * CELL_HEIGHT as usize * width * 4;
    let row_len = CELL_HEIGHT as usize * width * 4;
    if let Some(row) = frame.get_mut(row_start..row_start + row_len) {
        row.fill(0);
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphKey {
    font: usize,
    ch: char,
}

struct LoadedFont {
    font: FontArc,
    scale: PxScale,
    ascent: f32,
    height: f32,
}

struct GlyphBitmap {
    left: i16,
    top: i16,
    width: u16,
    height: u16,
    alpha: Vec<u8>,
}

struct TextRenderer {
    fonts: Vec<LoadedFont>,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
}

impl TextRenderer {
    fn new() -> Self {
        Self {
            fonts: load_fonts(),
            glyphs: HashMap::new(),
        }
    }

    fn draw_grid_row_to_frame(&mut self, grid: &Grid, frame: &mut [u8], width: usize, y: u16) {
        let Some(row) = grid.row(y) else {
            return;
        };
        for (x, cell) in row.iter().enumerate() {
            let ch = if cell.spacer { ' ' } else { cell.ch };
            self.draw_cell(frame, width, x as u16, y, ch, cell.style);
        }
    }

    fn draw_cell(
        &mut self,
        frame: &mut [u8],
        width: usize,
        cell_x: u16,
        cell_y: u16,
        ch: char,
        style: Style,
    ) {
        let fg = rgb(style.foreground, [220, 224, 232]);
        let bg = rgb(style.background, [16, 18, 24]);
        if draw_box_cell(frame, width, cell_x, cell_y, ch, fg, bg) {
            return;
        }

        fill_cell(frame, width, cell_x, cell_y, bg);
        if ch != ' ' {
            if let Some(key) = self.glyph_key(ch) {
                self.ensure_glyph(key);
                if let Some(glyph) = self.glyphs.get(&key) {
                    draw_glyph_bitmap(frame, width, cell_x, cell_y, glyph, fg, 0);
                    if style.bold {
                        draw_glyph_bitmap(frame, width, cell_x, cell_y, glyph, fg, 1);
                    }
                }
            } else {
                draw_font8x8_cell(frame, width, cell_x, cell_y, ch, fg, bg);
            }
        }
        if style.underline {
            draw_underline(frame, width, cell_x, cell_y, fg);
        }
    }

    fn glyph_key(&self, ch: char) -> Option<GlyphKey> {
        self.font_index(ch)
            .map(|font| GlyphKey { font, ch })
            .or_else(|| self.font_index('?').map(|font| GlyphKey { font, ch: '?' }))
    }

    fn font_index(&self, ch: char) -> Option<usize> {
        self.fonts
            .iter()
            .position(|font| font.font.glyph_id(ch) != GlyphId(0))
    }

    fn ensure_glyph(&mut self, key: GlyphKey) {
        if self.glyphs.contains_key(&key) {
            return;
        }
        let glyph = rasterize_glyph(&self.fonts[key.font], key.ch);
        self.glyphs.insert(key, glyph);
    }
}

fn load_fonts() -> Vec<LoadedFont> {
    let mut paths = Vec::new();
    paths.extend(env::var_os("TERMITE_FONT").and_then(|path| path.into_string().ok()));
    paths.extend(env::var_os("C_TERM_FONT").and_then(|path| path.into_string().ok()));

    let mut loaded = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if paths[..index].contains(path) {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let Ok(font) = FontArc::try_from_vec(bytes) else {
            continue;
        };
        let scaled = font.as_scaled(FONT_SIZE);
        let ascent = scaled.ascent();
        let height = scaled.height();
        loaded.push(LoadedFont {
            font,
            scale: PxScale::from(FONT_SIZE),
            ascent,
            height,
        });
    }
    loaded
}

fn rasterize_glyph(font: &LoadedFont, ch: char) -> GlyphBitmap {
    let scaled = font.font.as_scaled(font.scale);
    let glyph_id = scaled.glyph_id(ch);
    let advance = scaled.h_advance(glyph_id);
    let x = ((CELL_WIDTH as f32 - advance) * 0.5).floor().max(-1.0);
    let baseline = ((CELL_HEIGHT as f32 - font.height) * 0.5 + font.ascent).round();
    let glyph = glyph_id.with_scale_and_position(font.scale, point(x, baseline));
    let Some(outlined) = scaled.outline_glyph(glyph) else {
        return GlyphBitmap {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
            alpha: Vec::new(),
        };
    };

    let bounds = outlined.px_bounds();
    let width = bounds.width().max(0.0) as u16;
    let height = bounds.height().max(0.0) as u16;
    let mut alpha = vec![0; usize::from(width) * usize::from(height)];
    outlined.draw(|x, y, coverage| {
        let index =
            usize::try_from(y).unwrap_or(0) * usize::from(width) + usize::try_from(x).unwrap_or(0);
        if let Some(alpha) = alpha.get_mut(index) {
            *alpha = coverage.mul_add(255.0, 0.5).clamp(0.0, 255.0) as u8;
        }
    });

    GlyphBitmap {
        left: bounds.min.x.floor() as i16,
        top: bounds.min.y.floor() as i16,
        width,
        height,
        alpha,
    }
}

fn fill_cell(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, color: [u8; 3]) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for py in 0..CELL_HEIGHT as usize {
        for px in 0..CELL_WIDTH as usize {
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

fn draw_glyph_bitmap(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    glyph: &GlyphBitmap,
    color: [u8; 3],
    x_shift: i16,
) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for y in 0..glyph.height {
        let cell_py = glyph.top + y as i16;
        if !(0..CELL_HEIGHT as i16).contains(&cell_py) {
            continue;
        }
        for x in 0..glyph.width {
            let cell_px = glyph.left + x as i16 + x_shift;
            if !(0..CELL_WIDTH as i16).contains(&cell_px) {
                continue;
            }
            let alpha = glyph.alpha[usize::from(y) * usize::from(glyph.width) + usize::from(x)];
            if alpha == 0 {
                continue;
            }
            let index = ((origin_y + cell_py as usize) * width + origin_x + cell_px as usize) * 4;
            blend_pixel(&mut frame[index..index + 4], color, alpha);
        }
    }
}

fn blend_pixel(pixel: &mut [u8], color: [u8; 3], alpha: u8) {
    let alpha = u16::from(alpha);
    let inv = 255 - alpha;
    for (channel, color) in pixel[..3].iter_mut().zip(color) {
        *channel = ((u16::from(*channel) * inv + u16::from(color) * alpha) / 255) as u8;
    }
    pixel[3] = 0xff;
}

fn draw_font8x8_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
) {
    let glyph = BASIC_FONTS
        .get(ch)
        .or_else(|| BASIC_FONTS.get('?'))
        .unwrap_or([0; 8]);
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    for py in 0..CELL_HEIGHT as usize {
        let glyph_row = glyph[py / 2];
        for px in 0..CELL_WIDTH as usize {
            let color = if ((glyph_row >> px) & 1) != 0 { fg } else { bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

fn draw_underline(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, color: [u8; 3]) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let y = origin_y + CELL_HEIGHT as usize - 2;
    for px in 0..CELL_WIDTH as usize {
        let index = (y * width + origin_x + px) * 4;
        frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
    }
}

fn draw_box_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
) -> bool {
    let Some((left, right, up, down)) = box_segments(ch) else {
        return false;
    };
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let center_x = CELL_WIDTH as usize / 2;
    let center_y = CELL_HEIGHT as usize / 2;
    let thickness = 2;

    for py in 0..CELL_HEIGHT as usize {
        for px in 0..CELL_WIDTH as usize {
            let horizontal = py.abs_diff(center_y) < thickness
                && ((left && px <= center_x) || (right && px >= center_x));
            let vertical = px.abs_diff(center_x) < thickness
                && ((up && py <= center_y) || (down && py >= center_y));
            let color = if horizontal || vertical { fg } else { bg };
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
    true
}

fn box_segments(ch: char) -> Option<(bool, bool, bool, bool)> {
    match ch {
        '─' | '━' | '╌' | '╍' | '⎺' | '⎻' | '⎼' | '⎽' => {
            Some((true, true, false, false))
        }
        '╴' => Some((true, false, false, false)),
        '╶' => Some((false, true, false, false)),
        '│' | '┃' | '╎' | '╏' | '┆' | '┇' | '┊' | '┋' => {
            Some((false, false, true, true))
        }
        '╵' => Some((false, false, true, false)),
        '╷' => Some((false, false, false, true)),
        '┌' | '┏' | '╭' => Some((false, true, false, true)),
        '┐' | '┓' | '╮' => Some((true, false, false, true)),
        '└' | '┗' | '╰' => Some((false, true, true, false)),
        '┘' | '┛' | '╯' => Some((true, false, true, false)),
        '├' | '┣' => Some((false, true, true, true)),
        '┤' | '┫' => Some((true, false, true, true)),
        '┬' | '┳' => Some((true, true, false, true)),
        '┴' | '┻' => Some((true, true, true, false)),
        '┼' | '╋' => Some((true, true, true, true)),
        _ => None,
    }
}

fn draw_cursor_shape(frame: &mut [u8], width: usize, cell_x: u16, cell_y: u16, shape: CursorShape) {
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;
    let (x_start, y_start, cursor_width, cursor_height) = match shape {
        CursorShape::Block => (0, 0, CELL_WIDTH as usize, CELL_HEIGHT as usize),
        CursorShape::Beam => (0, 0, (CELL_WIDTH as usize / 4).max(1), CELL_HEIGHT as usize),
        CursorShape::Underline => (
            0,
            CELL_HEIGHT as usize - (CELL_HEIGHT as usize / 5).max(1),
            CELL_WIDTH as usize,
            (CELL_HEIGHT as usize / 5).max(1),
        ),
    };

    for py in y_start..y_start + cursor_height {
        for px in x_start..x_start + cursor_width {
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            for channel in &mut frame[index..index + 3] {
                *channel = 255 - *channel;
            }
        }
    }
}

pub(crate) fn rgb(color: Color, fallback: [u8; 3]) -> [u8; 3] {
    match color {
        Color::DefaultForeground | Color::DefaultBackground => fallback,
        Color::Indexed(index) => ANSI_COLORS
            .get(usize::from(index))
            .copied()
            .unwrap_or(fallback),
        Color::Rgb(r, g, b) => [r, g, b],
    }
}

const ANSI_COLORS: [[u8; 3]; 16] = [
    [12, 12, 12],
    [197, 15, 31],
    [19, 161, 14],
    [193, 156, 0],
    [0, 55, 218],
    [136, 23, 152],
    [58, 150, 221],
    [204, 204, 204],
    [118, 118, 118],
    [231, 72, 86],
    [22, 198, 12],
    [249, 241, 165],
    [59, 120, 255],
    [180, 0, 158],
    [97, 214, 214],
    [242, 242, 242],
];

#[cfg(test)]
mod tests {
    use super::*;
    use c_term_core::TerminalCore;

    #[test]
    fn box_drawing_characters_are_drawn_without_font_fallback() {
        let width = CELL_WIDTH as usize;
        let mut frame = vec![0; width * CELL_HEIGHT as usize * 4];

        assert!(draw_box_cell(
            &mut frame,
            width,
            0,
            0,
            '─',
            [220, 224, 232],
            [16, 18, 24],
        ));

        let center_y = CELL_HEIGHT as usize / 2;
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
    fn base_frame_cache_updates_only_dirty_rows() {
        let mut terminal = TerminalCore::new(4, 2);
        let _ = terminal.process_pty_input(b"A\x1b[2;1HB");
        let mut cache = RenderCache::new();

        let frame = cache.update(terminal.grid());
        let first_row = frame[..CELL_HEIGHT as usize * 4 * CELL_WIDTH as usize * 4].to_vec();

        let tick = terminal.process_pty_input(b"\x1b[2;2HC");
        cache.apply_damage(&tick.damage, terminal.grid().height());
        let frame = cache.update(terminal.grid());

        assert_eq!(
            &frame[..CELL_HEIGHT as usize * 4 * CELL_WIDTH as usize * 4],
            first_row.as_slice()
        );
    }

    #[test]
    fn sgr_mouse_encoding_uses_one_based_coordinates() {
        let mouse = MouseState {
            tracking: MouseTracking::Click,
            sgr: true,
        };

        assert_eq!(
            encode_mouse_event(mouse, 0, 2, 3, ModifiersState::empty(), false),
            b"\x1b[<0;3;4M"
        );
        assert_eq!(
            encode_mouse_event(mouse, 0, 2, 3, ModifiersState::empty(), true),
            b"\x1b[<0;3;4m"
        );
    }

    #[test]
    fn mouse_encoding_includes_modifiers() {
        let mouse = MouseState {
            tracking: MouseTracking::Click,
            sgr: true,
        };

        assert_eq!(
            encode_mouse_event(
                mouse,
                64,
                0,
                0,
                ModifiersState::SHIFT | ModifiersState::CONTROL,
                false,
            ),
            b"\x1b[<84;1;1M"
        );
    }
}
