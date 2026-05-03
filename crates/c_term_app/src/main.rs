use std::{
    env,
    error::Error,
    ffi::CString,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    sync::mpsc,
    thread,
    time::Duration,
};

mod window_backend;

use c_term_app::{AppAction, TerminalApp};
use c_term_core::{Color, Grid, Style};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{self, Attribute, Print, SetAttribute},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use nix::{
    pty::{ForkptyResult, Winsize, forkpty},
    sys::wait::{WaitPidFlag, WaitStatus, waitpid},
    unistd::execvp,
};

fn main() {
    if let Err(error) = run() {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        eprintln!("c-term: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    if env::args().any(|arg| arg == "--host") {
        run_host_backend()?;
    } else {
        window_backend::run_window_backend()?;
    }
    Ok(())
}

fn run_host_backend() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "c-term must be run from an interactive terminal",
        ));
    }

    let (cols, rows) = terminal::size()?;
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let mut child = spawn_shell(&shell, cols, rows)?;

    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;

    let result = run_loop(&mut child, cols, rows);

    let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();

    result
}

fn run_loop(child: &mut PtyChild, cols: u16, rows: u16) -> io::Result<()> {
    let (pty_tx, pty_rx) = mpsc::channel();
    let mut reader = child.master.try_clone()?;
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if pty_tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });

    let mut app = TerminalApp::new(cols, rows, u32::from(cols) * 8, u32::from(rows) * 16);
    let mut stdout = io::stdout();
    render_grid(&mut stdout, app.core().grid())?;

    loop {
        while let Ok(bytes) = pty_rx.try_recv() {
            if app.handle_action(AppAction::PtyBytes(bytes)).is_ok() {
                render_grid(&mut stdout, app.core().grid())?;
            }
        }

        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('q'))
                    {
                        return Ok(());
                    }

                    if let Some(bytes) = encode_key(key) {
                        let _ = app.handle_action(AppAction::KeyPress(c_term_core::KeyPress {
                            logical_key: format!("{:?}", key.code),
                            text: key_text(&key),
                            modifiers: c_term_core::KeyModifiers {
                                ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
                                alt: key.modifiers.contains(KeyModifiers::ALT),
                                shift: key.modifiers.contains(KeyModifiers::SHIFT),
                                super_key: key.modifiers.contains(KeyModifiers::SUPER),
                            },
                        }));
                        child.master.write_all(&bytes)?;
                        child.master.flush()?;
                    }
                }
                Event::Resize(new_cols, new_rows) => {
                    set_pty_winsize(child.master.as_raw_fd(), new_cols, new_rows)?;
                    let _ = app.handle_action(AppAction::ResizeCells {
                        width: new_cols,
                        height: new_rows,
                    });
                    let _ = app.handle_action(AppAction::ResizePixels {
                        width: u32::from(new_cols) * 8,
                        height: u32::from(new_rows) * 16,
                    });
                    render_grid(&mut stdout, app.core().grid())?;
                }
                Event::FocusGained | Event::FocusLost | Event::Mouse(_) | Event::Paste(_) => {}
            }
        }

        if child_has_exited(child.pid) {
            return Ok(());
        }
    }
}

fn render_grid(stdout: &mut impl Write, grid: &Grid) -> io::Result<()> {
    queue!(
        stdout,
        cursor::Hide,
        cursor::MoveTo(0, 0),
        style::ResetColor,
        SetAttribute(Attribute::Reset)
    )?;

    let mut current_style = None;
    for y in 0..grid.height() {
        queue!(stdout, cursor::MoveTo(0, y))?;
        for x in 0..grid.width() {
            let Some(cell) = grid.cell(x, y) else {
                continue;
            };
            if current_style != Some(cell.style) {
                queue_style(stdout, cell.style)?;
                current_style = Some(cell.style);
            }
            queue!(stdout, Print(cell.ch))?;
        }
    }

    queue!(
        stdout,
        style::ResetColor,
        SetAttribute(Attribute::Reset),
        cursor::MoveTo(grid.cursor().x, grid.cursor().y)
    )?;
    stdout.flush()
}

fn queue_style(stdout: &mut impl Write, cell_style: Style) -> io::Result<()> {
    queue!(stdout, style::ResetColor, SetAttribute(Attribute::Reset))?;

    if let Some(color) = to_crossterm_color(cell_style.foreground) {
        queue!(stdout, style::SetForegroundColor(color))?;
    }
    if let Some(color) = to_crossterm_color(cell_style.background) {
        queue!(stdout, style::SetBackgroundColor(color))?;
    }
    if cell_style.bold {
        queue!(stdout, SetAttribute(Attribute::Bold))?;
    }
    if cell_style.italic {
        queue!(stdout, SetAttribute(Attribute::Italic))?;
    }
    if cell_style.underline {
        queue!(stdout, SetAttribute(Attribute::Underlined))?;
    }

    Ok(())
}

