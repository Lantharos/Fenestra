mod window;

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, Write},
    process::Child,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

use sabine_platform::{WindowChrome as PlatformWindowChrome, WindowOptions, WindowRegionRect};
#[cfg(target_os = "windows")]
use winit::platform::windows::BackdropType;
use winit::{
    cursor::CursorIcon,
    data_transfer::DataTransferId,
    event_loop::{ActiveEventLoop, EventLoopProxy},
    window::{ActivationToken, UserAttentionType, Window as WinitWindow},
};

use crate::osr::frame_buffer::FrameBuffer;
use crate::osr::protocol::OsrFrame;
use crate::osr::transport::IpcStream;
use crate::render::GpuRenderer;
use crate::{SabineWindowChrome, osr};
#[cfg(target_os = "windows")]
use sabine_platform::WindowBackgroundEffect;
use sabine_platform::WindowEffect;

use super::config::OsrHostConfig;
use super::socket::start_socket_reader;
use super::types::{
    ClickMemory, LifecycleState, MouseButtons, OsrHostEvent, OverlayLayer, PendingResizePaint,
    TitlebarControl, uses_sabine_chrome,
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
    pub(super) scale_factor: f64,
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
    pub(super) pending_suspend_at: Option<Instant>,
    pub(super) effect_regions_dirty: bool,
    pub(super) activity_hibernation_blockers: BTreeSet<String>,
    pub(super) presented: bool,
    pub(super) pending_activation_token: Option<ActivationToken>,
    pub(super) active_file_drag: Option<DataTransferId>,
    pub(super) started: Instant,
    /// CEF exited with process-singleton handoff (code 24). The existing
    /// browser process owns this window's OSR endpoint; keep listening.
    pub(super) cef_handed_off: bool,
    /// Deadline for the primary CEF process to connect after exit-24 handoff.
    pub(super) handoff_deadline: Option<Instant>,
    pub(super) accel_fallback: crate::osr::accel::AccelFallbackPolicy,
}

impl OsrNativeHost {
    pub(super) fn new(
        config: OsrHostConfig,
        sender: mpsc::Sender<OsrHostEvent>,
        receiver: mpsc::Receiver<OsrHostEvent>,
        proxy: EventLoopProxy,
    ) -> Self {
        start_parent_bridge_reader(sender.clone(), proxy.clone());
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
            scale_factor: 1.0,
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
            pending_suspend_at: None,
            effect_regions_dirty: false,
            activity_hibernation_blockers: BTreeSet::new(),
            presented: false,
            pending_activation_token: None,
            active_file_drag: None,
            started: Instant::now(),
            cef_handed_off: false,
            handoff_deadline: None,
            accel_fallback: crate::osr::accel::AccelFallbackPolicy::default(),
        }
    }

    pub(super) fn launch_child(&mut self) {
        if self.child.is_some() {
            return;
        }
        let Some(app_id) = self
            .config
            .app_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            eprintln!("Sabine OSR host requires a non-empty app_id");
            return;
        };
        let (endpoint, listener) = match crate::osr::transport::IpcEndpoint::bind(app_id) {
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
            endpoint.clone(),
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
    }

    pub(super) fn relaunch_software_osr(&mut self) {
        if self.accel_fallback.is_software() || self.config.software_osr_fallback {
            return;
        }
        self.accel_fallback.mark_software();
        self.config.software_osr_fallback = true;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.socket = None;
        self.main_frame = None;
        self.main_buffer.release();
        self.overlays.clear();
        self.launch_child();
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
        if uses_sabine_chrome(self.config.chrome) {
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
        // gone. Handed-off / shared-singleton windows still need the grace
        // period so CloseBrowser can finish without killing sibling windows.
        if self.child.is_none() && !self.cef_handed_off {
            event_loop.exit();
        }
    }

    pub(super) fn force_close(&mut self, event_loop: &dyn ActiveEventLoop) {
        // Do not kill the CEF child here. Multi-window apps share one CEF
        // process via profile singleton handoff; killing it would close every
        // window. CloseBrowser / socket-EOF teardown owns CEF lifetime.
        // Hibernate is the only path that intentionally kills CEF.
        if let Some(mut child) = self.child.take() {
            let _ = child.try_wait();
        }
        self.socket = None;
        event_loop.exit();
    }
}

pub(super) fn can_defer_window_visibility() -> bool {
    // Windows accelerated OSR can stay silent while CEF/GPU spin up; showing
    // the native shell immediately avoids an invisible window forever.
    #[cfg(target_os = "windows")]
    {
        return false;
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

pub(super) fn platform_chrome(chrome: SabineWindowChrome) -> PlatformWindowChrome {
    match chrome {
        SabineWindowChrome::System => PlatformWindowChrome::System,
        SabineWindowChrome::Sabine => PlatformWindowChrome::Sabine,
        SabineWindowChrome::Frameless | SabineWindowChrome::None => PlatformWindowChrome::None,
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

fn start_parent_bridge_reader(sender: mpsc::Sender<OsrHostEvent>, proxy: EventLoopProxy) {
    std::thread::spawn(move || {
        let input = std::io::stdin();
        for line in input.lock().lines().map_while(std::result::Result::ok) {
            if let Some((command, value)) = crate::parse_host_control(&line) {
                if let Some(control) = super::events::host_control_from_parts(command, value) {
                    if sender.send(OsrHostEvent::HostControl(control)).is_err() {
                        break;
                    }
                    proxy.wake_up();
                    continue;
                }
            }
            if sender.send(OsrHostEvent::ControlLine(line)).is_err() {
                break;
            }
            proxy.wake_up();
        }
    });
}
