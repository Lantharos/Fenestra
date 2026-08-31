use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
};

use sabine_bridge::BridgeHandlers;
use sabine_platform::{PlatformEvent, ShellSurfaceMargin};

use super::request_dispatch::{BridgeIpcRequest, BridgeRequestDispatcher};
use crate::launch::browser::HOST_CONTROL_PREFIX;

#[derive(Clone)]
pub struct BridgeEventEmitter {
    targets: Arc<Mutex<Vec<BridgeTarget>>>,
    visibility_waiters: Arc<Mutex<HashMap<u64, VisibilityWaiter>>>,
}

static NEXT_VISIBILITY_REQUEST: AtomicU64 = AtomicU64::new(1);

struct VisibilityWaiter {
    window_id: u32,
    completion: crossbeam_channel::Sender<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellSurfaceVisibilityState {
    Pending,
    Mapped,
    Unmapped,
    Disconnected,
}

#[must_use = "poll the request state to observe when the compositor commit completes"]
pub struct ShellSurfaceVisibilityRequest {
    request_id: u64,
    requested_visible: bool,
    completion: crossbeam_channel::Receiver<bool>,
    observed: Mutex<Option<bool>>,
}

impl ShellSurfaceVisibilityRequest {
    pub fn id(&self) -> u64 {
        self.request_id
    }

    pub fn requested_visible(&self) -> bool {
        self.requested_visible
    }

    pub fn state(&self) -> ShellSurfaceVisibilityState {
        let Ok(mut observed) = self.observed.lock() else {
            return ShellSurfaceVisibilityState::Disconnected;
        };
        if let Some(mapped) = *observed {
            return visibility_state(mapped);
        }
        match self.completion.try_recv() {
            Ok(mapped) => {
                *observed = Some(mapped);
                visibility_state(mapped)
            }
            Err(crossbeam_channel::TryRecvError::Empty) => ShellSurfaceVisibilityState::Pending,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                ShellSurfaceVisibilityState::Disconnected
            }
        }
    }
}

fn visibility_state(mapped: bool) -> ShellSurfaceVisibilityState {
    if mapped {
        ShellSurfaceVisibilityState::Mapped
    } else {
        ShellSurfaceVisibilityState::Unmapped
    }
}

struct BridgeTarget {
    window_id: u32,
    writer: BridgeWriter,
}

#[derive(Clone)]
pub(crate) struct BridgeWriter {
    sender: crossbeam_channel::Sender<String>,
}

