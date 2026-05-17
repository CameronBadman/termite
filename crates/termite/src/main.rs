use std::{env, ffi::CString, fs::File, io, os::fd::RawFd};

mod config;
mod plugins;
mod runner;
mod theme;
mod window_backend;

use nix::{
    pty::{ForkptyResult, Winsize, forkpty},
    unistd::execvp,
};

use termite_core::{PROGRAM, PROGRAM_VERSION, TERM};

fn main() {
    if let Err(error) = profiler::run(|| config::runner().run()) {
        eprintln!("termite: {error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "profile")]
mod profiler {
    use std::{
        env,
        fs::{self, File},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use pprof::ProfilerGuard;

    pub(crate) fn run<R>(work: impl FnOnce() -> R) -> R {
        let Some(output) = profile_output() else {
            return work();
        };
        let frequency = env::var("TERMITE_PPROF_HZ")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(997);

        let guard = match ProfilerGuard::new(frequency) {
            Ok(guard) => Some(guard),
            Err(error) => {
                eprintln!("termite-profile: failed to start pprof sampler: {error}");
                None
            }
        };

        let result = work();

        if let Some(guard) = guard {
            if let Err(error) = write_flamegraph(guard, &output) {
                eprintln!(
                    "termite-profile: failed to write {}: {error}",
                    output.display()
                );
            } else {
                eprintln!("termite-profile flamegraph={}", output.display());
            }
        }

        result
    }

    fn profile_output() -> Option<PathBuf> {
        let value = env::var_os("TERMITE_PPROF")?;
        if value.is_empty() || value == "1" {
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            Some(PathBuf::from(format!(
                "target/profiles/termite-pprof-{seconds}.svg"
            )))
        } else {
            Some(PathBuf::from(value))
        }
    }

    fn write_flamegraph(guard: ProfilerGuard<'_>, output: &PathBuf) -> Result<(), String> {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let report = guard.report().build().map_err(|error| error.to_string())?;
        let file = File::create(output).map_err(|error| error.to_string())?;
        report.flamegraph(file).map_err(|error| error.to_string())
    }
}

#[cfg(not(feature = "profile"))]
mod profiler {
    pub(crate) fn run<R>(work: impl FnOnce() -> R) -> R {
        work()
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
            unsafe {
                env::set_var("TERM", TERM);
                env::set_var("TERM_PROGRAM", PROGRAM);
                env::set_var("TERM_PROGRAM_VERSION", PROGRAM_VERSION);
            }
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
