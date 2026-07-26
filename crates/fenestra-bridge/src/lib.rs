//! Engine-neutral Fenestra bridge, activity, and web page IPC primitives.
//!
//! `fenestra-bridge` is the load-bearing crate for the bridge protocol that
//! lets a Fenestra webview call into the host process. It is shared by the
//! CEF and WebView2 backends so the wire format, the JS surface, the
//! activity registry, and the security model have one source of truth.
//!
//! Crates that depend on `fenestra-bridge`:
//!
//! - `fenestra-cef` — drives the C++ CEF host over stdio; re-exports the
//!   bridge types from this crate to keep the historical public surface
//!   stable.
//! - `fenestra-webview2` — drives WebView2 in-process; defines the
//!   [`ActivityEventEmitter`] implementation that talks to
//!   `ICoreWebView2::ExecuteScript`.
//! - Apps depend on `fenestra-cef` (which re-exports the bridge surface).

pub mod activity;
pub mod bridge;
pub mod guest;
pub mod metrics;
pub mod web_bridge;

pub use activity::{
    ActivityEventEmitter, ActivityHostUpdate, ActivityOptions, ActivityRecord, ActivityRegistry,
    FenestraActivityLease, POPUP_CLOSE_COMMAND, POPUP_OPEN_COMMAND,
    bridge_commands_with_all_internal, bridge_commands_with_internal, host_update_json,
};
pub use guest::{
    CREATE_COMMAND as GUEST_CREATE_COMMAND, DESTROY_COMMAND as GUEST_DESTROY_COMMAND,
    DOWNLOAD_ACTION_COMMAND as GUEST_DOWNLOAD_ACTION_COMMAND, EXECUTE_JS_COMMAND as GUEST_EXECUTE_JS_COMMAND,
    FOCUS_COMMAND as GUEST_FOCUS_COMMAND, GET_COMMAND as GUEST_GET_COMMAND,
    GO_BACK_COMMAND as GUEST_GO_BACK_COMMAND, GO_FORWARD_COMMAND as GUEST_GO_FORWARD_COMMAND,
    LIST_COMMAND as GUEST_LIST_COMMAND, NAVIGATE_COMMAND as GUEST_NAVIGATE_COMMAND, POPUP_GUEST_ID,
    RELOAD_COMMAND as GUEST_RELOAD_COMMAND, SET_BOUNDS_COMMAND as GUEST_SET_BOUNDS_COMMAND,
    SET_VISIBLE_COMMAND as GUEST_SET_VISIBLE_COMMAND, SET_ZOOM_COMMAND as GUEST_SET_ZOOM_COMMAND,
    GuestBounds, GuestCreateOptions, GuestDownloadAction, GuestDownloadEvent, GuestDownloadState,
    GuestHostControl, GuestInfo, GuestPopupPolicy, bridge_commands_with_guest, default_partition_for,
    is_guest_command, normalize_guest_id,
};
pub use bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeHandlers, BridgeRegistry,
    BridgeResponse, BridgeResult, BridgeRuntime, WebViewSecurity, current_bridge_targets,
};
pub use metrics::{
    FENESTRA_TRACE_ENV, FenestraLaunchMetric, FenestraLaunchMetricsSnapshot, LaunchMetrics,
};
pub use web_bridge::{
    BRIDGE_SCHEME, BridgeRequest, INSTALL_SCRIPT, WINDOW_SCHEME, WindowCommand, bridge_url,
    install_script, parse_bridge_url,
};
