use std::{
    collections::VecDeque,
    io::{self, BufRead, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
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
    Presentation {
        visible: bool,
        request_id: u64,
        alpha: f32,
        margin: ShellSurfaceMargin,
    },
    Alpha(f32),
    Margin(ShellSurfaceMargin),
    Size(u32, u32),
    FrameRate(crate::ShellSurfaceFrameRate),
    Quit,
    ControlLine(String),
    Disconnected,
}

pub(super) struct MessageQueue {
    state: Mutex<MessageQueueState>,
    space_available: Condvar,
}

const MAX_QUEUED_MESSAGES: usize = 256;
const MAX_QUEUED_CONTROLS: usize = 256;
const MESSAGE_DISPATCH_BUDGET: usize = 32;

#[derive(Default)]
struct MessageQueueState {
    messages: VecDeque<OsrMessage>,
    wake_queued: bool,
}

impl MessageQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(MessageQueueState::default()),
            space_available: Condvar::new(),
        }
    }

    fn push(&self, message: OsrMessage) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        let mut state = state;
        while state.messages.len() >= MAX_QUEUED_MESSAGES {
            let Ok(next) = self.space_available.wait(state) else {
                return false;
            };
            state = next;
        }
        match message {
            OsrMessage::PaintBatch(incoming) => {
                let matching = state
                    .messages
                    .iter_mut()
                    .rev()
                    .take_while(|message| matches!(message, OsrMessage::PaintBatch(_)))
                    .find_map(|message| match message {
                        OsrMessage::PaintBatch(queued) if queued.surface == incoming.surface => {
                            Some(queued)
                        }
                        _ => None,
                    });
                let incoming = if let Some(queued) = matching {
                    merge_paint_batch(queued, incoming)
                } else {
                    Some(incoming)
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

    pub(super) fn drain_budgeted(&self) -> (VecDeque<OsrMessage>, bool) {
        let Ok(mut state) = self.state.lock() else {
            return (VecDeque::new(), false);
        };
        let count = state.messages.len().min(MESSAGE_DISPATCH_BUDGET);
        let messages = state.messages.drain(..count).collect();
        let remaining = !state.messages.is_empty();
        state.wake_queued = remaining;
        drop(state);
        self.space_available.notify_all();
        (messages, remaining)
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
    error: Option<String>,
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
                error: None,
            }),
            ready: Condvar::new(),
        });
        let worker_queue = Arc::clone(&queue);
        thread::spawn(move || {
            while let Some(message) = worker_queue.next() {
                let line = match message {
                    ControlMessage::Motion(line) | ControlMessage::Ordered(line) => line,
                };
                if let Err(error) = stream.write_all(line.as_bytes()) {
                    worker_queue.fail(error);
                    break;
                }
            }
        });
        Self { queue }
    }

    pub(super) fn send(&self, line: String) -> Result<(), String> {
        self.queue.push(ControlMessage::Ordered(line))
    }

    pub(super) fn send_motion(&self, line: String) -> Result<(), String> {
        let Ok(mut state) = self.queue.state.lock() else {
            return Err("control queue lock was poisoned".to_string());
        };
        if state.closed {
            return Err(state
                .error
                .clone()
                .unwrap_or_else(|| "control writer is closed".to_string()));
        }
        if state.messages.len() >= MAX_QUEUED_CONTROLS {
            if let Some(pending) = state
                .messages
                .iter_mut()
                .rev()
                .find_map(|message| match message {
                    ControlMessage::Motion(pending) => Some(pending),
                    ControlMessage::Ordered(_) => None,
                })
            {
                *pending = line;
            }
            return Ok(());
        }
        if let Some(ControlMessage::Motion(pending)) = state.messages.back_mut() {
            *pending = line;
        } else {
            state.messages.push_back(ControlMessage::Motion(line));
        }
        drop(state);
        self.queue.ready.notify_one();
        Ok(())
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
    fn push(&self, message: ControlMessage) -> Result<(), String> {
        let Ok(state) = self.state.lock() else {
            return Err("control queue lock was poisoned".to_string());
        };
        let mut state = state;
        while state.messages.len() >= MAX_QUEUED_CONTROLS && !state.closed {
            let Ok(next) = self.ready.wait(state) else {
                return Err("control queue lock was poisoned".to_string());
            };
            state = next;
        }
        if state.closed {
            return Err(state
                .error
                .clone()
                .unwrap_or_else(|| "control writer is closed".to_string()));
        }
        state.messages.push_back(message);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn next(&self) -> Option<ControlMessage> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(message) = state.messages.pop_front() {
                self.ready.notify_all();
                return Some(message);
            }
            if state.closed {
                return None;
            }
            state = self.ready.wait(state).ok()?;
        }
    }

    fn fail(&self, error: io::Error) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.error = Some(error.to_string());
        }
        self.ready.notify_all();
    }
}

