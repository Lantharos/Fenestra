use std::{process::Child, time::Instant};

use layershellev::{
    WindowState, blur::BlurOption, calloop::channel::Sender, id, reexport::wl_shm::WlShm,
};
use smithay_client_toolkit::shm::slot::{Buffer as ShmBuffer, SlotPool};
use wayland_client::QueueHandle;

use crate::osr::control::ControlWriter;
use crate::osr::host::OsrHostConfig;
use crate::osr::protocol::OsrFrame;
use crate::render::RasterText;

use super::alpha::LayerAlphaModifier;
use super::socket::{LayerHostEvent, LayerSocketHandle};

pub(super) struct OsrLayerHost {
    pub(super) config: OsrHostConfig,
    pub(super) sender: Sender<LayerHostEvent>,
    pub(super) child: Option<Child>,
    pub(super) child_retry_at: Option<Instant>,
    pub(super) child_handoff_deadline: Option<Instant>,
    pub(super) pending_socket: Option<LayerSocketHandle>,
    pub(super) control_writer: Option<ControlWriter>,
    pub(super) last_sent_content_size: Option<(u32, u32, u64)>,
    pub(super) shm: Option<WlShm>,
    pub(super) queue_handle: Option<QueueHandle<WindowState<()>>>,
    pub(super) main_pool: Option<SlotPool>,
    pub(super) main_pool_error: Option<String>,
    pub(super) main_buffers: Vec<ShmBuffer>,
    pub(super) pending_surface_refresh: bool,
    pub(super) pending_surface_damage: Option<super::buffer::DamageRect>,
    pub(super) buffer_size: (u32, u32),
    pub(super) surface_size: (u32, u32),
    pub(super) scale: f64,
    pub(super) main_frame: Option<OsrFrame>,
    pub(super) main_frame_surface_size: Option<(u32, u32)>,
    pub(super) main_load_ready: bool,
    pub(super) popup: Option<PopupSurface>,
    pub(super) main_buffer: Vec<u8>,
    pub(super) presentation_buffer: Vec<u8>,
    pub(super) presentation_full_damage: bool,
    pub(super) scratch: Vec<u8>,
    pub(super) surface_lifecycle: LayerSurfaceLifecycle,
    pub(super) layer_layout_dirty: bool,
    pub(super) presentation_dirty: bool,
    pub(super) configure_generation: u64,
    pub(super) next_remap_sync_token: u64,
    pub(super) wayland_failed: bool,
    pub(super) visible: bool,
    pub(super) pending_visibility_ack: Option<(u64, bool)>,
    pub(super) cursor_shape: String,
    pub(super) cursor_x: f32,
    pub(super) cursor_y: f32,
    pub(super) pointer_inside: bool,
    pub(super) modifiers: layershellev::keyboard::ModifiersState,
    pub(super) mouse: MouseButtons,
    pub(super) suppressed_mouse_releases: MouseButtons,
    pub(super) last_click: Option<ClickMemory>,
    pub(super) active_click_count: i32,
    pub(super) focused: bool,
    pub(super) lifecycle_state: LayerLifecycleState,
    pub(super) alpha_manager_name: Option<u32>,
    pub(super) alpha_modifier: Option<LayerAlphaModifier>,
    pub(super) surface_alpha: f32,
    pub(super) blur_option: Option<BlurOption>,
    pub(super) loading: Option<crate::osr::host::types::NativeLoading>,
    pub(super) text_renderer: RasterText,
    pub(super) tooltip: Option<LayerTooltip>,
}

