mod chrome;
mod config;
mod events;
mod guest_preview;
mod input;
mod lifecycle;
mod native;
mod paint;
mod paint_accel;
mod paint_upload;
mod socket;
mod types;

use std::path::PathBuf;
use std::sync::mpsc;

use winit::event_loop::EventLoop;

pub(crate) use config::OsrHostConfig;
#[allow(unused_imports)]
pub(crate) use guest_preview::guest_preview_data_url;

use native::OsrNativeHost;

pub(crate) fn run(config_path: PathBuf) -> Result<(), String> {
    let config = OsrHostConfig::read(config_path)?;
    #[cfg(target_os = "linux")]
    if config.shell_surface.is_some() {
        return crate::osr::layer::run(config);
    }
    #[cfg(not(target_os = "linux"))]
    if config.shell_surface.is_some() {
        return Err(
            "shell surfaces are Linux-only; use a hide-on-blur palette window here".to_string(),
        );
    }
    let event_loop = EventLoop::new().map_err(|error| error.to_string())?;
    let proxy = event_loop.create_proxy();
    let (sender, receiver) = mpsc::sync_channel(8);
    event_loop
        .run_app(OsrNativeHost::new(config, sender, receiver, proxy))
        .map_err(|error| error.to_string())
}

fn trace_host(config: &OsrHostConfig, stage: impl AsRef<str>) {
    let enabled = std::env::var(sabine_bridge::SABINE_TRACE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "trace"
        )
    });
    if !enabled {
        return;
    }
    let label = config.app_id.as_deref().unwrap_or(&config.title);
    eprintln!(
        "sabine trace [{label}] osr-host pid={} {}",
        std::process::id(),
        stage.as_ref()
    );
}
