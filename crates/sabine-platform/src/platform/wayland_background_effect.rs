#[cfg(target_os = "linux")]
use crate::WindowBackgroundEffect;
use crate::WindowOptions;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use winit::window::Window;

#[cfg(target_os = "linux")]
use std::ptr;

#[cfg(target_os = "linux")]
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};

#[cfg(target_os = "linux")]
#[path = "wayland_background_effect_protocol.rs"]
mod wayland_background_effect_protocol;
#[cfg(target_os = "linux")]
use wayland_background_effect_protocol::*;

#[cfg(target_os = "linux")]
#[path = "wayland_background_effect_ext.rs"]
mod wayland_background_effect_ext;
#[cfg(target_os = "linux")]
use wayland_background_effect_ext::{ExtBackgroundEffect, ManagerState, apply_surface_regions};

#[cfg(target_os = "linux")]
pub fn request(window: &Arc<dyn Window>, options: &WindowOptions) -> Option<WaylandEffect> {
    request_surface(
        window.as_ref(),
        options,
        options.width as i32,
        options.height as i32,
    )
}

#[cfg(target_os = "linux")]
pub fn request_surface<W>(
    window: &W,
    options: &WindowOptions,
    width: i32,
    height: i32,
) -> Option<WaylandEffect>
where
    W: HasDisplayHandle + HasWindowHandle + ?Sized,
{
    let wants_blur =
        options.transparent && options.background_effect != WindowBackgroundEffect::None;
    if !wants_blur && options.regions.is_empty() {
        debug("skipped: background effect and surface regions were not requested");
        return None;
    }
    let Some(display) = wayland_display(window) else {
        debug("skipped: native window is not backed by a Wayland display");
        return None;
    };
    let Some(surface) = wayland_surface(window) else {
        debug("skipped: native window is not backed by a Wayland surface");
        return None;
    };
    debug("requesting ext_background_effect_v1");
    unsafe {
        ExtBackgroundEffect::bind(
            display,
            surface,
            options.background_effect,
            width,
            height,
            wants_blur,
            options,
        )
    }
}

#[cfg(not(target_os = "linux"))]
pub fn request_surface<W>(
    _window: &W,
    _options: &WindowOptions,
    _width: i32,
    _height: i32,
) -> Option<WaylandEffect>
where
    W: ?Sized,
{
    None
}

#[derive(Debug)]
#[cfg(target_os = "linux")]
pub struct WaylandEffect {
    pub(super) display: *mut WlDisplay,
    pub(super) surface: *mut WlProxy,
    pub(super) effect: *mut WlProxy,
    pub(super) manager: *mut WlProxy,
    pub(super) compositor: *mut WlProxy,
    pub(super) _manager_state: Box<ManagerState>,
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub struct WaylandEffect;

#[cfg(not(target_os = "linux"))]
impl WaylandEffect {
    pub fn update(&self, _options: &WindowOptions, _width: i32, _height: i32) -> bool {
        false
    }
}

#[cfg(target_os = "linux")]
impl Drop for WaylandEffect {
    fn drop(&mut self) {
        unsafe {
            if !self.effect.is_null() {
                wl_proxy_marshal_flags(self.effect, EFFECT_DESTROY, ptr::null(), 1, DESTROY_FLAG);
                self.effect = ptr::null_mut();
            }
            if !self.manager.is_null() {
                wl_proxy_marshal_flags(self.manager, MANAGER_DESTROY, ptr::null(), 1, DESTROY_FLAG);
                self.manager = ptr::null_mut();
            }
            if !self.compositor.is_null() {
                wl_proxy_destroy(self.compositor.cast());
                self.compositor = ptr::null_mut();
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl WaylandEffect {
    pub fn update(&self, options: &WindowOptions, width: i32, height: i32) -> bool {
        if self.display.is_null() || self.surface.is_null() || self.compositor.is_null() {
            return false;
        }
        unsafe {
            apply_surface_regions(
                self.display,
                self.surface,
                self.compositor,
                self.effect,
                options,
                width,
                height,
            )
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn wayland_display<W>(window: &W) -> Option<*mut WlDisplay>
where
    W: HasDisplayHandle + ?Sized,
{
    match window.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Wayland(display) => Some(display.display.as_ptr().cast()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
pub(super) fn wayland_surface<W>(window: &W) -> Option<*mut WlProxy>
where
    W: HasWindowHandle + ?Sized,
{
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Wayland(surface) => Some(surface.surface.as_ptr().cast()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn debug(message: &str) {
    if std::env::var_os("SABINE_WAYLAND_EFFECT_DEBUG").is_some() {
        eprintln!("[sabine-wayland-effect] {message}");
    }
}
