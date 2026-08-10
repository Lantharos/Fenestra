pub(crate) mod bootstrap;
pub(crate) mod browser;

pub(crate) use browser::apply_browser_launch_args;

use crate::error::{SabineError, SabineResult};
use crate::osr;
use crate::window::config::SabineWindowConfig;
use sabine_bridge::ContentSecurity;
use std::path::{Path, PathBuf};
use winit::{dpi::PhysicalPosition, event_loop::ActiveEventLoop};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostMode {
    Osr,
    Bootstrap,
}

fn host_mode(args: &[String]) -> Option<HostMode> {
    if args.iter().any(|arg| arg == osr::launch::OSR_HOST_ARG) {
        Some(HostMode::Osr)
    } else if args.iter().any(|arg| arg == bootstrap::BOOTSTRAP_ARG) {
        Some(HostMode::Bootstrap)
    } else {
        None
    }
}

pub(crate) fn dispatch_host_mode_from_args(args: &[String]) -> bool {
    match host_mode(args) {
        Some(HostMode::Osr) => osr::run_from_args(args),
        Some(HostMode::Bootstrap) => bootstrap::run_from_args(args),
        None => false,
    }
}

pub(crate) fn metrics_label(config: &SabineWindowConfig) -> String {
    config
        .app_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.title)
        .to_string()
}

pub(crate) fn centered_window_position(
    event_loop: &dyn ActiveEventLoop,
    width: u32,
    height: u32,
) -> Option<PhysicalPosition<i32>> {
    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())?;
    let mode = monitor.current_video_mode()?;
    let monitor_size = mode.size();
    let monitor_position = monitor.position()?;
    let scale = monitor.scale_factor().max(1.0);
    let physical_width = (f64::from(width) * scale).round() as i32;
    let physical_height = (f64::from(height) * scale).round() as i32;
    let x = monitor_position.x + (monitor_size.width as i32 - physical_width).max(0) / 2;
    let y = monitor_position.y + (monitor_size.height as i32 - physical_height).max(0) / 2;
    Some(PhysicalPosition::new(x, y))
}

pub(crate) fn canonical_entry(entry: &str) -> SabineResult<PathBuf> {
    if entry.trim().is_empty() {
        return Err(SabineError::CreationFailed {
            message: "CEF entry path is empty".to_string(),
        });
    }
    let entry_path = PathBuf::from(entry);
    let path = if entry_path.is_absolute() {
        entry_path
    } else {
        std::env::current_dir()
            .map_err(|error| SabineError::CreationFailed {
                message: error.to_string(),
            })?
            .join(entry_path)
    };
    path.canonicalize()
        .map_err(|error| SabineError::CreationFailed {
            message: format!("failed to resolve CEF entry: {error}"),
        })
}

pub(crate) fn file_url(path: &Path) -> SabineResult<String> {
    url::Url::from_file_path(path)
        .map(String::from)
        .map_err(|()| SabineError::CreationFailed {
            message: format!(
                "failed to convert CEF entry to a file URL: {}",
                path.display()
            ),
        })
}

pub(crate) fn split_entry_suffix(entry: &str) -> (&str, &str) {
    let split = [entry.find('?'), entry.find('#')]
        .into_iter()
        .flatten()
        .min();
    match split {
        Some(index) => (&entry[..index], &entry[index..]),
        None => (entry, ""),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DevUrlParts {
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) suffix: String,
}

#[derive(Clone, Debug)]
pub(crate) struct DevServerCandidate {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) url: String,
}

pub(crate) fn dev_url_parts(url: &str) -> Option<DevUrlParts> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let suffix = rest[authority_end..].to_string();
    let authority = rest[..authority_end].rsplit('@').next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }

    let (host, port) = if let Some(stripped) = authority.strip_prefix('[') {
        let (host, after_host) = stripped.split_once(']')?;
        let port = after_host
            .strip_prefix(':')
            .and_then(|value| value.parse().ok())
            .unwrap_or(default_port);
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => {
                (host, port.parse().ok()?)
            }
            _ => (authority, default_port),
        }
    };
    (!host.is_empty()).then(|| DevUrlParts {
        scheme: scheme.to_string(),
        host: host.to_string(),
        port,
        suffix,
    })
}

