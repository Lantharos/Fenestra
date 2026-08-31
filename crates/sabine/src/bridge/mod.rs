mod events;
mod request_dispatch;

pub use events::{BridgeEventEmitter, ShellSurfaceVisibilityRequest, ShellSurfaceVisibilityState};
pub(crate) use events::{
    parse_host_control, platform_event_payload, prepare_bridge_command, spawn_bridge_dispatch,
    spawn_bridge_dispatch_for_window,
};