impl BridgeWriter {
    fn spawn(mut stdin: std::process::ChildStdin) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded::<String>(512);
        thread::spawn(move || {
            while let Ok(line) = receiver.recv() {
                if writeln!(stdin, "{line}").is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        Self { sender }
    }

    pub(super) fn try_send(
        &self,
        line: impl Into<String>,
    ) -> Result<(), crossbeam_channel::TrySendError<String>> {
        self.sender.try_send(line.into())
    }

    pub(super) fn send(&self, line: impl Into<String>) -> bool {
        self.sender.send(line.into()).is_ok()
    }
}

impl BridgeEventEmitter {
    pub fn emit(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        let event = BridgeIpcEvent {
            name: name.into(),
            payload,
        };
        self.write_line(event)
    }

    pub(crate) fn attach(&self, window_id: u32, writer: BridgeWriter) {
        if let Ok(mut targets) = self.targets.lock() {
            targets.retain(|target| target.window_id != window_id);
            targets.push(BridgeTarget { window_id, writer });
        }
    }

    pub(crate) fn detach(&self, window_id: u32) {
        if let Ok(mut targets) = self.targets.lock() {
            targets.retain(|target| target.window_id != window_id);
        }
        if let Ok(mut waiters) = self.visibility_waiters.lock() {
            waiters.retain(|_, waiter| waiter.window_id != window_id);
        }
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        self.emit_host_control("visible", if visible { "1" } else { "0" })
    }

    pub(crate) fn set_layer_visible(
        &self,
        window_id: u32,
        visible: bool,
    ) -> Option<ShellSurfaceVisibilityRequest> {
        let request_id = NEXT_VISIBILITY_REQUEST.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let Ok(mut waiters) = self.visibility_waiters.lock() else {
            return None;
        };
        waiters.insert(
            request_id,
            VisibilityWaiter {
                window_id,
                completion: sender,
            },
        );
        drop(waiters);
        if !self.emit_host_control_to(
            window_id,
            "visible",
            &format!("{}:{request_id}", u8::from(visible)),
        ) {
            if let Ok(mut waiters) = self.visibility_waiters.lock() {
                waiters.remove(&request_id);
            }
            return None;
        }
        Some(ShellSurfaceVisibilityRequest {
            request_id,
            requested_visible: visible,
            completion: receiver,
            observed: Mutex::new(None),
        })
    }

    pub(crate) fn set_layer_presentation(
        &self,
        window_id: u32,
        visible: bool,
        alpha: f32,
        margin: ShellSurfaceMargin,
    ) -> Option<ShellSurfaceVisibilityRequest> {
        let request_id = NEXT_VISIBILITY_REQUEST.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let Ok(mut waiters) = self.visibility_waiters.lock() else {
            return None;
        };
        waiters.insert(
            request_id,
            VisibilityWaiter {
                window_id,
                completion: sender,
            },
        );
        drop(waiters);
        let value = serde_json::json!({
            "visible": visible,
            "requestId": request_id,
            "alpha": alpha.clamp(0.0, 1.0),
            "margin": [margin.top, margin.right, margin.bottom, margin.left],
        });
        if !self.emit_host_control_to(window_id, "presentation", &value.to_string()) {
            if let Ok(mut waiters) = self.visibility_waiters.lock() {
                waiters.remove(&request_id);
            }
            return None;
        }
        Some(ShellSurfaceVisibilityRequest {
            request_id,
            requested_visible: visible,
            completion: receiver,
            observed: Mutex::new(None),
        })
    }

    pub fn set_alpha(&self, alpha: f32) -> bool {
        self.emit_host_control("alpha", &format!("{:.4}", alpha.clamp(0.0, 1.0)))
    }

    pub fn set_margin(&self, margin: ShellSurfaceMargin) -> bool {
        self.emit_host_control(
            "margin",
            &format!(
                "{},{},{},{}",
                margin.top, margin.right, margin.bottom, margin.left
            ),
        )
    }

    pub fn set_size(&self, width: u32, height: u32) -> bool {
        self.emit_host_control("size", &format!("{},{}", width.max(1), height.max(1)))
    }

    pub fn show(&self) -> bool {
        self.emit_host_control("show", "1")
    }

    pub fn hide(&self) -> bool {
        self.emit_host_control("hide", "1")
    }

    pub fn quit(&self) -> bool {
        self.emit_host_control("quit", "1")
    }

    pub fn focus_window(&self) -> bool {
        self.emit_host_control("focus", "1")
    }

    pub fn focus_window_with_activation_token(&self, token: Option<&str>) -> bool {
        self.emit_host_control(
            "focus",
            token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .unwrap_or("1"),
        )
    }

    /// Drive a guest surface from Rust. The payload is forwarded to the CEF
    /// host as `SABINE_HOST_CONTROL\tguest.<op>\t{json}`. Prefer the page
    /// `sabine.guest.*` bridge for UI-driven work; use this for host-owned
    /// guests (for example restoring a session before the page loads).
    pub fn guest_control(&self, control: &sabine_bridge::GuestHostControl) -> bool {
        self.emit_host_control(control.command_name(), &control.to_host_value().to_string())
    }

    pub(crate) fn emit_activity_update(&self, update: &sabine_bridge::ActivityHostUpdate) -> bool {
        let command = match update {
            sabine_bridge::ActivityHostUpdate::Begin(_) => "activity.begin",
            sabine_bridge::ActivityHostUpdate::End(_) => "activity.end",
        };
        self.emit_host_control(
            command,
            &sabine_bridge::host_update_json(update).to_string(),
        )
    }

    pub(crate) fn emit_host_control(&self, command: &str, value: &str) -> bool {
        self.write_line(format!("{HOST_CONTROL_PREFIX}\t{command}\t{value}"))
    }

    fn emit_host_control_to(&self, window_id: u32, command: &str, value: &str) -> bool {
        let Ok(targets) = self.targets.lock() else {
            return false;
        };
        let Some(target) = targets.iter().find(|target| target.window_id == window_id) else {
            return false;
        };
        target
            .writer
            .try_send(format!("{HOST_CONTROL_PREFIX}\t{command}\t{value}"))
            .is_ok()
    }

    fn write_line(&self, line: impl std::fmt::Display) -> bool {
        let Ok(mut targets) = self.targets.lock() else {
            return false;
        };
        if targets.is_empty() {
            return false;
        }
        let message = line.to_string();
        let mut delivered = false;
        targets.retain(|target| match target.writer.try_send(message.clone()) {
            Ok(()) => {
                delivered = true;
                true
            }
            Err(crossbeam_channel::TrySendError::Full(_)) => true,
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
        });
        delivered
    }
}

impl sabine_bridge::ActivityEventEmitter for BridgeEventEmitter {
    fn emit_activity_update(&self, update: &sabine_bridge::ActivityHostUpdate) -> bool {
        BridgeEventEmitter::emit_activity_update(self, update)
    }
}

pub(crate) fn platform_event_payload(event: PlatformEvent) -> (&'static str, serde_json::Value) {
    match event {
        PlatformEvent::Tray(activation) => (
            "tray.activate",
            serde_json::json!({
                "trayId": activation.tray_id,
                "itemId": activation.item_id,
                "action": activation.action,
            }),
        ),
        PlatformEvent::GlobalShortcut(activation) => (
            "globalShortcut.activate",
            serde_json::json!({
                "id": activation.id,
                "action": activation.action,
                "activationToken": activation.activation_token,
            }),
        ),
        PlatformEvent::SingleInstance(activation) => (
            "singleInstance.activate",
            serde_json::json!({
                "policy": format!("{:?}", activation.policy),
                "arguments": activation.arguments,
                "workingDirectory": activation.working_directory,
                "activationToken": activation.activation_token,
            }),
        ),
    }
}

pub(crate) fn prepare_bridge_command(command: &mut Command, _bridge_handlers: &BridgeHandlers) {
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
}

pub(crate) struct BridgeDispatch {
    pub(crate) thread: Option<JoinHandle<()>>,
    pub(crate) emitter: Option<BridgeEventEmitter>,
    pub(crate) ready: crossbeam_channel::Receiver<()>,
}

pub(crate) fn spawn_bridge_dispatch(
    child: &mut Child,
    bridge_runtime: sabine_bridge::BridgeRuntime,
    activity: sabine_bridge::ActivityRegistry,
) -> BridgeDispatch {
    let (ready_sender, ready) = crossbeam_channel::bounded(1);
    let Some(stdin) = child.stdin.take() else {
        return BridgeDispatch {
            thread: None,
            emitter: None,
            ready,
        };
    };
    let writer = BridgeWriter::spawn(stdin);
    let window_id = child.id();
    let emitter = BridgeEventEmitter {
        targets: Arc::new(Mutex::new(vec![BridgeTarget {
            window_id,
            writer: writer.clone(),
        }])),
        visibility_waiters: Arc::new(Mutex::new(HashMap::new())),
    };
    let Some(stdout) = child.stdout.take() else {
        return BridgeDispatch {
            thread: None,
            emitter: Some(emitter),
            ready,
        };
    };
    let dispatcher =
        BridgeRequestDispatcher::new(bridge_runtime, activity, emitter.clone(), writer);
    let detach_emitter = emitter.clone();
    let visibility_waiters = Arc::clone(&emitter.visibility_waiters);
    let thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            if line == "SABINE_OSR_READY" {
                let _ = ready_sender.try_send(());
                continue;
            }
            if acknowledge_visibility(&line, window_id, &visibility_waiters) {
                continue;
            }
            let Some(request) = BridgeIpcRequest::parse(&line) else {
                continue;
            };
            dispatcher.submit(request);
        }
        detach_emitter.detach(window_id);
    });
    BridgeDispatch {
        thread: Some(thread),
        emitter: Some(emitter),
        ready,
    }
}

