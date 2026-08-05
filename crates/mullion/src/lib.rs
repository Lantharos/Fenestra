mod bridge;
mod desktop;
mod error;
mod host;
mod launch;
mod osr;
mod render;
mod window;

pub use error::{MullionError, MullionResult};
pub use host::{MullionProcess, WindowId};
pub use launch::run_mullion_host_from_args;
pub use window::{
    AppChrome, GlassSpec, MullionLifecyclePolicy, MullionWindow, MullionWindowChrome,
    MullionWindowConfig, MullionWindowControlAction, parse_localhost_port, vite_dev_command,
    vite_dev_url,
};

/// Common imports for app authors.
pub mod prelude {
    pub use crate::{
        AppChrome, BridgeCommand, BridgeError, BridgeResponse, BridgeResult, GlassSpec,
        MullionError, MullionLifecyclePolicy, MullionProcess, MullionResult, MullionWindow,
        MullionWindowChrome, TrayIcon, WindowBackgroundEffect, WindowRegion, WindowRegionRect,
    };
}

pub use bridge::BridgeEventEmitter;
pub use desktop::{DesktopServiceState, apply_desktop_services, start_desktop_event_forwarder};
pub use host::{browser_profile_dir, ensure_host, host_release_binary, ld_library_path};
pub use launch::BrowserOptions;
pub use window::{DesktopServiceConfig, MullionWindowControlRegion};

pub use mullion_bridge::{
    ActivityEventEmitter, ActivityHostUpdate, ActivityOptions, ActivityRecord, MullionActivityLease,
};
pub use mullion_bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeHandlers, BridgeRegistry,
    BridgeResponse, BridgeResult, ContentSecurity, bridge_commands_with_all_internal,
    current_bridge_targets, host_update_json,
};
pub use mullion_bridge::{
    GUEST_CREATE_COMMAND, GuestBounds, GuestCreateOptions, GuestDownloadAction, GuestDownloadEvent,
    GuestDownloadState, GuestHostControl, GuestInfo, GuestPopupPolicy, POPUP_GUEST_ID,
    bridge_commands_with_guest, default_partition_for, is_guest_command, normalize_guest_id,
};
pub use mullion_bridge::{MULLION_TRACE_ENV, MullionLaunchMetric, MullionLaunchMetricsSnapshot};
pub use mullion_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon, TrayMenuItem, WindowBackgroundEffect,
    WindowRegion, WindowRegionRect, WindowRegions,
};
pub use mullion_platform::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};
pub use mullion_runtime::{
    RuntimeConfig, RuntimeError, RuntimeInfo, RuntimeInstallProgress, RuntimeInstallStep,
    RuntimeLocation, RuntimeMode, RuntimePackage, detect_runtime,
    install_user_runtime_with_progress, resolve_runtime, user_runtime_path,
};
pub use mullion_service::{
    AppManifest, AppUpdateConfig, MullionService, ServicePolicy, UpdatePolicy, ensure_ready,
    set_login_autostart,
};

pub(crate) use bridge::{
    parse_host_control, prepare_bridge_command, spawn_bridge_dispatch,
    spawn_bridge_dispatch_for_window,
};
pub(crate) use launch::{apply_browser_launch_args, centered_window_position};
