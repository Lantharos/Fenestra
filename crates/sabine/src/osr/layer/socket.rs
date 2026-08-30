use std::{
    collections::VecDeque,
    io::{self, BufRead, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use layershellev::calloop::channel::Sender;

use crate::ShellSurfaceMargin;
use crate::osr::protocol::{OsrMessage, OsrPaintBatch, read_message};

pub(super) enum LayerHostEvent {
    Connected(UnixStream),
    MessagesReady(Arc<MessageQueue>),
    Visible {
        visible: bool,
        request_id: Option<u64>,
    },
    Alpha(f32),
    Margin(ShellSurfaceMargin),
    Size(u32, u32),
    Quit,
    ControlLine(String),
    Disconnected,
}

pub(super) struct MessageQueue {
    state: Mutex<MessageQueueState>,
}

#[derive(Default)]
struct MessageQueueState {
    messages: VecDeque<OsrMessage>,
    wake_queued: bool,
}

impl MessageQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(MessageQueueState::default()),
        }
    }

    fn push(&self, message: OsrMessage) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        match message {
            OsrMessage::PaintBatch(incoming) => {
                let incoming = match state.messages.back_mut() {
                    Some(OsrMessage::PaintBatch(queued)) => merge_paint_batch(queued, incoming),
                    _ => Some(incoming),
                };
                if let Some(incoming) = incoming {
                    state.messages.push_back(OsrMessage::PaintBatch(incoming));
                }
            }
            message => state.messages.push_back(message),
        }
        if state.wake_queued {
            false
        } else {
            state.wake_queued = true;
            true
        }
    }

    pub(super) fn drain(&self) -> VecDeque<OsrMessage> {
        let Ok(mut state) = self.state.lock() else {
            return VecDeque::new();
        };
        state.wake_queued = false;
        std::mem::take(&mut state.messages)
    }
}

fn merge_paint_batch(queued: &mut OsrPaintBatch, incoming: OsrPaintBatch) -> Option<OsrPaintBatch> {
    if queued.surface != incoming.surface
        || queued.width != incoming.width
        || queued.height != incoming.height
    {
        return Some(incoming);
    }
    queued.x = incoming.x;
    queued.y = incoming.y;
    for frame in incoming.frames {
        queued
            .frames
            .retain(|queued_frame| !frame_covers(&frame, queued_frame));
        queued.frames.push(frame);
    }
    None
}

fn frame_covers(
    newer: &crate::osr::protocol::OsrFrame,
    older: &crate::osr::protocol::OsrFrame,
) -> bool {
    let newer_right = i64::from(newer.x) + i64::from(newer.width);
    let newer_bottom = i64::from(newer.y) + i64::from(newer.height);
    let older_right = i64::from(older.x) + i64::from(older.width);
    let older_bottom = i64::from(older.y) + i64::from(older.height);
    newer.x <= older.x
        && newer.y <= older.y
        && newer_right >= older_right
        && newer_bottom >= older_bottom
}

pub(super) struct ControlWriter {
    queue: Arc<ControlQueue>,
}

struct ControlQueue {
    state: Mutex<ControlQueueState>,
    ready: Condvar,
}

struct ControlQueueState {
    messages: VecDeque<ControlMessage>,
    closed: bool,
}

enum ControlMessage {
    Motion(String),
    Ordered(String),
}

impl ControlWriter {
    pub(super) fn start(mut stream: UnixStream) -> Self {
        let queue = Arc::new(ControlQueue {
            state: Mutex::new(ControlQueueState {
                messages: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        thread::spawn(move || {
            while let Some(message) = worker_queue.next() {
                let line = match message {
                    ControlMessage::Motion(line) | ControlMessage::Ordered(line) => line,
                };
                if stream.write_all(line.as_bytes()).is_err() {
                    break;
                }
            }
        });
        Self { queue }
    }

    pub(super) fn send(&self, line: String) {
        self.queue.push(ControlMessage::Ordered(line));
    }

    pub(super) fn send_motion(&self, line: String) {
        let Ok(mut state) = self.queue.state.lock() else {
            return;
        };
        if state.closed {
            return;
        }
        if let Some(ControlMessage::Motion(pending)) = state.messages.back_mut() {
            *pending = line;
        } else {
            state.messages.push_back(ControlMessage::Motion(line));
        }
        drop(state);
        self.queue.ready.notify_one();
    }
}

impl Drop for ControlWriter {
    fn drop(&mut self) {
        if let Ok(mut state) = self.queue.state.lock() {
            state.closed = true;
        }
        self.queue.ready.notify_one();
    }
}

impl ControlQueue {
    fn push(&self, message: ControlMessage) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.closed {
            return;
        }
        state.messages.push_back(message);
        drop(state);
        self.ready.notify_one();
    }

    fn next(&self) -> Option<ControlMessage> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(message) = state.messages.pop_front() {
                return Some(message);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).ok()?;
        }
    }
}

pub(super) fn start_layer_parent_bridge_reader(sender: Sender<LayerHostEvent>) {
    thread::spawn(move || {
        let input = io::stdin();
        for line in input.lock().lines().map_while(std::result::Result::ok) {
            if let Some((visible, request_id)) = parse_visibility_control(&line) {
                if sender
                    .send(LayerHostEvent::Visible {
                        visible,
                        request_id,
                    })
                    .is_err()
                {
                    break;
                }
                continue;
            }
            if let Some(alpha) = parse_alpha_control(&line) {
                if sender.send(LayerHostEvent::Alpha(alpha)).is_err() {
                    break;
                }
                continue;
            }
            if let Some(margin) = parse_margin_control(&line) {
                if sender.send(LayerHostEvent::Margin(margin)).is_err() {
                    break;
                }
                continue;
            }
            if let Some((width, height)) = parse_size_control(&line) {
                if sender.send(LayerHostEvent::Size(width, height)).is_err() {
                    break;
                }
                continue;
            }
            if parse_quit_control(&line) {
                let _ = sender.send(LayerHostEvent::Quit);
                break;
            }
            if sender.send(LayerHostEvent::ControlLine(line)).is_err() {
                break;
            }
        }
    });
}

pub(super) fn open_socket_reader(
    sender: Sender<LayerHostEvent>,
    authentication_token: String,
    app_id: &str,
) -> Option<PathBuf> {
    let (endpoint, listener) = match crate::osr::transport::IpcEndpoint::bind(app_id) {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("failed to bind Sabine layer OSR socket: {error}");
            return None;
        }
    };
    let crate::osr::transport::IpcEndpoint::Unix(ref socket_path) = endpoint;
    let path = socket_path.clone();
    start_socket_reader(listener, endpoint, authentication_token, sender);
    Some(path)
}