/// Attach another OSR-host child to an existing bridge emitter and dispatch
/// that window's bridge requests through the same handlers.
pub(crate) fn spawn_bridge_dispatch_for_window(
    child: &mut Child,
    bridge_runtime: sabine_bridge::BridgeRuntime,
    activity: sabine_bridge::ActivityRegistry,
    emitter: &BridgeEventEmitter,
) -> Option<JoinHandle<()>> {
    let writer = BridgeWriter::spawn(child.stdin.take()?);
    let window_id = child.id();
    emitter.attach(window_id, writer.clone());
    let stdout = child.stdout.take()?;
    let activity_emitter = emitter.clone();
    let visibility_waiters = Arc::clone(&emitter.visibility_waiters);
    let dispatcher =
        BridgeRequestDispatcher::new(bridge_runtime, activity, emitter.clone(), writer);
    Some(thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            if acknowledge_visibility(&line, window_id, &visibility_waiters) {
                continue;
            }
            let Some(request) = BridgeIpcRequest::parse(&line) else {
                continue;
            };
            dispatcher.submit(request);
        }
        activity_emitter.detach(window_id);
    }))
}

fn acknowledge_visibility(
    line: &str,
    window_id: u32,
    waiters: &Mutex<HashMap<u64, VisibilityWaiter>>,
) -> bool {
    let mut parts = line.split('\t');
    if parts.next() != Some("SABINE_LAYER_VISIBILITY") {
        return false;
    }
    let Some(request_id) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
        return true;
    };
    let mapped = parts.next() == Some("mapped");
    if let Ok(mut waiters) = waiters.lock()
        && waiters
            .get(&request_id)
            .is_some_and(|waiter| waiter.window_id == window_id)
        && let Some(waiter) = waiters.remove(&request_id)
    {
        let _ = waiter.completion.try_send(mapped);
    }
    true
}

