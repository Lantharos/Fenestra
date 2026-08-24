use std::sync::Arc;

#[cfg(target_os = "linux")]
use winit::platform::wayland::WindowAttributesWayland;
#[cfg(target_os = "windows")]
use winit::platform::windows::WindowAttributesWindows;
#[cfg(target_os = "linux")]
use winit::platform::x11::WindowAttributesX11;
use winit::{
    cursor::CursorIcon,
    dpi::LogicalSize,
    event_loop::ActiveEventLoop,
    window::{
        ImeCapabilities, ImeEnableRequest, ImeRequest, ImeRequestData, Window as WinitWindow,
        WindowAttributes, WindowLevel,
    },
};

use crate::osr::protocol::MAIN_TEXTURE_ID;
use crate::render::GpuRenderer;

use super::OsrNativeHost;

impl OsrNativeHost {
    pub(in crate::osr::host) fn ensure_window(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_none() {
            self.create_window(event_loop);
        }
    }

    pub(in crate::osr::host) fn create_window(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let activating = self.pending_activation_token.is_some();
        let defer_visibility = self.config.visible && !activating;
        let mut attributes = WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_surface_size(LogicalSize::new(
                f64::from(self.config.width),
                f64::from(self.config.height),
            ))
            .with_min_surface_size(LogicalSize::new(
                f64::from(self.config.min_width),
                f64::from(self.config.min_height),
            ))
            .with_resizable(self.config.resizable)
            .with_decorations(self.config.chrome.uses_native_decorations())
            .with_visible(self.config.visible && !defer_visibility)
            .with_active((self.config.active || activating) && !defer_visibility)
            .with_window_level(if self.config.always_on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            })
            .with_transparent(self.config.transparent);
        // On Linux, Sabine applies `ext_background_effect_v1` itself (with
        // blur/opaque/input regions). winit's `with_blur(true)` also creates an
        // effect on the same surface, and a second bind is a protocol error that
        // kills the Wayland connection.
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                let mut wayland_attributes = WindowAttributesWayland::default();
                let mut has_wayland_attributes = false;
                if let Some(app_id) = &self.config.app_id {
                    wayland_attributes = wayland_attributes.with_name(app_id, app_id);
                    has_wayland_attributes = true;
                }
                if let Some(token) = self.pending_activation_token.take() {
                    wayland_attributes = wayland_attributes.with_activation_token(token);
                    has_wayland_attributes = true;
                }
                if has_wayland_attributes {
                    attributes = attributes.with_platform_attributes(Box::new(wayland_attributes));
                }
            } else if let Some(app_id) = &self.config.app_id {
                let x11_attributes = WindowAttributesX11::default().with_name(app_id, app_id);
                attributes = attributes.with_platform_attributes(Box::new(x11_attributes));
            }
        }
        #[cfg(target_os = "windows")]
        if self.config.transparent || self.config.skip_taskbar {
            let windows_attributes = WindowAttributesWindows::default()
                .with_no_redirection_bitmap(self.config.transparent)
                .with_skip_taskbar(self.config.skip_taskbar);
            attributes = attributes.with_platform_attributes(Box::new(windows_attributes));
        }
        if let Some(position) =
            crate::centered_window_position(event_loop, self.config.width, self.config.height)
        {
            attributes = attributes.with_position(position);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::<dyn WinitWindow>::from(window),
            Err(error) => {
                eprintln!("failed to create Sabine OSR host window: {error}");
                event_loop.exit();
                return;
            }
        };
        self.surface_size = window.surface_size();
        self.scale_factor = window.scale_factor();
        let renderer =
            match pollster::block_on(GpuRenderer::new(window.clone(), self.config.transparent)) {
                Ok(renderer) => renderer,
                Err(error) => {
                    eprintln!("failed to initialize Sabine OSR renderer: {error}");
                    event_loop.exit();
                    return;
                }
            };
        self.renderer = Some(renderer);
        self.window = Some(window.clone());
        let ime_caps = ImeCapabilities::new().with_hint_and_purpose();
        if let Some(enable) = ImeEnableRequest::new(ime_caps, ImeRequestData::default()) {
            let _ = window.request_ime_update(ImeRequest::Enable(enable));
        }
        self.send_screen_origin();
        self.launch_child();
        self.upload_cached_textures();
        if self.main_frame.is_some() {
            self.present_after_first_frame();
        }
        if self.config.visible
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    pub(in crate::osr::host) fn drop_hidden_window(&mut self) {
        self.drop_presented_window();
        self.main_frame = None;
        self.overlays.clear();
        self.pending_resize_paint = None;
        self.main_buffer.release();
    }

    pub(in crate::osr::host) fn unmap_window(&mut self) {
        #[cfg(target_os = "linux")]
        self.drop_presented_window();

        #[cfg(not(target_os = "linux"))]
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
    }

    pub(in crate::osr::host) fn drop_presented_window(&mut self) {
        if let Some(window) = &self.window {
            let scale = window.scale_factor().max(1.0);
            self.config.width = (f64::from(self.surface_size.width) / scale)
                .round()
                .max(f64::from(self.config.min_width)) as u32;
            self.config.height = (f64::from(self.surface_size.height) / scale)
                .round()
                .max(f64::from(self.config.min_height)) as u32;
        }
        self.window = None;
        self.renderer = None;
        self.effect = None;
        self.presented = false;
        self.hovered_control = None;
        self.pressed_control = None;
        self.cursor = CursorIcon::Default;
        self.native_cursor_override = false;
    }

    pub(in crate::osr::host) fn upload_cached_textures(&mut self) {
        let main_frame = self
            .main_frame
            .as_ref()
            .map(|frame| (frame.width, frame.height));
        let overlays: Vec<(String, u32, u32, Vec<u8>)> = self
            .overlays
            .iter()
            .map(|(id, overlay)| {
                (
                    crate::osr::host::types::overlay_texture_id(id),
                    overlay.frame.width,
                    overlay.frame.height,
                    overlay.buffer.bytes().to_vec(),
                )
            })
            .collect();
        let main_bytes = self.main_buffer.bytes().to_vec();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Some((width, height)) = main_frame {
            let _ = renderer.update_dynamic_bgra_image_region(
                MAIN_TEXTURE_ID,
                (width, height),
                (0, 0),
                (width, height),
                &main_bytes,
            );
        }
        for (texture_id, width, height, bytes) in overlays {
            let _ = renderer.update_dynamic_bgra_image_region(
                &texture_id,
                (width, height),
                (0, 0),
                (width, height),
                &bytes,
            );
        }
    }

    pub(in crate::osr::host) fn update_effect_regions(&self) {
        let Some(effect) = &self.effect else {
            return;
        };
        let width = self.logical_width().round().max(1.0) as i32;
        let height = self.logical_height().round().max(1.0) as i32;
        let _ = effect.update(&self.window_options(), width, height);
    }
}
