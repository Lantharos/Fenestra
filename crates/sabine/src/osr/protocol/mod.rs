mod config_json;
mod encode;
mod wire;

pub(crate) use config_json::{
    control_regions_from_json, control_regions_to_json, lifecycle_from_json, lifecycle_to_json,
    rects_from_json, rects_to_json, regions_from_json, regions_to_json, shell_surface_from_json,
    shell_surface_to_json,
};
pub(crate) use encode::encode_component;
pub(crate) use wire::read_message;

use sabine_platform::WindowRegionRect;

pub(crate) const MAIN_TEXTURE_ID: &str = "__sabine_main";
pub(crate) const POPUP_TEXTURE_ID: &str = "__sabine_popup";
pub(crate) const POPUP_OVERLAY_ID: &str = "__sabine_popup";

#[derive(Debug)]
pub(crate) enum OsrMessage {
    Frame(OsrFrame),
    PaintBatch(OsrPaintBatch),
    AccelFrame(OsrAccelFrame),
    /// Hide the legacy single popup overlay (`__sabine_popup`).
    PopupHidden,
    /// Hide a guest overlay by id.
    GuestHidden(String),
    GuestCaptureRequested {
        browser_id: String,
        request_id: String,
        guest_id: String,
    },
    DraggableRegionsChanged {
        drag: Vec<WindowRegionRect>,
        exclusion: Vec<WindowRegionRect>,
    },
    Cursor(String),
    CloseRequested,
    StartDragRequested,
    MinimizeRequested,
    ToggleMaximizeRequested,
    ShowRequested,
    HideRequested,
    FocusRequested,
    FileDragRequested(FileDragRequest),
    /// Full `SABINE_BRIDGE_REQUEST\t...` line from the owning CEF handler.
    BridgeRequest(String),
}

#[derive(Clone, Debug)]
pub(crate) struct FileDragRequest {
    pub paths: Vec<String>,
    #[allow(dead_code)]
    pub x: i32,
    #[allow(dead_code)]
    pub y: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct OsrPaintBatch {
    pub surface: OsrSurface,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub frames: Vec<OsrFrame>,
}

#[derive(Clone, Debug)]
pub(crate) struct OsrAccelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub(crate) struct OsrAccelFrame {
    pub surface: OsrSurface,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub format: u32,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub modifier: u64,
    pub stride: u32,
    pub offset: u64,
    pub size: u64,
    /// Platform native share handle: Linux unused (0), Windows `HANDLE`, macOS `IOSurfaceID`.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub native_handle: u64,
    pub dirty: Vec<OsrAccelRect>,
    pub fd: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct OsrFrame {
    pub surface: OsrSurface,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OsrSurface {
    Main,
    /// Legacy popup overlay (also addressable as guest id `__sabine_popup`).
    Popup,
    /// Named guest overlay composited above the main surface.
    Guest(String),
}

impl OsrSurface {
    pub(crate) fn overlay_id(&self) -> Option<&str> {
        match self {
            Self::Main => None,
            Self::Popup => Some(POPUP_OVERLAY_ID),
            Self::Guest(id) => Some(id.as_str()),
        }
    }
}
