#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[path = "stub.rs"]
mod platform;

pub use platform::{DesktopServiceState, apply_desktop_services, start_desktop_event_forwarder};
