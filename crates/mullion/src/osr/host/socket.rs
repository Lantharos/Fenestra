use std::{sync::mpsc, thread};

use winit::event_loop::EventLoopProxy;

use crate::osr::protocol::read_message;
use crate::osr::transport::IpcListener;

use super::types::OsrHostEvent;

pub(super) fn start_socket_reader(
    listener: IpcListener,
    authentication_token: String,
    sender: mpsc::Sender<OsrHostEvent>,
    proxy: EventLoopProxy,
) {
    thread::spawn(move || {
        let mut stream = loop {
            let Ok((mut candidate, _)) = listener.accept() else {
                return;
            };
            if crate::osr::transport::authenticate(&mut candidate, &authentication_token).is_ok() {
                break candidate;
            }
        };
        if let Ok(writer) = stream.try_clone() {
            let _ = sender.send(OsrHostEvent::Connected(writer));
            proxy.wake_up();
        }
        loop {
            match read_message(&mut stream) {
                Ok(Some(message)) => {
                    if sender.send(OsrHostEvent::Message(message)).is_err() {
                        break;
                    }
                    proxy.wake_up();
                }
                Ok(None) => break,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    break;
                }
                Err(error) => {
                    eprintln!("Mullion OSR socket read failed: {error}");
                    break;
                }
            }
        }
        let _ = sender.send(OsrHostEvent::Disconnected);
        proxy.wake_up();
    });
}
