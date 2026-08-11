use std::{process::Child, time::Instant};

use layershellev::{WindowState, calloop::channel::Sender, id, reexport::wl_shm::WlShm};
use smithay_client_toolkit::shm::slot::{Buffer as ShmBuffer, SlotPool};
use wayland_client::QueueHandle;

use crate::osr::host::OsrHostConfig;
use crate::osr::protocol::OsrFrame;

use super::alpha::LayerAlphaModifier;
use super::socket::{ControlWriter, LayerHostEvent};

pub(super) struct OsrLayerHost {
    pub(super) config: OsrHostConfig,
    pub(super) sender: Sender<LayerHostEvent>,
    pub(super) child: Option<Child>,
    pub(super) control_writer: Option<ControlWriter>,
    pub(super) shm: Option<WlShm>,
    pub(super) queue_handle: Option<QueueHandle<WindowState<()>>>,
    pub(super) main_pool: Option<SlotPool>,
    pub(super) main_buffers: Vec<ShmBuffer>,
    pub(super) pending_surface_refresh: bool,
    pub(super) buffer_size: (u32, u32),
    pub(super) surface_size: (u32, u32),
    pub(super) scale: f64,
    pub(super) main_frame: Option<OsrFrame>,
    pub(super) main_frame_surface_size: Option<(u32, u32)>,
    pub(super) popup: Option<PopupSurface>,
    pub(super) main_buffer: Vec<u8>,
    pub(super) scratch: Vec<u8>,
    pub(super) surface_mapped: bool,
    pub(super) visible: bool,
    pub(super) cursor_shape: String,
    pub(super) cursor_x: f32,
    pub(super) cursor_y: f32,
    pub(super) pointer_inside: bool,
    pub(super) modifiers: layershellev::keyboard::ModifiersState,
    pub(super) mouse: MouseButtons,
    pub(super) last_click: Option<ClickMemory>,
    pub(super) active_click_count: i32,
    pub(super) focused: bool,
    pub(super) lifecycle_state: LayerLifecycleState,
    pub(super) alpha_modifier: Option<LayerAlphaModifier>,
    pub(super) surface_alpha: f32,
}

pub(super) struct PopupSurface {
    pub(super) id: id::Id,
    pub(super) position: (i32, i32),
    pub(super) size: (u32, u32),
    pub(super) frame: Option<OsrFrame>,
    pub(super) pending_frames: Vec<OsrFrame>,
    pub(super) pool: Option<SlotPool>,
    pub(super) buffers: Vec<ShmBuffer>,
    pub(super) pending_refresh: bool,
    pub(super) buffer: Vec<u8>,
    pub(super) scratch: Vec<u8>,
    pub(super) mapped: bool,
    pub(super) effect: Option<sabine_platform::WindowEffect>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MouseButtons {
    pub(super) left: bool,
    pub(super) middle: bool,
    pub(super) right: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ClickMemory {
    pub(super) button: u32,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) at: Instant,
    pub(super) count: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayerLifecycleState {
    Active,
    Suspended,
}

impl OsrLayerHost {
    pub(super) fn new(config: OsrHostConfig, sender: Sender<LayerHostEvent>) -> Self {
        super::socket::start_layer_parent_bridge_reader(sender.clone());
        let surface_size = (config.width.max(1), config.height.max(1));
        let visible = config.visible;
        let surface_alpha = if visible {
            config.shell_surface_alpha.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let focused = config.active;
        let lifecycle_state = if visible {
            LayerLifecycleState::Active
        } else {
            LayerLifecycleState::Suspended
        };
        Self {
            config,
            sender,
            child: None,
            control_writer: None,
            shm: None,
            queue_handle: None,
            main_pool: None,
            main_buffers: Vec::new(),
            pending_surface_refresh: false,
            buffer_size: surface_size,
            surface_size,
            scale: 1.0,
            main_frame: None,
            main_frame_surface_size: None,
            popup: None,
            main_buffer: Vec::new(),
            scratch: Vec::new(),
            surface_mapped: false,
            visible,
            cursor_shape: "default".to_string(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            pointer_inside: false,
            modifiers: Default::default(),
            mouse: MouseButtons::default(),
            last_click: None,
            active_click_count: 1,
            focused,
            lifecycle_state,
            alpha_modifier: None,
            surface_alpha,
        }
    }
}

impl Drop for OsrLayerHost {
    fn drop(&mut self) {
        // Leave CEF running if other OSR handlers still need the process.
        // Socket EOF closes this browser; last handler quits the message loop.
        if let Some(mut child) = self.child.take() {
            let _ = child.try_wait();
        }
    }
}
