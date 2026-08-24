#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[path = "stub.rs"]
mod platform;

#[cfg(target_os = "linux")]
pub(crate) use platform::integrate_appimage;
pub use platform::{DesktopServiceState, apply_desktop_services, start_desktop_event_forwarder};

pub(crate) const INSTANCE_ALREADY_RUNNING: &str = "another instance is already running";
