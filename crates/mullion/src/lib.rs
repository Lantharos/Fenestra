mod bootstrap;
mod bridge_events;
mod browser;
mod config;
mod error;
mod glass;
mod host;
mod launch_support;
mod process;
mod process_tree;
mod render;
mod style;
mod window;

#[cfg(target_os = "linux")]
mod desktop_services;
#[cfg(target_os = "macos")]
mod desktop_services_macos;
#[cfg(target_os = "windows")]
mod desktop_services_windows;
mod osr;
mod osr_frame_buffer;
mod osr_host;
#[cfg(target_os = "linux")]
mod osr_layer_host;
mod osr_protocol;
mod osr_transport;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod desktop_services_stub;
#[cfg(target_os = "macos")]
use desktop_services_macos as desktop_services;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use desktop_services_stub as desktop_services;
#[cfg(target_os = "windows")]
use desktop_services_windows as desktop_services;

pub use desktop_services::{
    DesktopServiceState, apply_desktop_services, start_desktop_event_forwarder,
};
pub use host::{browser_profile_dir, ensure_host, host_release_binary, ld_library_path};
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

pub use bridge_events::BridgeEventEmitter;
pub use browser::BrowserOptions;
pub use config::{
    DesktopServiceConfig, MullionLifecyclePolicy, MullionWindowChrome, MullionWindowConfig,
    MullionWindowControlAction, MullionWindowControlRegion,
};
pub use error::{MullionError, MullionResult};
pub use glass::GlassSpec;
pub use launch_support::run_mullion_host_from_args;
pub use process::MullionProcess;
pub use window::MullionWindow;

#[cfg(target_os = "linux")]
pub(crate) use bridge_events::parse_host_control;
pub(crate) use bridge_events::{
    prepare_bridge_command, spawn_bridge_dispatch, spawn_native_host_bridge_proxy,
};
#[cfg(target_os = "linux")]
pub(crate) use browser::HOST_CONTROL_PREFIX;
pub(crate) use browser::apply_browser_launch_args;
pub(crate) use launch_support::centered_window_position;
