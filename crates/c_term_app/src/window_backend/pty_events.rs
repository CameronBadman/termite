use std::{
    io::{self, Read},
    sync::mpsc,
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

enum ReaderMessage {
    Bytes(Vec<u8>),
    Exited,
}

pub(super) fn spawn_pty_reader(
    child: &mut PtyChild,
    proxy: EventLoopProxy<UserEvent>,
) -> io::Result<()> {
    let mut reader = child.master.try_clone()?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0_u8; READ_BUFFER_BYTES];
        loop {
            let n = match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(0) | Err(_) => {
                    let _ = sender.send(ReaderMessage::Exited);
                    break;
                }
                Ok(n) => n,
            };
            if sender
                .send(ReaderMessage::Bytes(buffer[..n].to_vec()))
                .is_err()
            {
                break;
            }
        }
    });
    thread::spawn(move || {
        while let Ok(message) = receiver.recv() {
            let mut batch = match message {
                ReaderMessage::Bytes(bytes) => bytes,
                ReaderMessage::Exited => {
                    let _ = proxy.send_event(UserEvent::ChildExited);
                    break;
                }
            };
            let mut exited = false;
            let deadline = Instant::now() + COALESCE_WINDOW;
            while batch.len() < MAX_PTY_EVENT_BYTES {
                match receiver.try_recv() {
                    Ok(ReaderMessage::Bytes(mut bytes)) => batch.append(&mut bytes),
                    Ok(ReaderMessage::Exited) => {
                        exited = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        let now = Instant::now();
                        if now >= deadline {
                            break;
                        }
                        match receiver.recv_timeout(deadline.saturating_duration_since(now)) {
                            Ok(ReaderMessage::Bytes(mut bytes)) => batch.append(&mut bytes),
                            Ok(ReaderMessage::Exited) => {
                                exited = true;
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                exited = true;
                                break;
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        exited = true;
                        break;
                    }
                }
            }
            if proxy.send_event(UserEvent::PtyBytes(batch)).is_err() {
                break;
            }
            if exited {
                let _ = proxy.send_event(UserEvent::ChildExited);
                break;
            }
        }
    });
    Ok(())
}