pub(super) fn start_layer_parent_bridge_reader(sender: Sender<LayerHostEvent>) {
    thread::spawn(move || {
        let input = io::stdin();
        for line in input.lock().lines().map_while(std::result::Result::ok) {
            if let Some((visible, request_id, alpha, margin)) = parse_presentation_control(&line) {
                if sender
                    .send(LayerHostEvent::Presentation {
                        visible,
                        request_id,
                        alpha,
                        margin,
                    })
                    .is_err()
                {
                    break;
                }
                continue;
            }
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
            if let Some(frame_rate) = parse_frame_rate_control(&line) {
                if sender.send(LayerHostEvent::FrameRate(frame_rate)).is_err() {
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

fn parse_presentation_control(line: &str) -> Option<(bool, u64, f32, ShellSurfaceMargin)> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "presentation" {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(value).ok()?;
    let visible = value.get("visible")?.as_bool()?;
    let request_id = value.get("requestId")?.as_u64()?;
    let alpha = value.get("alpha")?.as_f64()? as f32;
    let margin = value.get("margin")?.as_array()?;
    let [top, right, bottom, left] = margin.as_slice() else {
        return None;
    };
    Some((
        visible,
        request_id,
        alpha.clamp(0.0, 1.0),
        ShellSurfaceMargin {
            top: i32::try_from(top.as_i64()?).ok()?,
            right: i32::try_from(right.as_i64()?).ok()?,
            bottom: i32::try_from(bottom.as_i64()?).ok()?,
            left: i32::try_from(left.as_i64()?).ok()?,
        },
    ))
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
            if let Err(error) = candidate.set_read_timeout(Some(Duration::from_millis(750))) {
                eprintln!("Sabine layer OSR could not set authentication deadline: {error}");
                continue;
            }
            match crate::osr::transport::authenticate(&mut candidate, &authentication_token) {
                Ok(crate::osr::transport::Authentication::Accepted) => {
                    if let Err(error) = candidate.set_read_timeout(None) {
                        eprintln!(
                            "Sabine layer OSR could not clear authentication deadline: {error}"
                        );
                        continue;
                    }
                    break candidate;
                }
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

fn parse_frame_rate_control(line: &str) -> Option<crate::ShellSurfaceFrameRate> {
    let (command, value) = crate::parse_host_control(line)?;
    if command != "shell-frame-rate" {
        return None;
    }
    crate::ShellSurfaceFrameRate::new(value.trim().parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::{parse_frame_rate_control, parse_size_control, parse_visibility_control};

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

    #[test]
    fn frame_rate_control_validates_bounds() {
        assert_eq!(
            parse_frame_rate_control("SABINE_HOST_CONTROL\tshell-frame-rate\t144")
                .map(|rate| rate.get()),
            Some(144)
        );
        assert!(parse_frame_rate_control("SABINE_HOST_CONTROL\tshell-frame-rate\t0").is_none());
    }
}
