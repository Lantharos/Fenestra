use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    process::Child,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

use mullion_platform::{WindowChrome as PlatformWindowChrome, WindowOptions, WindowRegionRect};
#[cfg(target_os = "linux")]
use winit::platform::wayland::WindowAttributesWayland;
#[cfg(target_os = "windows")]
use winit::platform::windows::{BackdropType, WindowAttributesWindows};
use winit::{
    cursor::CursorIcon,
    data_transfer::DataTransferId,
    dpi::LogicalSize,
    event_loop::{ActiveEventLoop, EventLoopProxy},
    window::{
        ActivationToken, UserAttentionType, Window as WinitWindow, WindowAttributes, WindowLevel,
    },
};

use crate::osr::frame_buffer::FrameBuffer;
use crate::osr::protocol::{MAIN_TEXTURE_ID, OsrFrame};
use crate::osr::transport::IpcStream;
use crate::render::GpuRenderer;
use crate::{MullionWindowChrome, osr};
#[cfg(target_os = "windows")]
use mullion_platform::WindowBackgroundEffect;
use mullion_platform::WindowEffect;

use super::config::OsrHostConfig;
use super::socket::start_socket_reader;
use super::types::{
    ClickMemory, LifecycleState, MouseButtons, OsrHostEvent, OverlayLayer, PendingResizePaint,
    TitlebarControl, uses_mullion_chrome,
};

pub(super) struct OsrNativeHost {
    pub(super) config: OsrHostConfig,
    pub(super) sender: mpsc::Sender<OsrHostEvent>,
    pub(super) receiver: mpsc::Receiver<OsrHostEvent>,
    pub(super) proxy: EventLoopProxy,
    pub(super) window: Option<Arc<dyn WinitWindow>>,
    pub(super) renderer: Option<GpuRenderer>,
    pub(super) effect: Option<WindowEffect>,
    pub(super) child: Option<Child>,
    pub(super) socket: Option<Arc<Mutex<IpcStream>>>,
    pub(super) surface_size: winit::dpi::PhysicalSize<u32>,
    pub(super) main_frame: Option<OsrFrame>,
    pub(super) main_buffer: FrameBuffer,
    pub(super) overlays: BTreeMap<String, OverlayLayer>,
    pub(super) page_drag_regions: Vec<WindowRegionRect>,
    pub(super) page_drag_exclusion_regions: Vec<WindowRegionRect>,
    pub(super) hovered_control: Option<TitlebarControl>,
    pub(super) pressed_control: Option<TitlebarControl>,
    pub(super) cursor: CursorIcon,
    pub(super) native_cursor_override: bool,
    pub(super) modifiers: winit::keyboard::ModifiersState,
    pub(super) mouse: MouseButtons,
    pub(super) last_click: Option<ClickMemory>,
    pub(super) active_click_count: i32,
    pub(super) cursor_x: f32,
    pub(super) cursor_y: f32,
    pub(super) focused: bool,
    pub(super) occluded: bool,
    pub(super) lifecycle_state: LifecycleState,
    pub(super) hibernate_deadline: Option<Instant>,
    pub(super) hibernate_commit_deadline: Option<Instant>,
    pub(super) closing_deadline: Option<Instant>,
    pub(super) pending_resize_paint: Option<PendingResizePaint>,
    pub(super) activity_hibernation_blockers: BTreeSet<String>,
    pub(super) presented: bool,
    pub(super) pending_activation_token: Option<ActivationToken>,
    pub(super) active_file_drag: Option<DataTransferId>,
    pub(super) started: Instant,
    /// CEF exited with process-singleton handoff (code 24). The existing
    /// browser process owns this window's OSR endpoint; keep listening.
    pub(super) cef_handed_off: bool,
}

impl OsrNativeHost {
    pub(super) fn new(
        config: OsrHostConfig,
        sender: mpsc::Sender<OsrHostEvent>,
        receiver: mpsc::Receiver<OsrHostEvent>,
        proxy: EventLoopProxy,
    ) -> Self {
        let surface_size = winit::dpi::PhysicalSize::new(config.width, config.height);
        let visible = config.visible;
        let focused = visible && config.active;
        let lifecycle_state = if visible {
            LifecycleState::Active
        } else {
            LifecycleState::Suspended
        };
        let hibernate_deadline = if visible {
            None
        } else {
            config
                .lifecycle
                .hibernate_after
                .map(|delay| Instant::now() + delay)
        };
        Self {
            config,
            sender,
            receiver,
            proxy,
            window: None,
            renderer: None,
            effect: None,
            child: None,
            socket: None,
            surface_size,
            main_frame: None,
            main_buffer: FrameBuffer::new(),
            overlays: BTreeMap::new(),
            page_drag_regions: Vec::new(),
            page_drag_exclusion_regions: Vec::new(),
            hovered_control: None,
            pressed_control: None,
            cursor: CursorIcon::Default,
            native_cursor_override: false,
            modifiers: Default::default(),
            mouse: MouseButtons::default(),
            last_click: None,
            active_click_count: 1,
            cursor_x: 0.0,
            cursor_y: 0.0,
            focused,
            occluded: false,
            lifecycle_state,
            hibernate_deadline,
            hibernate_commit_deadline: None,
            closing_deadline: None,
            pending_resize_paint: None,
            activity_hibernation_blockers: BTreeSet::new(),
            presented: false,
            pending_activation_token: None,
            active_file_drag: None,
            started: Instant::now(),
            cef_handed_off: false,
        }
    }