pub(super) struct PopupSurface {
    pub(super) id: id::Id,
    pub(super) position: (i32, i32),
    pub(super) size: (u32, u32),
    pub(super) frame: Option<OsrFrame>,
    pub(super) pending_frames: Vec<OsrFrame>,
    pub(super) pool: Option<SlotPool>,
    pub(super) pool_error: Option<String>,
    pub(super) buffers: Vec<ShmBuffer>,
    pub(super) pending_refresh: bool,
    pub(super) pending_damage: Option<super::buffer::DamageRect>,
    pub(super) buffer: Vec<u8>,
    pub(super) scratch: Vec<u8>,
    pub(super) mapped: bool,
    pub(super) blur_configured: bool,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LayerSurfaceLifecycle {
    mapped: bool,
    barrier: LayerSurfaceBarrier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayerSurfaceBarrier {
    Ready,
    DrainBeforeUnmap,
    SyncBeforeRemap(u64),
    ConfigureBeforePresent(u64),
}

impl LayerSurfaceLifecycle {
    pub(super) const fn new() -> Self {
        Self {
            mapped: false,
            barrier: LayerSurfaceBarrier::Ready,
        }
    }

    pub(super) const fn is_mapped(self) -> bool {
        self.mapped
    }

    pub(super) const fn presentation_ready(self) -> bool {
        matches!(self.barrier, LayerSurfaceBarrier::Ready)
    }

    pub(super) const fn unmap_pending(self) -> bool {
        matches!(self.barrier, LayerSurfaceBarrier::DrainBeforeUnmap)
    }

    pub(super) fn mark_mapped(&mut self) {
        self.mapped = true;
        self.barrier = LayerSurfaceBarrier::Ready;
    }

    pub(super) fn schedule_unmap(&mut self) -> bool {
        if !self.mapped || self.unmap_pending() {
            return false;
        }
        self.barrier = LayerSurfaceBarrier::DrainBeforeUnmap;
        true
    }

    pub(super) fn cancel_scheduled_unmap(&mut self) -> bool {
        if !self.unmap_pending() {
            return false;
        }
        self.barrier = LayerSurfaceBarrier::Ready;
        true
    }

    pub(super) fn complete_unmap(&mut self, sync_token: u64) -> bool {
        if !self.unmap_pending() {
            return false;
        }
        self.mapped = false;
        self.barrier = LayerSurfaceBarrier::SyncBeforeRemap(sync_token);
        true
    }

    pub(super) fn complete_sync(&mut self, sync_token: u64) -> bool {
        if self.barrier != LayerSurfaceBarrier::SyncBeforeRemap(sync_token) {
            return false;
        }
        self.barrier = LayerSurfaceBarrier::Ready;
        true
    }

    pub(super) fn wait_for_configure(&mut self, configure_generation: u64) {
        self.barrier = LayerSurfaceBarrier::ConfigureBeforePresent(configure_generation);
    }

    pub(super) fn accept_configure(&mut self, configure_generation: u64) -> bool {
        match self.barrier {
            LayerSurfaceBarrier::Ready => true,
            LayerSurfaceBarrier::ConfigureBeforePresent(previous_generation)
                if configure_generation > previous_generation =>
            {
                self.barrier = LayerSurfaceBarrier::Ready;
                true
            }
            LayerSurfaceBarrier::DrainBeforeUnmap
            | LayerSurfaceBarrier::SyncBeforeRemap(_)
            | LayerSurfaceBarrier::ConfigureBeforePresent(_) => false,
        }
    }
}

impl OsrLayerHost {
    pub(super) fn new(config: OsrHostConfig, sender: Sender<LayerHostEvent>) -> Self {
        super::socket::start_layer_parent_bridge_reader(sender.clone());
        let surface_size = (config.width.max(1), config.height.max(1));
        let visible = config.visible;
        let surface_alpha = config.shell_surface_alpha.clamp(0.0, 1.0);
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
            child_retry_at: None,
            child_handoff_deadline: None,
            pending_socket: None,
            control_writer: None,
            last_sent_content_size: None,
            shm: None,
            queue_handle: None,
            main_pool: None,
            main_pool_error: None,
            main_buffers: Vec::new(),
            pending_surface_refresh: false,
            pending_surface_damage: None,
            buffer_size: surface_size,
            surface_size,
            scale: 1.0,
            main_frame: None,
            main_frame_surface_size: None,
            main_load_ready: false,
            popup: None,
            main_buffer: Vec::new(),
            presentation_buffer: Vec::new(),
            presentation_full_damage: false,
            scratch: Vec::new(),
            surface_lifecycle: LayerSurfaceLifecycle::new(),
            layer_layout_dirty: false,
            presentation_dirty: false,
            configure_generation: 0,
            next_remap_sync_token: 0,
            wayland_failed: false,
            visible,
            pending_visibility_ack: None,
            cursor_shape: "default".to_string(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            pointer_inside: false,
            modifiers: Default::default(),
            mouse: MouseButtons::default(),
            suppressed_mouse_releases: MouseButtons::default(),
            last_click: None,
            active_click_count: 1,
            focused,
            lifecycle_state,
            alpha_manager_name: None,
            alpha_modifier: None,
            surface_alpha,
            blur_option: None,
            loading: visible.then(|| {
                crate::osr::host::types::NativeLoading::new(
                    crate::osr::host::types::LoadingKind::Opening,
                )
            }),
            text_renderer: RasterText::new(),
            tooltip: None,
        }
    }
}

pub(super) struct LayerTooltip {
    pub(super) text: String,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) reveal_at: Instant,
    pub(super) shown: bool,
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
