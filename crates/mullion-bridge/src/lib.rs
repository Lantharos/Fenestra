//! Mullion bridge, activity, guest, and web page IPC primitives.
//!
//! `mullion-bridge` is the load-bearing crate for the bridge protocol that
//! lets a Mullion page call into the native host. The wire format, JavaScript
//! surface, activity registry, and security model have one source of truth.
//!
//! Crates that depend on `mullion-bridge`:
//!
//! - `mullion` — drives the C++ CEF OSR host and re-exports these types.
//! - Apps depend on `mullion` (which re-exports the bridge surface).

pub mod activity;
pub mod bridge;
pub mod guest;
pub mod guest_input;
pub mod metrics;
pub mod web_bridge;

pub use activity::{
    ActivityEventEmitter, ActivityHostUpdate, ActivityOptions, ActivityRecord, ActivityRegistry,
    MullionActivityLease, POPUP_CLOSE_COMMAND, POPUP_OPEN_COMMAND,
    bridge_commands_with_all_internal, bridge_commands_with_internal, host_update_json,
};
pub use bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeHandlers, BridgeRegistry,
    BridgeResponse, BridgeResult, BridgeRuntime, ContentSecurity, current_bridge_targets,
};
pub use guest::{
    CREATE_COMMAND as GUEST_CREATE_COMMAND, DESTROY_COMMAND as GUEST_DESTROY_COMMAND,
    DOWNLOAD_ACTION_COMMAND as GUEST_DOWNLOAD_ACTION_COMMAND,
    EXECUTE_JS_COMMAND as GUEST_EXECUTE_JS_COMMAND, FOCUS_COMMAND as GUEST_FOCUS_COMMAND,
    GET_COMMAND as GUEST_GET_COMMAND, GO_BACK_COMMAND as GUEST_GO_BACK_COMMAND,
    GO_FORWARD_COMMAND as GUEST_GO_FORWARD_COMMAND, GuestBounds, GuestCreateOptions,
    GuestDownloadAction, GuestDownloadEvent, GuestDownloadState, GuestHostControl, GuestInfo,
    GuestPopupPolicy, LIST_COMMAND as GUEST_LIST_COMMAND,
    NAVIGATE_COMMAND as GUEST_NAVIGATE_COMMAND, POPUP_GUEST_ID,
    RELOAD_COMMAND as GUEST_RELOAD_COMMAND, SET_BOUNDS_COMMAND as GUEST_SET_BOUNDS_COMMAND,
    SET_VISIBLE_COMMAND as GUEST_SET_VISIBLE_COMMAND, SET_ZOOM_COMMAND as GUEST_SET_ZOOM_COMMAND,
    bridge_commands_with_guest, default_partition_for, is_guest_command, normalize_guest_id,
};
pub use guest_input::{
    MOD_ALT, MOD_COMMAND, MOD_CONTROL, MOD_MASK, MOD_SHIFT, is_predominantly_horizontal_wheel,
    match_intercepted_shortcut, platform_primary_modifier,
};
pub use metrics::{
    LaunchMetrics, MULLION_TRACE_ENV, MullionLaunchMetric, MullionLaunchMetricsSnapshot,
};
pub use web_bridge::{
    BRIDGE_SCHEME, BridgeRequest, INSTALL_SCRIPT, WINDOW_SCHEME, WindowCommand, bridge_url,
    install_script, parse_bridge_url,
};