    pub(super) fn launch_child(&mut self) {
        if self.child.is_some() {
            return;
        }
        let (endpoint, listener) = match crate::osr::transport::IpcEndpoint::bind() {
            Ok(connection) => connection,
            Err(error) => {
                eprintln!("failed to bind OSR transport: {error}");
                return;
            }
        };
        let authentication_token = match crate::osr::transport::authentication_token() {
            Ok(token) => token,
            Err(error) => {
                eprintln!("failed to secure OSR transport: {error}");
                return;
            }
        };
        start_socket_reader(
            listener,
            authentication_token.clone(),
            self.sender.clone(),
            self.proxy.clone(),
        );

        let (width, height, scale) = self.content_size_for_cef();
        let mut command = osr::cef_osr_command(
            &self.config.runtime_dir,
            &self.config.host_binary,
            &endpoint,
            &authentication_token,
            &self.config,
            width,
            height,
            scale,
            self.active_frame_rate(),
        );
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to launch CEF OSR child: {error}");
                return;
            }
        };
        self.child = Some(child);
        if let Some(child) = self.child.as_mut() {
            let sender = self.sender.clone();
            let proxy = self.proxy.clone();
            crate::spawn_native_host_bridge_proxy(child, move |command, value| {
                let Some(control) = super::events::host_control_from_parts(&command, &value) else {
                    return;
                };
                if sender.send(OsrHostEvent::HostControl(control)).is_ok() {
                    proxy.wake_up();
                }
            });
        }
    }

    pub(super) fn content_size_for_cef(&self) -> (u32, u32, f64) {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
        if !self.config.visible
            && self.window.is_none()
            && self.config.lifecycle.hibernate_after.is_some()
        {
            return (1, 1, scale);
        }
        let logical_width = f64::from(self.surface_size.width) / scale.max(1.0);
        let logical_height = (f64::from(self.surface_size.height) / scale.max(1.0)
            - f64::from(self.titlebar_height()))
        .max(1.0);
        (
            logical_width.round().max(1.0) as u32,
            logical_height.round().max(1.0) as u32,
            scale,
        )
    }

    pub(super) fn titlebar_height(&self) -> f32 {
        if uses_mullion_chrome(self.config.chrome) {
            super::types::TITLEBAR_HEIGHT
        } else {
            0.0
        }
    }

    pub(super) fn window_options(&self) -> WindowOptions {
        WindowOptions {
            title: self.config.title.clone(),
            width: self.config.width,
            height: self.config.height,
            min_width: self.config.min_width,
            min_height: self.config.min_height,
            chrome: platform_chrome(self.config.chrome),
            resizable: self.config.resizable,
            visible: self.config.visible,
            active: self.config.active,
            always_on_top: self.config.always_on_top,
            transparent: self.config.transparent,
            background_effect: self.config.background_effect,
            regions: self.config.regions.clone(),
            ..WindowOptions::default()
        }
    }

    pub(super) fn send_control(&self, line: &str) {
        let Some(socket) = &self.socket else {
            return;
        };
        if let Ok(mut socket) = socket.lock() {
            let _ = socket.write_all(line.as_bytes());
            let _ = socket.flush();
        }
    }

    pub(super) fn content_surface_size(&self) -> (u32, u32) {
        let (width, height, _) = self.content_size_for_cef();
        (width, height)
    }

    pub(super) fn content_position(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let titlebar_height = self.titlebar_height();
        (y >= titlebar_height).then_some((x.max(0.0), (y - titlebar_height).max(0.0)))
    }

    pub(super) fn logical_width(&self) -> f32 {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        self.surface_size.width as f32 / scale.max(1.0)
    }

    pub(super) fn logical_height(&self) -> f32 {
        let scale = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor()) as f32;
        self.surface_size.height as f32 / scale.max(1.0)
    }

    pub(super) fn begin_close(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.closing_deadline.is_some() {
            return;
        }
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
        self.send_control("close\n");
        self.closing_deadline = Some(Instant::now() + super::types::CLOSE_GRACE);
        // Local CEF child owns the process — exit immediately if it is already
        // gone. Handed-off windows still need the grace period so the shared
        // CEF process can close this browser without quitting other windows.
        if self.child.is_none() && !self.cef_handed_off {
            event_loop.exit();
        }
    }

    pub(super) fn force_close(&mut self, event_loop: &dyn ActiveEventLoop) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        event_loop.exit();
    }

    pub(super) fn ensure_window(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_none() {
            self.create_window(event_loop);
        }
    }

    pub(super) fn create_window(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let activating = self.pending_activation_token.is_some();
        let defer_visibility = self.config.visible && can_defer_window_visibility() && !activating;
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
        // On Linux, Mullion applies `ext_background_effect_v1` itself (with
        // blur/opaque/input regions). winit's `with_blur(true)` also creates an
        // effect on the same surface, and a second bind is a protocol error that
        // kills the Wayland connection.
        #[cfg(not(target_os = "linux"))]
        {
            attributes =
                attributes.with_blur(self.config.background_effect.requires_transparency());
        }
        #[cfg(target_os = "linux")]
        {
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
        }
        #[cfg(target_os = "windows")]
        if let Some(backdrop) = windows_system_backdrop(self.config.background_effect) {
            attributes = attributes.with_platform_attributes(Box::new(
                WindowAttributesWindows::default().with_system_backdrop(backdrop),
            ));
        }
        if let Some(position) =
            crate::centered_window_position(event_loop, self.config.width, self.config.height)
        {
            attributes = attributes.with_position(position);
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::<dyn WinitWindow>::from(window),
            Err(error) => {
                eprintln!("failed to create Mullion OSR host window: {error}");
                event_loop.exit();
                return;
            }
        };
        self.surface_size = window.surface_size();
        let renderer = match pollster::block_on(GpuRenderer::new(window.clone())) {
            Ok(renderer) => renderer,
            Err(error) => {
                eprintln!("failed to initialize Mullion OSR renderer: {error}");
                event_loop.exit();
                return;
            }
        };
        self.renderer = Some(renderer);
        self.window = Some(window);
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

    pub(super) fn drop_hidden_window(&mut self) {
        self.drop_presented_window();
        self.main_frame = None;
        self.overlays.clear();
        self.pending_resize_paint = None;
        self.main_buffer.release();
    }

    pub(super) fn drop_presented_window(&mut self) {
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

    pub(super) fn upload_cached_textures(&mut self) {
        let main_frame = self
            .main_frame
            .as_ref()
            .map(|frame| (frame.width, frame.height));
        let overlays: Vec<(String, u32, u32, Vec<u8>)> = self
            .overlays
            .iter()
            .map(|(id, overlay)| {
                (
                    super::types::overlay_texture_id(id),
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
                width,
                height,
                0,
                0,
                width,
                height,
                &main_bytes,
            );
        }
        for (texture_id, width, height, bytes) in overlays {
            let _ = renderer.update_dynamic_bgra_image_region(
                &texture_id,
                width,
                height,
                0,
                0,
                width,
                height,
                &bytes,
            );
        }
    }

    pub(super) fn update_effect_regions(&self) {
        let Some(effect) = &self.effect else {
            return;
        };
        let width = self.logical_width().round().max(1.0) as i32;
        let height = self.logical_height().round().max(1.0) as i32;
        let _ = effect.update(&self.window_options(), width, height);
    }
}

pub(super) fn can_defer_window_visibility() -> bool {
    true
}

pub(super) fn platform_chrome(chrome: MullionWindowChrome) -> PlatformWindowChrome {
    match chrome {
        MullionWindowChrome::System => PlatformWindowChrome::System,
        MullionWindowChrome::Mullion => PlatformWindowChrome::Mullion,
        MullionWindowChrome::Frameless | MullionWindowChrome::None => PlatformWindowChrome::None,
    }
}

#[cfg(target_os = "windows")]
pub(super) fn windows_system_backdrop(effect: WindowBackgroundEffect) -> Option<BackdropType> {
    match effect {
        WindowBackgroundEffect::Acrylic | WindowBackgroundEffect::Glass => {
            Some(BackdropType::TransientWindow)
        }
        WindowBackgroundEffect::Mica => Some(BackdropType::MainWindow),
        WindowBackgroundEffect::MicaAlt => Some(BackdropType::TabbedWindow),
        WindowBackgroundEffect::None => None,
        _ => Some(BackdropType::TransientWindow),
    }
}

pub(super) fn present_window(window: &Arc<dyn WinitWindow>) {
    window.set_visible(true);
    window.set_minimized(false);
    window.request_user_attention(Some(UserAttentionType::Informational));
    window.focus_window();
    window.request_redraw();
}
