use std::{
    io::{self, Read},
    os::fd::{AsRawFd, RawFd},
    thread,
    time::{Duration, Instant},
};

use winit::event_loop::EventLoopProxy;

use crate::PtyChild;

const READ_BUFFER_BYTES: usize = 64 * 1024;
const MAX_PTY_EVENT_BYTES: usize = 1024 * 1024;
const COALESCE_WINDOW: Duration = Duration::from_millis(1);

#[derive(Debug)]
pub(super) enum UserEvent {
    PtyBytes(Vec<u8>),
    ChildExited,
}

pub(super) fn spawn_pty_reader(
    child: &mut PtyChild,
    proxy: EventLoopProxy<UserEvent>,
) -> io::Result<()> {
    let mut reader = child.master.try_clone()?;
    thread::spawn(move || {
        let fd = reader.as_raw_fd();
        let mut buffer = [0_u8; READ_BUFFER_BYTES];
        let mut batch = Vec::with_capacity(READ_BUFFER_BYTES);
        loop {
            if batch.is_empty() {
                match wait_readable(fd, None) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(_) => {
                        let _ = proxy.send_event(UserEvent::ChildExited);
                        break;
                    }
                }
            }

            match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) | Ok(0) => {
                    let _ = proxy.send_event(UserEvent::ChildExited);
                    break;
                }
                Ok(n) => batch.extend_from_slice(&buffer[..n]),
            }

            let deadline = Instant::now() + COALESCE_WINDOW;
            while batch.len() < MAX_PTY_EVENT_BYTES {
                match wait_readable(fd, Some(deadline.saturating_duration_since(Instant::now()))) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(_) => {
                        let _ = proxy.send_event(UserEvent::ChildExited);
                        return;
                    }
                }

                match reader.read(&mut buffer) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) | Ok(0) => {
                        if !batch.is_empty()
                            && proxy
                                .send_event(UserEvent::PtyBytes(std::mem::take(&mut batch)))
                                .is_err()
                        {
                            return;
                        }
                        let _ = proxy.send_event(UserEvent::ChildExited);
                        return;
                    }
                    Ok(n) => {
                        batch.extend_from_slice(&buffer[..n]);
                        if Instant::now() >= deadline {
                            break;
                        }
                    }
                }
            }

            if proxy
                .send_event(UserEvent::PtyBytes(std::mem::take(&mut batch)))
                .is_err()
            {
                break;
            }
        }
    });
    Ok(())
}

fn wait_readable(fd: RawFd, timeout: Option<Duration>) -> io::Result<bool> {
    let timeout_ms = timeout.map_or(-1, duration_to_poll_timeout_ms);
    loop {
        let mut pollfd = nix::libc::pollfd {
            fd,
            events: nix::libc::POLLIN | nix::libc::POLLHUP | nix::libc::POLLERR,
            revents: 0,
        };
        // SAFETY: poll receives a valid pointer to one pollfd for the duration of the call.
        let result = unsafe { nix::libc::poll(&mut pollfd, 1, timeout_ms) };
        match result {
            0 => return Ok(false),
            n if n > 0 => return Ok(true),
            _ => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

fn duration_to_poll_timeout_ms(duration: Duration) -> i32 {
    if duration.is_zero() {
        0
    } else {
        duration.as_millis().saturating_add(1).min(i32::MAX as u128) as i32
    }
}