pub(crate) fn parse_host_control(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.splitn(3, '\t');
    if parts.next()? != HOST_CONTROL_PREFIX {
        return None;
    }
    Some((parts.next()?, parts.next().unwrap_or("1")))
}

struct BridgeIpcEvent {
    name: String,
    payload: serde_json::Value,
}

impl std::fmt::Display for BridgeIpcEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = serde_json::to_string(&self.name).unwrap_or_else(|_| "\"event\"".to_string());
        let payload = serde_json::to_string(&self.payload).unwrap_or_else(|_| "null".to_string());
        write!(formatter, "SABINE_BRIDGE_EVENT\t{name}\t{payload}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn emitter_with_reader() -> (Child, BridgeEventEmitter) {
        let mut command = if cfg!(target_os = "windows") {
            let mut command = Command::new("cmd");
            command.args(["/d", "/c", "more"]);
            command
        } else {
            Command::new("cat")
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("stdin reader");
        let writer = BridgeWriter::spawn(child.stdin.take().expect("stdin"));
        let emitter = BridgeEventEmitter {
            targets: Arc::new(Mutex::new(vec![BridgeTarget {
                window_id: child.id(),
                writer,
            }])),
            visibility_waiters: Arc::new(Mutex::new(HashMap::new())),
        };
        (child, emitter)
    }

    #[test]
    fn emitter_detach_stops_broadcast() {
        let (mut child, emitter) = emitter_with_reader();
        assert!(emitter.emit("ping", serde_json::json!({})));
        emitter.detach(child.id());
        assert!(!emitter.emit("ping", serde_json::json!({})));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn layer_visibility_is_queued_and_completed_asynchronously() {
        let (mut child, emitter) = emitter_with_reader();
        let request = emitter
            .set_layer_visible(child.id(), true)
            .expect("visibility request queued");
        assert!(request.requested_visible());
        assert_eq!(request.state(), ShellSurfaceVisibilityState::Pending);
        assert!(acknowledge_visibility(
            &format!("SABINE_LAYER_VISIBILITY\t{}\tmapped", request.id()),
            child.id(),
            &emitter.visibility_waiters,
        ));
        assert_eq!(request.state(), ShellSurfaceVisibilityState::Mapped);
        assert_eq!(request.state(), ShellSurfaceVisibilityState::Mapped);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn detaching_a_layer_disconnects_pending_visibility_requests() {
        let (mut child, emitter) = emitter_with_reader();
        let request = emitter
            .set_layer_visible(child.id(), false)
            .expect("visibility request queued");
        emitter.detach(child.id());
        assert_eq!(request.state(), ShellSurfaceVisibilityState::Disconnected);
        let _ = child.kill();
        let _ = child.wait();
    }
}
