//! Non-desktop stub implementations for desktop services.
//!
//! Linux, macOS, and Windows ship native desktop-service backends.
//! Remaining targets keep these stubs so `sabine` still compiles.

use std::thread::{self, JoinHandle};

use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    PlatformEvent, SingleInstancePolicy, TrayIcon,
};

#[derive(Debug, Default)]
pub struct DesktopServiceState;

impl DesktopServiceState {
    pub fn take_events(&self) -> Vec<PlatformEvent> {
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_desktop_services(
    _tray: Option<&TrayIcon>,
    _autostart: &[AutostartEntry],
    _shortcuts: &[GlobalShortcutRegistration],
    _deep_links: &[DeepLinkRegistration],
    _native_messaging: &[NativeMessagingHost],
    _single_instance_id: Option<&str>,
    _single_instance_policy: Option<SingleInstancePolicy>,
) -> Result<DesktopServiceState, String> {
    Ok(DesktopServiceState)
}

pub fn start_desktop_event_forwarder<F>(
    _state: &DesktopServiceState,
    _stop: crossbeam_channel::Receiver<()>,
    _forwarder: F,
) -> JoinHandle<()>
where
    F: FnMut(PlatformEvent) + Send + 'static,
{
    thread::spawn(|| {})
}