fn start_socket_reader(
    listener: UnixListener,
    endpoint: crate::osr::transport::IpcEndpoint,
    authentication_token: String,
    sender: Sender<LayerHostEvent>,
) {
    thread::spawn(move || {
        let messages = Arc::new(MessageQueue::new());
        let mut stream = loop {
            let Ok((mut candidate, _)) = listener.accept() else {
                endpoint.unlink();
                return;
            };
            match crate::osr::transport::authenticate(&mut candidate, &authentication_token) {
                Ok(crate::osr::transport::Authentication::Accepted) => break candidate,
                Ok(crate::osr::transport::Authentication::Probe) => continue,
                Err(error) => {
                    eprintln!("Sabine layer OSR reject connect: {error}");
                }
            }
        };
        if let Ok(writer) = stream.try_clone() {
            let _ = sender.send(LayerHostEvent::Connected(writer));
        }
        loop {
            match read_message(&mut stream) {
                Ok(Some(message)) => {
                    if messages.push(message)
                        && sender
                            .send(LayerHostEvent::MessagesReady(Arc::clone(&messages)))
                            .is_err()
                    {
                        break;
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
                    eprintln!("Sabine layer OSR socket read failed: {error}");
                    break;
                }
            }
        }
        endpoint.unlink();
        let _ = stream.shutdown(Shutdown::Both);
        let _ = sender.send(LayerHostEvent::Disconnected);
    });
}

fn parse_visibility_control(line: &str) -> Option<(bool, Option<u64>)> {
    let (command, value) = crate::parse_host_control(line)?;
    match command {
        "visible" => {
            let (value, request_id) = value
                .split_once(':')
                .map(|(value, request_id)| (value, request_id.parse::<u64>().ok()))
                .unwrap_or((value, None));
            match value {
                "1" | "true" | "yes" | "show" | "visible" => Some((true, request_id)),
                "0" | "false" | "no" | "hide" | "hidden" => Some((false, request_id)),
                _ => None,
            }
        }
        "show" | "focus" => Some((true, None)),
        "hide" => Some((false, None)),
        _ => None,
    }
}

fn parse_quit_control(line: &str) -> bool {
    crate::parse_host_control(line).is_some_and(|(command, _)| command == "quit")
}

fn parse_alpha_control(line: &str) -> Option<f32> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "alpha" {
        return None;
    }
    value.parse::<f32>().ok().map(|alpha| alpha.clamp(0.0, 1.0))
}

fn parse_margin_control(line: &str) -> Option<ShellSurfaceMargin> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "margin" {
        return None;
    }
    let mut parts = value.split(',').map(str::trim);
    let top = parts.next()?.parse::<i32>().ok()?;
    let right = parts.next()?.parse::<i32>().ok()?;
    let bottom = parts.next()?.parse::<i32>().ok()?;
    let left = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ShellSurfaceMargin {
        top,
        right,
        bottom,
        left,
    })
}

fn parse_size_control(line: &str) -> Option<(u32, u32)> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "size" {
        return None;
    }
    let (width, height) = value.split_once(',')?;
    Some((
        width.trim().parse::<u32>().ok()?.max(1),
        height.trim().parse::<u32>().ok()?.max(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::{parse_size_control, parse_visibility_control};

    #[test]
    fn visibility_control_preserves_completion_request_id() {
        assert_eq!(
            parse_visibility_control("SABINE_HOST_CONTROL\tvisible\t1:42"),
            Some((true, Some(42)))
        );
        assert_eq!(
            parse_visibility_control("SABINE_HOST_CONTROL\tvisible\t0"),
            Some((false, None))
        );
    }

    #[test]
    fn size_control_clamps_empty_dimensions() {
        assert_eq!(
            parse_size_control("SABINE_HOST_CONTROL\tsize\t228,363"),
            Some((228, 363))
        );
        assert_eq!(
            parse_size_control("SABINE_HOST_CONTROL\tsize\t0,0"),
            Some((1, 1))
        );
    }
}