fn to_crossterm_color(color: Color) -> Option<style::Color> {
    match color {
        Color::DefaultForeground | Color::DefaultBackground => None,
        Color::Indexed(index) => Some(match index {
            0 => style::Color::Black,
            1 => style::Color::DarkRed,
            2 => style::Color::DarkGreen,
            3 => style::Color::DarkYellow,
            4 => style::Color::DarkBlue,
            5 => style::Color::DarkMagenta,
            6 => style::Color::DarkCyan,
            7 => style::Color::Grey,
            8 => style::Color::DarkGrey,
            9 => style::Color::Red,
            10 => style::Color::Green,
            11 => style::Color::Yellow,
            12 => style::Color::Blue,
            13 => style::Color::Magenta,
            14 => style::Color::Cyan,
            15 => style::Color::White,
            index => style::Color::AnsiValue(index),
        }),
        Color::Rgb(r, g, b) => Some(style::Color::Rgb { r, g, b }),
    }
}

pub(crate) struct PtyChild {
    pub(crate) pid: nix::unistd::Pid,
    pub(crate) master: File,
}

pub(crate) fn spawn_shell(shell: &str, cols: u16, rows: u16) -> io::Result<PtyChild> {
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: The child branch immediately calls execvp or exits. The parent
    // only receives the PTY master and child pid.
    match unsafe { forkpty(&winsize, None) }.map_err(io::Error::other)? {
        ForkptyResult::Parent { child, master } => Ok(PtyChild {
            pid: child,
            master: owned_fd_to_file(master),
        }),
        ForkptyResult::Child => {
            let shell = CString::new(shell).unwrap_or_else(|_| CString::new("/bin/sh").unwrap());
            let argv = [shell.as_c_str()];
            let _ = execvp(&shell, &argv);
            std::process::exit(127);
        }
    }
}

fn owned_fd_to_file(fd: OwnedFd) -> File {
    File::from(fd)
}

pub(crate) fn child_has_exited(pid: nix::unistd::Pid) -> bool {
    matches!(
        waitpid(pid, Some(WaitPidFlag::WNOHANG)),
        Ok(WaitStatus::Exited(_, _)) | Ok(WaitStatus::Signaled(_, _, _)) | Err(_)
    )
}

pub(crate) fn set_pty_winsize(fd: i32, cols: u16, rows: u16) -> io::Result<()> {
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: ioctl receives a valid PTY file descriptor and a pointer to a
    // stack-allocated winsize for the duration of the call.
    let result = unsafe { nix::libc::ioctl(fd, nix::libc::TIOCSWINSZ, &winsize) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();

    if key.modifiers.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }

    match key.code {
        KeyCode::Backspace => bytes.push(0x7f),
        KeyCode::Enter => bytes.push(b'\r'),
        KeyCode::Left => bytes.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => bytes.extend_from_slice(b"\x1b[C"),
        KeyCode::Up => bytes.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => bytes.extend_from_slice(b"\x1b[B"),
        KeyCode::Home => bytes.extend_from_slice(b"\x1b[H"),
        KeyCode::End => bytes.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => bytes.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => bytes.extend_from_slice(b"\x1b[6~"),
        KeyCode::Tab | KeyCode::BackTab => bytes.push(b'\t'),
        KeyCode::Delete => bytes.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => bytes.extend_from_slice(b"\x1b[2~"),
        KeyCode::Esc => bytes.push(0x1b),
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(control) = ctrl_byte(ch) {
                bytes.push(control);
            }
        }
        KeyCode::Char(ch) => {
            let mut encoded = [0_u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
        }
        KeyCode::F(n) => bytes.extend_from_slice(function_key_sequence(n)?.as_bytes()),
        KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => return None,
    }

    Some(bytes)
}

fn key_text(key: &KeyEvent) -> Option<String> {
    match key.code {
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => Some(ch.to_string()),
        _ => None,
    }
}

fn ctrl_byte(ch: char) -> Option<u8> {
    let lower = ch.to_ascii_lowercase();
    if lower.is_ascii_alphabetic() {
        Some((lower as u8) - b'a' + 1)
    } else {
        match lower {
            ' ' | '@' | '2' => Some(0x00),
            '[' => Some(0x1b),
            '\\' => Some(0x1c),
            ']' => Some(0x1d),
            '^' => Some(0x1e),
            '_' => Some(0x1f),
            '?' => Some(0x7f),
            _ => None,
        }
    }
}

fn function_key_sequence(n: u8) -> Option<&'static str> {
    match n {
        1 => Some("\x1bOP"),
        2 => Some("\x1bOQ"),
        3 => Some("\x1bOR"),
        4 => Some("\x1bOS"),
        5 => Some("\x1b[15~"),
        6 => Some("\x1b[17~"),
        7 => Some("\x1b[18~"),
        8 => Some("\x1b[19~"),
        9 => Some("\x1b[20~"),
        10 => Some("\x1b[21~"),
        11 => Some("\x1b[23~"),
        12 => Some("\x1b[24~"),
        _ => None,
    }
}
