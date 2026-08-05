mod events;

pub use events::BridgeEventEmitter;
#[cfg(target_os = "linux")]
pub(crate) use events::parse_host_control;
pub(crate) use events::{
    platform_event_payload, prepare_bridge_command, spawn_bridge_dispatch,
    spawn_native_host_bridge_proxy,
};
