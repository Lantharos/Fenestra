use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use winit::event_loop::EventLoopProxy;

use crate::osr::message_queue::MessageQueue;
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
        let messages = Arc::new(MessageQueue::new());
        let mut stream = loop {
            let Ok((mut candidate, _)) = listener.accept() else {
                endpoint.unlink();
                let _ = sender.send(OsrHostEvent::Disconnected(generation));
                proxy.wake_up();
                return;
            };
            if let Err(error) = candidate.set_read_timeout(Some(Duration::from_millis(750))) {
                eprintln!("Sabine OSR could not set authentication deadline: {error}");
                continue;
            }
            match crate::osr::transport::authenticate(&mut candidate, &authentication_token) {
                Ok(crate::osr::transport::Authentication::Accepted) => {
                    if let Err(error) = candidate.set_read_timeout(None) {
                        eprintln!("Sabine OSR could not clear authentication deadline: {error}");
                        continue;
                    }
                    break candidate;
                }
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
                    if messages.push(message) {
                        if sender
                            .send(OsrHostEvent::MessagesReady(
                                generation,
                                Arc::clone(&messages),
                            ))
                            .is_err()
                        {
                            break;
                        }
                        proxy.wake_up();
                    }
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
