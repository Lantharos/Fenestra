mod bridge;
mod desktop;
mod error;
mod host;
mod launch;
mod osr;
mod render;
mod window;

pub use error::{SabineError, SabineResult};
pub use host::{SabineProcess, WindowId};
pub use window::{
    AppChrome, SabineLifecyclePolicy, SabineWindow, SabineWindowChrome, SabineWindowControlAction,
};

/// Common imports for app authors.
pub mod prelude {
    pub use crate::{
        AppChrome, BridgeCommand, BridgeError, BridgeResponse, BridgeResult, SabineError,
        SabineLifecyclePolicy, SabineProcess, SabineResult, SabineWindow, SabineWindowChrome,
        TrayIcon, WindowBackgroundEffect, WindowRegion, WindowRegionRect,
    };
}

pub use bridge::BridgeEventEmitter;
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
    PlatformEvent, SingleInstancePolicy, TrayIcon, TrayMenuItem, WindowBackgroundEffect,
    WindowRegion, WindowRegionRect, WindowRegions,
};
pub use sabine_platform::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};
pub use sabine_runtime::{RuntimeConfig, RuntimeMode};

pub(crate) use bridge::{
    parse_host_control, prepare_bridge_command, spawn_bridge_dispatch,
    spawn_bridge_dispatch_for_window,
};
pub(crate) use launch::{apply_browser_launch_args, centered_window_position};
