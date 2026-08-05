use std::{
    io::{self, BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use mullion_bridge::{BridgeCommand, BridgeHandlers, BridgeResult};
use mullion_platform::{PlatformEvent, ShellSurfaceMargin};

use crate::browser::HOST_CONTROL_PREFIX;

#[derive(Clone)]
pub struct BridgeEventEmitter {
    stdin: Arc<Mutex<std::process::ChildStdin>>,
}

impl BridgeEventEmitter {
    pub fn emit(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        let event = BridgeIpcEvent {
            name: name.into(),
            payload,
        };
        self.write_line(event)
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        self.emit_host_control("visible", if visible { "1" } else { "0" })
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

    pub fn show(&self) -> bool {
        self.emit_host_control("show", "1")
    }

    pub fn hide(&self) -> bool {
        self.emit_host_control("hide", "1")
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
    /// host as `MULLION_HOST_CONTROL\tguest.<op>\t{json}`. Prefer the page
    /// `mullion.guest.*` bridge for UI-driven work; use this for host-owned
    /// guests (for example restoring a session before the page loads).
    pub fn guest_control(&self, control: &mullion_bridge::GuestHostControl) -> bool {
        self.emit_host_control(control.command_name(), &control.to_host_value().to_string())
    }

    pub(crate) fn emit_activity_update(&self, update: &mullion_bridge::ActivityHostUpdate) -> bool {
        let command = match update {
            mullion_bridge::ActivityHostUpdate::Begin(_) => "activity.begin",
            mullion_bridge::ActivityHostUpdate::End(_) => "activity.end",
        };
        self.emit_host_control(
            command,
            &mullion_bridge::host_update_json(update).to_string(),
        )
    }

    fn emit_host_control(&self, command: &str, value: &str) -> bool {
        self.write_line(format!("{HOST_CONTROL_PREFIX}\t{command}\t{value}"))
    }

    fn write_line(&self, line: impl std::fmt::Display) -> bool {
        let Ok(mut stdin) = self.stdin.lock() else {
            return false;
        };
        if writeln!(stdin, "{line}").is_err() {
            return false;
        }
        stdin.flush().is_ok()
    }
}

impl mullion_bridge::ActivityEventEmitter for BridgeEventEmitter {
    fn emit_activity_update(&self, update: &mullion_bridge::ActivityHostUpdate) -> bool {
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
    command.stderr(Stdio::null());
}

pub(crate) struct BridgeDispatch {
    pub(crate) thread: Option<JoinHandle<()>>,
    pub(crate) emitter: Option<BridgeEventEmitter>,
}

pub(crate) fn spawn_bridge_dispatch(
    child: &mut Child,
    bridge_runtime: mullion_bridge::BridgeRuntime,
    activity: mullion_bridge::ActivityRegistry,
) -> BridgeDispatch {
    let Some(stdin) = child.stdin.take() else {
        return BridgeDispatch {
            thread: None,
            emitter: None,
        };
    };
    let stdin = Arc::new(Mutex::new(stdin));
    let emitter = Some(BridgeEventEmitter {
        stdin: Arc::clone(&stdin),
    });
    let Some(stdout) = child.stdout.take() else {
        return BridgeDispatch {
            thread: None,
            emitter,
        };
    };
    let activity_emitter = emitter.clone();
    let thread = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(std::result::Result::ok) {
            let Some(request) = BridgeIpcRequest::parse(&line) else {
                continue;
            };
            let response = if let Some((response, update)) =
                activity.dispatch_bridge_command(&request.command)
            {
                if let (Ok(_), Some(update), Some(emitter)) = (
                    response.as_ref(),
                    update.as_ref(),
                    activity_emitter.as_ref(),
                ) {
                    let _ = emitter.emit_activity_update(update);
                }
                response
            } else {
                bridge_runtime.dispatch(request.command)
            };
            let line = BridgeIpcResponse::from_result(request.browser_id, request.id, response);
            let Ok(mut stdin) = stdin.lock() else {
                break;
            };
            if writeln!(stdin, "{line}").is_err() {
                break;
            }
            let _ = stdin.flush();
        }
    });
    BridgeDispatch {
        thread: Some(thread),
        emitter,
    }
}

pub(crate) fn spawn_native_host_bridge_proxy<F>(child: &mut Child, mut host_control: F)
where
    F: FnMut(String, String) + Send + 'static,
{
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut output = io::stdout();
            for line in reader.lines().map_while(std::result::Result::ok) {
                if writeln!(output, "{line}").is_err() {
                    break;
                }
                let _ = output.flush();
            }
        });
    }

    if let Some(mut stdin) = child.stdin.take() {
        thread::spawn(move || {
            let input = io::stdin();
            for line in input.lock().lines().map_while(std::result::Result::ok) {
                if let Some((command, value)) = parse_host_control(&line) {
                    host_control(command.to_string(), value.to_string());
                    continue;
                }
                if line.starts_with(HOST_CONTROL_PREFIX) {
                    continue;
                }
                if writeln!(stdin, "{line}").is_err() {
                    break;
                }
                let _ = stdin.flush();
            }
        });
    }
}

pub(crate) fn parse_host_control(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.splitn(3, '\t');
    if parts.next()? != HOST_CONTROL_PREFIX {
        return None;
    }
    Some((parts.next()?, parts.next().unwrap_or("1")))
}

struct BridgeIpcRequest {
    browser_id: String,
    id: String,
    command: BridgeCommand,
}

impl BridgeIpcRequest {
    fn parse(line: &str) -> Option<Self> {
        let parts = line.splitn(6, '\t').collect::<Vec<_>>();
        if parts.first().copied()? != "MULLION_BRIDGE_REQUEST" || parts.len() != 6 {
            return None;
        }
        let params = serde_json::from_str(parts[5]).ok()?;
        Some(Self {
            browser_id: parts[1].to_string(),
            id: parts[2].to_string(),
            command: BridgeCommand {
                origin: Some(parts[3].to_string()).filter(|origin| !origin.is_empty()),
                name: parts[4].to_string(),
                params,
            },
        })
    }
}

struct BridgeIpcResponse {
    browser_id: String,
    id: String,
    ok: bool,
    payload: serde_json::Value,
}

impl BridgeIpcResponse {
    fn from_result(browser_id: String, id: String, result: BridgeResult) -> Self {
        match result {
            Ok(response) => Self {
                browser_id,
                id,
                ok: true,
                payload: response.result,
            },
            Err(error) => Self {
                browser_id,
                id,
                ok: false,
                payload: serde_json::json!({ "message": error.message }),
            },
        }
    }
}

impl std::fmt::Display for BridgeIpcResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.ok { "ok" } else { "error" };
        let payload = serde_json::to_string(&self.payload).unwrap_or_else(|_| "null".to_string());
        write!(
            formatter,
            "MULLION_BRIDGE_RESPONSE\t{}\t{}\t{status}\t{payload}",
            self.browser_id, self.id
        )
    }
}

struct BridgeIpcEvent {
    name: String,
    payload: serde_json::Value,
}

impl std::fmt::Display for BridgeIpcEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = serde_json::to_string(&self.name).unwrap_or_else(|_| "\"event\"".to_string());
        let payload = serde_json::to_string(&self.payload).unwrap_or_else(|_| "null".to_string());
        write!(formatter, "MULLION_BRIDGE_EVENT\t{name}\t{payload}")
    }
}
