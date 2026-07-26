//! Non-desktop stub implementations for desktop services.
//!
//! Linux and macOS ship real tray / shortcut / deep-link backends.
//! Windows uses the WebView2 backend's own desktop-services module.
//! Remaining targets keep these stubs so `fenestra-cef` still compiles.

use std::{
    sync::{Arc, atomic::AtomicBool},
    thread::{self, JoinHandle},
};

use fenestra_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon,
};

#[derive(Debug, Default)]
pub struct LinuxDesktopServiceState;

impl LinuxDesktopServiceState {
    pub fn take_events(&self) -> Vec<PlatformEvent> {
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_linux_desktop_services(
    _tray: Option<&TrayIcon>,
    _autostart: &[AutostartEntry],
    _shortcuts: &[GlobalShortcutRegistration],
    _deep_links: &[DeepLinkRegistration],
    _native_messaging: &[NativeMessagingHost],
    _single_instance_id: Option<&str>,
    _single_instance_policy: Option<SingleInstancePolicy>,
) -> Result<LinuxDesktopServiceState, String> {
    Ok(LinuxDesktopServiceState)
}

pub fn start_desktop_event_forwarder<F>(
    _state: &LinuxDesktopServiceState,
    _running: Arc<AtomicBool>,
    _forwarder: F,
) -> JoinHandle<()>
where
    F: FnMut(PlatformEvent) + Send + 'static,
{
    thread::spawn(|| {})
}
