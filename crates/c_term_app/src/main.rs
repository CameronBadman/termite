use std::{ffi::CString, fs::File, io, os::fd::RawFd};

mod config;
mod plugins;
mod window_backend;

use nix::{
    pty::{ForkptyResult, Winsize, forkpty},
    unistd::execvp,
};

fn main() {
    if let Err(error) = window_backend::run() {
        eprintln!("c-term: {error}");
        std::process::exit(1);
    }
}

pub(crate) struct PtyChild {
    pub(crate) master: File,
}

pub(crate) fn spawn_shell(shell: &str, cols: u16, rows: u16) -> io::Result<PtyChild> {
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: the child immediately execs the shell or exits; the parent only keeps the PTY master.
    match unsafe { forkpty(&winsize, None) }.map_err(io::Error::other)? {
        ForkptyResult::Parent { master, .. } => Ok(PtyChild {
            master: File::from(master),
        }),
        ForkptyResult::Child => {
            let shell = CString::new(shell).unwrap_or_else(|_| CString::new("/bin/sh").unwrap());
            let _ = execvp(&shell, &[shell.as_c_str()]);
            std::process::exit(127);
        }
    }
}

pub(crate) fn set_pty_winsize(fd: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: ioctl receives a valid PTY fd and a live winsize pointer for the duration of the call.
    match unsafe { nix::libc::ioctl(fd, nix::libc::TIOCSWINSZ, &winsize) } {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}