pub(crate) fn dev_server_candidates(url: &str) -> Vec<DevServerCandidate> {
    let Some(parts) = dev_url_parts(url) else {
        return Vec::new();
    };
    let mut hosts = vec![parts.host.clone()];
    if is_loopback_host(&parts.host) || is_unspecified_host(&parts.host) {
        hosts.extend(["localhost", "127.0.0.1", "::1"].map(str::to_string));
    }
    let mut candidates = Vec::new();
    for host in hosts {
        if candidates
            .iter()
            .any(|candidate: &DevServerCandidate| candidate.host == host)
        {
            continue;
        }
        candidates.push(DevServerCandidate {
            url: format_dev_url(&parts.scheme, &host, parts.port, &parts.suffix),
            host,
            port: parts.port,
        });
    }
    candidates
}

pub(crate) fn allow_dev_origins(security: &mut ContentSecurity, url: &str) {
    let candidates = dev_server_candidates(url);
    if candidates.is_empty() {
        return;
    }
    security.remote_content = true;
    for candidate in candidates {
        let Some(parts) = dev_url_parts(&candidate.url) else {
            continue;
        };
        allow_origin(
            security,
            format_dev_origin(&parts.scheme, &parts.host, parts.port),
        );
    }
}

pub(crate) fn allow_url_origin(security: &mut ContentSecurity, url: &str) {
    let Some(parts) = dev_url_parts(url) else {
        return;
    };
    allow_origin(
        security,
        format_dev_origin(&parts.scheme, &parts.host, parts.port),
    );
}

