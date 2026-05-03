use std::{
    io::{self, Read},
    thread,
};

use winit::event_loop::EventLoopProxy;

use crate::PtyChild;

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
