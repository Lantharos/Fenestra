use std::{sync::mpsc, thread};

use winit::event_loop::EventLoopProxy;

use crate::osr::protocol::read_message;
use crate::osr::transport::{IpcEndpoint, IpcListener};

use super::types::OsrHostEvent;

pub(super) fn start_socket_reader(
    generation: u64,
    listener: IpcListener,
    endpoint: IpcEndpoint,
    authentication_token: String,
    sender: mpsc::SyncSender<OsrHostEvent>,
    proxy: EventLoopProxy,
) {
    thread::spawn(move || {
        let mut stream = loop {
            let Ok((mut candidate, _)) = listener.accept() else {
                endpoint.unlink();
                let _ = sender.send(OsrHostEvent::Disconnected(generation));
                proxy.wake_up();
                return;
            };
            match crate::osr::transport::authenticate(&mut candidate, &authentication_token) {
                Ok(crate::osr::transport::Authentication::Accepted) => break candidate,
                Ok(crate::osr::transport::Authentication::Probe) => continue,
                Err(error) => {
                    eprintln!("Sabine OSR reject connect: {error}");
                }
            }
        };
        let Ok(writer) = stream.try_clone() else {
            endpoint.unlink();
            let _ = sender.send(OsrHostEvent::Disconnected(generation));
            proxy.wake_up();
            return;
        };
        let _ = sender.send(OsrHostEvent::Connected(generation, writer));
        proxy.wake_up();
        loop {
            match read_message(&mut stream) {
                Ok(Some(message)) => {
                    if sender
                        .send(OsrHostEvent::Message(generation, message))
                        .is_err()
                    {
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
                    eprintln!("Sabine OSR socket read failed: {error}");
                    break;
                }
            }
        }
        endpoint.unlink();
        let _ = sender.send(OsrHostEvent::Disconnected(generation));
        proxy.wake_up();
    });
}
