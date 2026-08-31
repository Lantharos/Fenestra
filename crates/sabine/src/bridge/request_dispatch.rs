use std::thread;

use sabine_bridge::{BridgeCommand, BridgeError, BridgeResult};

const MAX_PENDING_REQUESTS: usize = 128;
const MAX_PENDING_RESPONSES: usize = MAX_PENDING_REQUESTS + REQUEST_WORKERS;
const REQUEST_WORKERS: usize = 4;

pub(super) struct BridgeIpcRequest {
    browser_id: String,
    id: String,
    command: BridgeCommand,
}

impl BridgeIpcRequest {
    pub(super) fn parse(line: &str) -> Option<Self> {
        let parts = line.splitn(6, '\t').collect::<Vec<_>>();
        if parts.first().copied()? != "SABINE_BRIDGE_REQUEST" || parts.len() != 6 {
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

    fn complete(self, result: BridgeResult) -> BridgeIpcResponse {
        BridgeIpcResponse::from_result(self.browser_id, self.id, result)
    }

    fn overloaded(self) -> BridgeIpcResponse {
        self.complete(Err(BridgeError::new(
            "Bridge request capacity is exhausted; retry later",
        )))
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
            "SABINE_BRIDGE_RESPONSE\t{}\t{}\t{status}\t{payload}",
            self.browser_id, self.id
        )
    }
}

pub(super) struct BridgeRequestDispatcher {
    requests: crossbeam_channel::Sender<BridgeIpcRequest>,
    responses: crossbeam_channel::Sender<BridgeIpcResponse>,
}

impl BridgeRequestDispatcher {
    pub(super) fn new(
        runtime: sabine_bridge::BridgeRuntime,
        activity: sabine_bridge::ActivityRegistry,
        activity_emitter: super::events::BridgeEventEmitter,
        writer: super::events::BridgeWriter,
    ) -> Self {
        let (request_sender, request_receiver) =
            crossbeam_channel::bounded::<BridgeIpcRequest>(MAX_PENDING_REQUESTS);
        let (response_sender, response_receiver) =
            crossbeam_channel::bounded::<BridgeIpcResponse>(MAX_PENDING_RESPONSES);

        for _ in 0..REQUEST_WORKERS {
            let requests = request_receiver.clone();
            let responses = response_sender.clone();
            let runtime = runtime.clone();
            let activity = activity.clone();
            let emitter = activity_emitter.clone();
            thread::spawn(move || {
                while let Ok(request) = requests.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if let Some((result, update)) =
                            activity.dispatch_bridge_command(&request.command)
                        {
                            if let (Ok(_), Some(update)) = (result.as_ref(), update.as_ref()) {
                                let _ = emitter.emit_activity_update(update);
                            }
                            result
                        } else {
                            runtime.dispatch(request.command.clone())
                        }
                    }))
                    .unwrap_or_else(|_| Err(BridgeError::new("Bridge handler panicked")));
                    if responses.send(request.complete(result)).is_err() {
                        break;
                    }
                }
            });
        }

        thread::spawn(move || {
            while let Ok(response) = response_receiver.recv() {
                if !writer.send(response.to_string()) {
                    break;
                }
            }
        });

        Self {
            requests: request_sender,
            responses: response_sender,
        }
    }

    pub(super) fn submit(&self, request: BridgeIpcRequest) {
        match self.requests.try_send(request) {
            Ok(()) => {}
            Err(crossbeam_channel::TrySendError::Full(request)) => {
                let _ = self.responses.try_send(request.overloaded());
            }
            Err(crossbeam_channel::TrySendError::Disconnected(request)) => {
                let _ = self
                    .responses
                    .try_send(request.complete(Err(BridgeError::new(
                        "Bridge request executor is unavailable",
                    ))));
            }
        }
    }
}
