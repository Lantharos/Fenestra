use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use winit::event::MouseButton;

use crate::SabineWindowChrome;
use crate::osr::frame_buffer::FrameBuffer;
use crate::osr::protocol::{OsrFrame, OsrMessage, OsrSurface, POPUP_OVERLAY_ID, POPUP_TEXTURE_ID};
use crate::osr::transport::IpcStream;

pub(super) const TITLEBAR_HEIGHT: f32 = 38.0;
pub(super) const CONTROL_SIZE: f32 = 24.0;
pub(super) const CONTROL_GAP: f32 = 8.0;
pub(super) const RESIZE_EDGE: f32 = 7.0;
pub(super) const CLOSE_GRACE: Duration = Duration::from_millis(300);
pub(super) const RESIZE_REPAINT_RETRY: Duration = Duration::from_millis(100);
pub(super) const RESIZE_REPAINT_GRACE: Duration = Duration::from_millis(900);
pub(super) const FALLBACK_ACTIVE_FRAME_RATE: u32 = 60;
/// Wayland briefly drops focus/occlusion around interactive move. Suspending
/// immediately thrashes lifecycle on the secondary (handed-off) window.
pub(super) const LIFECYCLE_SUSPEND_DEBOUNCE: Duration = Duration::from_millis(200);
pub(super) const LOADING_REVEAL_DELAY: Duration = Duration::from_millis(120);
pub(in crate::osr) const LOADING_ANIMATION_INTERVAL: Duration = Duration::from_millis(100);
pub(in crate::osr) const LOADING_MESSAGE_INTERVAL: Duration = Duration::from_millis(3_200);
pub(super) const TOOLTIP_REVEAL_DELAY: Duration = Duration::from_millis(500);

pub(super) const EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
pub(super) const EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
pub(super) const EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
pub(super) const EVENTFLAG_LEFT_MOUSE_BUTTON: u32 = 1 << 4;
pub(super) const EVENTFLAG_MIDDLE_MOUSE_BUTTON: u32 = 1 << 5;
pub(super) const EVENTFLAG_RIGHT_MOUSE_BUTTON: u32 = 1 << 6;
pub(super) const EVENTFLAG_COMMAND_DOWN: u32 = 1 << 7;
pub(super) const EVENTFLAG_IS_REPEAT: u32 = 1 << 13;
pub(super) const EVENTFLAG_PRECISION_SCROLLING_DELTA: u32 = 1 << 14;

pub(super) struct OverlayLayer {
    pub(super) frame: OsrFrame,
    pub(super) buffer: FrameBuffer,
}

pub(super) fn overlay_texture_id(overlay_id: &str) -> String {
    if overlay_id == POPUP_OVERLAY_ID {
        POPUP_TEXTURE_ID.to_string()
    } else {
        format!("__sabine_guest_{overlay_id}")
    }
}

pub(super) fn overlay_id_for_surface(surface: &OsrSurface) -> Option<String> {
    surface.overlay_id().map(str::to_string)
}

pub(super) fn uses_sabine_chrome(chrome: SabineWindowChrome) -> bool {
    matches!(chrome, SabineWindowChrome::Sabine)
}

pub(super) enum OsrHostEvent {
    Connected(u64, IpcStream),
    Message(u64, OsrMessage),
    MessagesReady(u64, Arc<crate::osr::message_queue::MessageQueue>),
    HostControl(HostControl),
    /// Forward a bridge or guest-control line to the owning CEF handler socket.
    ControlLine(String),
    Disconnected(u64),
}

impl OsrHostEvent {
    pub(super) fn connection_generation(&self) -> Option<u64> {
        match self {
            Self::Connected(generation, _)
            | Self::Message(generation, _)
            | Self::MessagesReady(generation, _)
            | Self::Disconnected(generation) => Some(*generation),
            Self::HostControl(_) | Self::ControlLine(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::osr) enum LoadingKind {
    Opening,
    Resuming,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::osr) struct NativeLoading {
    pub(in crate::osr) kind: LoadingKind,
    pub(in crate::osr) started: Instant,
    pub(in crate::osr) reveal_at: Instant,
    pub(in crate::osr) next_frame: Instant,
    pub(in crate::osr) message_seed: u64,
}

#[derive(Clone, Debug)]
pub(super) struct NativeTooltip {
    pub(super) text: String,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) reveal_at: Instant,
    pub(super) shown: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ImeSurrounding {
    pub(super) text: String,
    pub(super) cursor: usize,
    pub(super) anchor: usize,
    pub(super) base_utf16: usize,
}

#[derive(Clone, Debug)]
pub(super) struct ImePreedit {
    pub(super) text: String,
    pub(super) selection: Option<(usize, usize)>,
}

impl NativeTooltip {
    pub(super) fn new(text: String, x: f32, y: f32) -> Self {
        Self {
            text,
            x,
            y,
            reveal_at: Instant::now() + TOOLTIP_REVEAL_DELAY,
            shown: false,
        }
    }
}

impl NativeLoading {
    pub(in crate::osr) fn new(kind: LoadingKind) -> Self {
        let started = Instant::now();
        let reveal_at = started + LOADING_REVEAL_DELAY;
        Self {
            kind,
            started,
            reveal_at,
            next_frame: reveal_at,
            message_seed: next_loading_seed(),
        }
    }

    pub(super) fn revealed(self) -> bool {
        Instant::now() >= self.reveal_at
    }
}

fn next_loading_seed() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    time ^ u64::from(std::process::id()) ^ SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TitlebarControl {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LifecycleState {
    Active,
    Suspended,
    Hibernating,
    Hibernated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HostControl {
    Show,
    Hide,
    Focus(Option<String>),
    Visible(bool),
    Quit,
    ActivityBegin(HostActivity),
    ActivityEnd(HostActivity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HostActivity {
    pub(super) id: String,
    pub(super) prevents_hibernation: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ControlRect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl ControlRect {
    pub(super) fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MouseButtons {
    pub(super) left: bool,
    pub(super) middle: bool,
    pub(super) right: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ClickMemory {
    pub(super) button: MouseButton,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) at: Instant,
    pub(super) count: i32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingResizePaint {
    pub(super) size: (u32, u32),
    pub(super) retry_at: Instant,
    pub(super) deadline: Instant,
}
