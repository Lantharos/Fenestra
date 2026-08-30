mod bridge;
mod desktop;
mod error;
mod host;
mod launch;
mod osr;
mod render;
mod shell;
mod window;

pub use error::{SabineError, SabineResult};
pub use host::{SabineProcess, SabineProcessHandle, WindowId};
pub use window::{
    AppChrome, SabineColor, SabineLifecyclePolicy, SabineWindow, SabineWindowChrome,
    SabineWindowControlAction,
};

/// Common imports for app authors.
pub mod prelude {
    pub use crate::{
        AppChrome, BridgeCommand, BridgeError, BridgeResponse, BridgeResult, SabineColor,
        SabineError, SabineLifecyclePolicy, SabineProcess, SabineProcessHandle, SabineResult,
        SabineWindow, SabineWindowChrome, ShellSurfaceVisibilityRequest,
        ShellSurfaceVisibilityState, TrayIcon, WindowBackgroundEffect, WindowRegion,
        WindowRegionRect,
    };
}

pub use bridge::{BridgeEventEmitter, ShellSurfaceVisibilityRequest, ShellSurfaceVisibilityState};
pub use shell::ShellSurfaceFrameRate;
pub use window::SabineWindowControlRegion;

pub use sabine_bridge::{ActivityOptions, ActivityRecord, SabineActivityLease};
pub use sabine_bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeResponse, BridgeResult,
    ContentSecurity,
};
pub use sabine_bridge::{
    GuestBounds, GuestCreateOptions, GuestDownloadAction, GuestDownloadEvent, GuestDownloadState,
    GuestHostControl, GuestInfo, GuestPopupPolicy,
};
pub use sabine_bridge::{SabineLaunchMetric, SabineLaunchMetricsSnapshot};
pub use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, Shortcut, ShortcutModifiers, SingleInstancePolicy, TrayIcon, TrayMenuItem,
    WindowBackgroundEffect, WindowRegion, WindowRegionRect, WindowRegions,
};
pub use sabine_runtime::{RuntimeConfig, RuntimeMode};
pub use shell::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};

/// Runs an internal Sabine child mode selected by `args`.
///
/// Custom entry points should call this before argument parsers, logging,
/// configuration, or other application initialization and return immediately
/// when it yields `true`. The dispatcher recognizes both OSR-host and runtime
/// bootstrap children. Ordinary application arguments return `false` without
/// initializing Sabine.
pub fn dispatch_host_mode_from_args(args: &[String]) -> bool {
    launch::dispatch_host_mode_from_args(args)
}

pub(crate) use bridge::{
    parse_host_control, prepare_bridge_command, spawn_bridge_dispatch,
    spawn_bridge_dispatch_for_window,
};
pub(crate) use launch::{apply_browser_launch_args, centered_window_position};