pub(crate) fn allow_origin(security: &mut ContentSecurity, origin: String) {
    security.remote_content = true;
    if !security
        .allowed_origins
        .iter()
        .any(|allowed| allowed == &origin)
    {
        security.allowed_origins.push(origin);
    }
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

pub(crate) fn is_unspecified_host(host: &str) -> bool {
    host == "0.0.0.0" || host == "::"
}

pub(crate) fn format_dev_url(scheme: &str, host: &str, port: u16, suffix: &str) -> String {
    format!("{}://{}:{}{}", scheme, format_url_host(host), port, suffix)
}

pub(crate) fn format_dev_origin(scheme: &str, host: &str, port: u16) -> String {
    if is_default_port(scheme, port) {
        format!("{}://{}", scheme, format_url_host(host))
    } else {
        format!("{}://{}:{}", scheme, format_url_host(host), port)
    }
}

pub(crate) fn is_default_port(scheme: &str, port: u16) -> bool {
    matches!((scheme, port), ("http", 80) | ("https", 443))
}

pub(crate) fn format_url_host(host: &str) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod host_mode_tests {
    use super::{HostMode, host_mode};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn identifies_internal_host_modes_without_initializing_them() {
        assert_eq!(host_mode(&args(&["app"])), None);
        assert_eq!(
            host_mode(&args(&["app", "--sabine-osr-host", "config.json"])),
            Some(HostMode::Osr)
        );
        assert_eq!(
            host_mode(&args(&["app", "--sabine-bootstrap", "config.json"])),
            Some(HostMode::Bootstrap)
        );
    }

    #[test]
    fn osr_mode_takes_precedence_if_arguments_are_malformed() {
        assert_eq!(
            host_mode(&args(&[
                "app",
                "--sabine-bootstrap",
                "bootstrap.json",
                "--sabine-osr-host",
                "osr.json",
            ])),
            Some(HostMode::Osr)
        );
    }
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use std::time::Duration;

    use sabine_platform::WindowBackgroundEffect;

    use crate::window::{SabineLifecyclePolicy, SabineWindow};

    #[test]
    fn recipes_set_expected_defaults() {
        let app = SabineWindow::new().app();
        assert!(!app.config.transparent);
        assert_eq!(app.config.chrome, crate::window::SabineWindowChrome::System);
        assert_eq!(app.config.lifecycle, SabineLifecyclePolicy::browser_tab());

        let palette = SabineWindow::new().palette();
        assert!(palette.config.transparent);
        assert!(palette.config.hide_on_blur);
        assert!(palette.config.skip_taskbar);
        assert_eq!(
            palette.config.lifecycle,
            SabineLifecyclePolicy::hidden_window()
        );

        let tray = SabineWindow::new().tray_app();
        assert!(!tray.config.visible);
        assert!(tray.config.skip_taskbar);
        assert_eq!(
            tray.config.lifecycle,
            SabineLifecyclePolicy::hidden_window()
        );
    }

    #[test]
    fn app_chrome_adds_drag_and_controls() {
        let window = SabineWindow::new().app_chrome(crate::AppChrome::new(38, 260));
        assert_eq!(window.config.drag_regions.len(), 1);
        assert_eq!(window.config.control_regions.len(), 3);
        assert!(window.config.regions.blur.is_some());
        assert!(window.config.regions.opaque.is_some());
    }

    #[test]
    fn hidden_window_lifecycle_is_palette_biased() {
        let lifecycle = SabineLifecyclePolicy::hidden_window();
        assert_eq!(lifecycle.background_frame_rate, 1);
        assert!(lifecycle.suspend_on_blur);
        assert_eq!(lifecycle.hibernate_after, None);
        assert_eq!(lifecycle.hibernate_grace, Duration::from_millis(150));
        assert!(lifecycle.retain_hidden_frame);
        assert!(!lifecycle.memory_saver);
    }

    #[test]
    fn memory_saver_hidden_window_hibernates_quickly() {
        let lifecycle = SabineLifecyclePolicy::memory_saver_hidden_window();
        assert_eq!(lifecycle.background_frame_rate, 1);
        assert!(lifecycle.suspend_on_blur);
        assert_eq!(lifecycle.hibernate_after, Some(Duration::from_secs(5)));
        assert_eq!(lifecycle.hibernate_grace, Duration::from_millis(150));
        assert!(!lifecycle.retain_hidden_frame);
        assert!(lifecycle.memory_saver);
    }

    #[test]
    fn hidden_builder_uses_hidden_lifecycle_defaults() {
        let window = SabineWindow::new().hidden();
        assert!(!window.config.visible);
        assert_eq!(window.config.lifecycle.background_frame_rate, 1);
        assert!(window.config.lifecycle.suspend_on_blur);
        assert_eq!(window.config.lifecycle.hibernate_after, None);
        assert!(window.config.lifecycle.retain_hidden_frame);
    }

    #[test]
    fn glass_defaults_to_platform_native_material() {
        let window = SabineWindow::new().glass();
        assert!(window.config.transparent);
        let expected = match sabine_platform::current_desktop_os() {
            sabine_platform::PlatformOs::Windows => WindowBackgroundEffect::Acrylic,
            sabine_platform::PlatformOs::Macos => WindowBackgroundEffect::Vibrancy,
            sabine_platform::PlatformOs::Linux => WindowBackgroundEffect::Blur,
            _ => WindowBackgroundEffect::None,
        };
        assert_eq!(window.config.background_effect, expected);
    }

    #[test]
    fn url_sets_production_url_and_bridge_origin() {
        let window = SabineWindow::new().url("https://raday.lantharos.com/dashboard");

        assert_eq!(
            window.entry_url().unwrap(),
            "https://raday.lantharos.com/dashboard"
        );
        assert!(window.config.security.remote_content);
        assert!(
            window
                .config
                .security
                .allowed_origins
                .iter()
                .any(|origin| origin == "https://raday.lantharos.com")
        );
    }

    #[test]
    fn dev_url_takes_precedence_over_production_url() {
        let window = SabineWindow::new()
            .url("https://raday.lantharos.com")
            .dev_url("http://localhost:5173");

        assert_eq!(window.entry_url().unwrap(), "http://localhost:5173");
        assert!(
            window
                .config
                .security
                .allowed_origins
                .iter()
                .any(|origin| origin == "https://raday.lantharos.com")
        );
        assert!(
            window
                .config
                .security
                .allowed_origins
                .iter()
                .any(|origin| origin == "http://localhost:5173")
        );
    }

    #[test]
    fn window_config_rejects_ambiguous_content_sources() {
        let window = SabineWindow::new()
            .app_id("com.sabine.notes")
            .entry("ui/index.html")
            .url("https://example.com");

        assert!(window.config.validate().is_err());
    }
}
