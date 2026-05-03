use std::{
    env,
    error::Error,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::Arc,
    thread,
};

use c_term_core::{Color, Grid, Style, TerminalCore};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::{PtyChild, set_pty_winsize, spawn_shell};

const CELL_WIDTH: u32 = 8;
const CELL_HEIGHT: u32 = 16;
const INITIAL_WIDTH: u32 = 960;
const INITIAL_HEIGHT: u32 = 540;

#[derive(Debug)]
enum UserEvent {
    PtyBytes(Vec<u8>),
    ChildExited,
}

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut state = WindowBackend::new(shell, event_loop.create_proxy());
    event_loop.run_app(&mut state)?;
    Ok(())
}

struct WindowBackend {
    shell: String,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    pixels: Option<Pixels<'static>>,
    terminal: Option<TerminalCore>,
    child: Option<PtyChild>,
    modifiers: ModifiersState,
    cols: u16,
    rows: u16,
}

impl WindowBackend {
    fn new(shell: String, proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            shell,
            proxy,
            window: None,
            pixels: None,
            terminal: None,
            child: None,
            modifiers: ModifiersState::empty(),
            cols: 1,
            rows: 1,
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
        self.terminal = Some(TerminalCore::new(cols, rows));
        self.pixels = Some(pixels);
        self.child = Some(child);
        self.window = Some(window);
        self.mark_dirty();
        Ok(())
    }

    fn mark_dirty(&mut self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn handle_pty_bytes(&mut self, bytes: Vec<u8>) {
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        if !terminal.process_pty_input(&bytes).is_idle() {
            self.mark_dirty();
        }
    }

    fn handle_resize(&mut self, size: PhysicalSize<u32>) {
        let Some(pixels) = &mut self.pixels else {
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
            let _ = terminal.resize(cols, rows);
        }

        if let Err(error) = pixels.resize_buffer(buffer_width(cols), buffer_height(rows)) {
            eprintln!("c-term: failed to resize GPU buffer: {error}");
        }

        self.cols = cols;
        self.rows = rows;
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

    fn render(&mut self) {
        let (Some(terminal), Some(pixels)) = (&self.terminal, &mut self.pixels) else {
            return;
        };

        draw_grid_to_frame(terminal.grid(), pixels.frame_mut());
        if let Err(error) = pixels.render() {
            eprintln!("c-term: GPU render failed: {error}");
        }
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.handle_resize(size),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
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

fn draw_grid_to_frame(grid: &Grid, frame: &mut [u8]) {
    let width = usize::from(grid.width()) * CELL_WIDTH as usize;
    frame.fill(0);

    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let Some(cell) = grid.cell(x, y) else {
                continue;
            };
            draw_cell(
                frame,
                width,
                x,
                y,
                cell.ch,
                cell.style,
                grid.cursor().visible && grid.cursor().x == x && grid.cursor().y == y,
            );
        }
    }
}

fn draw_cell(
    frame: &mut [u8],
    width: usize,
    cell_x: u16,
    cell_y: u16,
    ch: char,
    style: Style,
    cursor: bool,
) {
    let fg = rgb(style.foreground, [220, 224, 232]);
    let bg = rgb(style.background, [16, 18, 24]);
    let glyph = BASIC_FONTS
        .get(ch)
        .or_else(|| BASIC_FONTS.get('?'))
        .unwrap_or([0; 8]);
    let origin_x = usize::from(cell_x) * CELL_WIDTH as usize;
    let origin_y = usize::from(cell_y) * CELL_HEIGHT as usize;

    for py in 0..CELL_HEIGHT as usize {
        let glyph_row = glyph[py / 2];
        for px in 0..CELL_WIDTH as usize {
            let bit_set = ((glyph_row >> px) & 1) != 0;
            let mut color = if bit_set { fg } else { bg };
            if cursor {
                color = [255 - color[0], 255 - color[1], 255 - color[2]];
            }
            let index = ((origin_y + py) * width + origin_x + px) * 4;
            frame[index..index + 4].copy_from_slice(&[color[0], color[1], color[2], 0xff]);
        }
    }
}

fn rgb(color: Color, fallback: [u8; 3]) -> [u8; 3] {
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
