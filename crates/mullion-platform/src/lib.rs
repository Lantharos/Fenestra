#[cfg(any(target_os = "windows", target_os = "macos"))]
#[path = "platform/desktop_background_effect.rs"]
mod desktop_background_effect;
mod desktop_integration;
mod regions;
mod shell;
#[path = "platform/wayland_background_effect.rs"]
mod wayland_background_effect;
mod window_options;

use std::sync::Arc;

use winit::window::Window;

pub use desktop_integration::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutActivation, GlobalShortcutRegistration,
    NativeMessagingHost, PlatformEvent, Shortcut, ShortcutModifiers, SingleInstanceActivation,
    SingleInstancePolicy, TrayActivation, TrayIcon, TrayMenuItem,
};
pub use regions::{WindowRegion, WindowRegionAdaptive, WindowRegionRect, WindowRegions};
pub use shell::{
    ShellSurfaceAnchor, ShellSurfaceKeyboardInteractivity, ShellSurfaceLayer, ShellSurfaceMargin,
    ShellSurfaceOptions,
};
pub use wayland_background_effect::WaylandEffect as WindowEffect;
pub use window_options::{
    PlatformOs, WindowBackgroundEffect, WindowChrome, WindowOptions, current_desktop_os,
};

pub fn request_window_effect(
    window: &Arc<dyn Window>,
    options: &WindowOptions,
) -> Option<WindowEffect> {
    #[cfg(target_os = "linux")]
    {
        wayland_background_effect::request(window, options)
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        desktop_background_effect::request(window, options)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = window;
        let _ = options;
        None
    }
}

pub fn request_surface_effect<W>(
    window: &W,
    options: &WindowOptions,
    width: i32,
    height: i32,
) -> Option<WindowEffect>
where
    W: raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle + ?Sized,
{
    wayland_background_effect::request_surface(window, options, width, height)
}
