//! DirectComposition (visual) hosting for guest WebView2s.
//!
//! Guests render into a non-topmost DComp tree on the top-level HWND, which
//! sits *under* the primary WebView2 child HWND. A `SetWindowRgn` hole in
//! the primary host lets the guest show through and receive hit-testing;
//! HTML dialogs in the primary UI stay on top by clearing those holes.

#![cfg(target_os = "windows")]

use std::sync::{Arc, Weak};

use fenestra_bridge::guest::GuestBounds;
use webview2_com::{
    CreateCoreWebView2CompositionControllerCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MOUSE_EVENT_KIND, COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS,
        COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC, ICoreWebView2CompositionController,
        ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2Environment3,
    },
};
use windows::{
    core::Interface,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            DirectComposition::{
                DCompositionCreateDevice2, IDCompositionDevice, IDCompositionTarget,
                IDCompositionVisual,
            },
            Gdi::{CombineRgn, CreateRectRgnIndirect, SetWindowRgn, RGN_DIFF},
        },
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                GetClientRect, SetCursor, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
                WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDBLCLK, WM_XBUTTONDOWN,
                WM_XBUTTONUP,
            },
        },
    },
};

use crate::{
    WebView2Error, WebView2ProcessInner, WebView2Result,
    windows::{bridge, guest_host},
};

const SUBCLASS_ID: usize = 0xFE_5E_57_2A;

/// Root DirectComposition tree attached to the top-level window.
pub(crate) struct DCompRoot {
    pub device: IDCompositionDevice,
    pub _target: IDCompositionTarget,
    pub guest_layer: IDCompositionVisual,
}

impl DCompRoot {
    pub(crate) fn create(top_level_hwnd: isize) -> WebView2Result<Self> {
        let hwnd = HWND(top_level_hwnd as *mut _);
        unsafe {
            let device: IDCompositionDevice = DCompositionCreateDevice2(None).map_err(|error| {
                WebView2Error::Backend(format!("DCompositionCreateDevice2: {error}"))
            })?;
            // Non-topmost → behind every WS_CHILD (including the primary host).
            let target = device
                .CreateTargetForHwnd(hwnd, false)
                .map_err(|error| WebView2Error::Backend(format!("CreateTargetForHwnd: {error}")))?;
            let root = device
                .CreateVisual()
                .map_err(|error| WebView2Error::Backend(format!("CreateVisual(root): {error}")))?;
            target
                .SetRoot(&root)
                .map_err(|error| WebView2Error::Backend(format!("SetRoot: {error}")))?;
            let guest_layer = device.CreateVisual().map_err(|error| {
                WebView2Error::Backend(format!("CreateVisual(guest_layer): {error}"))
            })?;
            root.AddVisual(&guest_layer, true, None).map_err(|error| {
                WebView2Error::Backend(format!("AddVisual(guest_layer): {error}"))
            })?;
            device
                .Commit()
                .map_err(|error| WebView2Error::Backend(format!("DComp Commit: {error}")))?;
            Ok(Self {
                device,
                _target: target,
                guest_layer,
            })
        }
    }

    pub(crate) fn commit(&self) -> WebView2Result<()> {
        unsafe { self.device.Commit() }
            .map_err(|error| WebView2Error::Backend(format!("DComp Commit: {error}")))
    }
}

pub(crate) fn create_guest_visual(
    dcomp: &DCompRoot,
    bounds: GuestBounds,
) -> WebView2Result<IDCompositionVisual> {
    unsafe {
        let visual = dcomp
            .device
            .CreateVisual()
            .map_err(|error| WebView2Error::Backend(format!("CreateVisual(guest): {error}")))?;
        visual
            .SetOffsetX2(bounds.x as f32)
            .map_err(bridge::webview2_error)?;
        visual
            .SetOffsetY2(bounds.y as f32)
            .map_err(bridge::webview2_error)?;
        dcomp
            .guest_layer
            .AddVisual(&visual, true, None)
            .map_err(|error| WebView2Error::Backend(format!("AddVisual(guest): {error}")))?;
        dcomp.commit()?;
        Ok(visual)
    }
}

pub(crate) fn move_guest_visual(
    dcomp: &DCompRoot,
    visual: &IDCompositionVisual,
    bounds: GuestBounds,
) -> WebView2Result<()> {
    unsafe {
        visual
            .SetOffsetX2(bounds.x as f32)
            .map_err(bridge::webview2_error)?;
        visual
            .SetOffsetY2(bounds.y as f32)
            .map_err(bridge::webview2_error)?;
    }
    dcomp.commit()
}

pub(crate) fn remove_guest_visual(
    dcomp: &DCompRoot,
    visual: &IDCompositionVisual,
) -> WebView2Result<()> {
    let _ = unsafe { dcomp.guest_layer.RemoveVisual(visual) };
    dcomp.commit()
}

pub(crate) fn create_composition_controller(
    top_level_hwnd: isize,
    environment: &ICoreWebView2Environment,
    visual: &IDCompositionVisual,
) -> WebView2Result<(ICoreWebView2CompositionController, ICoreWebView2Controller)> {
    let env3: ICoreWebView2Environment3 = environment
        .cast()
        .map_err(|error| WebView2Error::Backend(format!("ICoreWebView2Environment3: {error}")))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let handler = CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(
        move |status, controller| {
            let result = (|| {
                status?;
                controller.ok_or_else(|| {
                    windows::core::Error::from(windows::core::HRESULT(0x80004003u32 as i32))
                })
            })();
            tx.send(result).map_err(|_| {
                windows::core::Error::from(windows::core::HRESULT(0x80000004u32 as i32))
            })
        },
    ));
    unsafe {
        env3.CreateCoreWebView2CompositionController(HWND(top_level_hwnd as *mut _), &handler)
            .map_err(|error| {
                WebView2Error::Backend(format!("CreateCoreWebView2CompositionController: {error}"))
            })?;
    }
    let composition = guest_host::wait_bounded(
        rx,
        "guest composition controller",
        guest_host::CONTROLLER_CREATE_TIMEOUT,
    )?
    .map_err(|error| {
        WebView2Error::Backend(format!("guest composition controller callback: {error}"))
    })?;
    unsafe {
        composition
            .SetRootVisualTarget(visual)
            .map_err(|error| WebView2Error::Backend(format!("SetRootVisualTarget: {error}")))?;
    }
    let controller: ICoreWebView2Controller = composition
        .cast()
        .map_err(|error| WebView2Error::Backend(format!("composition as controller: {error}")))?;
    Ok((composition, controller))
}

/// Punch guest rectangles out of the primary host so DComp guests show
/// through and receive mouse hit-testing.
pub(crate) fn set_primary_holes(primary_host: isize, holes: &[GuestBounds]) {
    if primary_host == 0 {
        return;
    }
    let hwnd = HWND(primary_host as *mut _);
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_err() {
        return;
    }
    unsafe {
        let region = CreateRectRgnIndirect(&client);
        if region.is_invalid() {
            return;
        }
        for hole in holes {
            let rect = RECT {
                left: hole.x,
                top: hole.y,
                right: hole.x + hole.width.max(1) as i32,
                bottom: hole.y + hole.height.max(1) as i32,
            };
            let cut = CreateRectRgnIndirect(&rect);
            if !cut.is_invalid() {
                let _ = CombineRgn(Some(region), Some(region), Some(cut), RGN_DIFF);
                let _ = windows::Win32::Graphics::Gdi::DeleteObject(
                    windows::Win32::Graphics::Gdi::HGDIOBJ(cut.0),
                );
            }
        }
        // SetWindowRgn takes ownership of the region.
        let _ = SetWindowRgn(hwnd, Some(region), true);
    }
}

pub(crate) fn clear_primary_holes(primary_host: isize) {
    if primary_host == 0 {
        return;
    }
    unsafe {
        let _ = SetWindowRgn(HWND(primary_host as *mut _), None, true);
    }
}

struct SubclassState {
    inner: Weak<WebView2ProcessInner>,
}

pub(crate) fn attach_input_subclass(top_level_hwnd: isize, inner: Arc<WebView2ProcessInner>) {
    if top_level_hwnd == 0 {
        return;
    }
    let state = Box::new(SubclassState {
        inner: Arc::downgrade(&inner),
    });
    let ptr = Box::into_raw(state) as usize;
    let ok = unsafe {
        SetWindowSubclass(
            HWND(top_level_hwnd as *mut _),
            Some(subclass_proc),
            SUBCLASS_ID,
            ptr,
        )
    };
    if !ok.as_bool() {
        let _ = unsafe { Box::from_raw(ptr as *mut SubclassState) };
    }
}

pub(crate) fn detach_input_subclass(top_level_hwnd: isize) {
    if top_level_hwnd == 0 {
        return;
    }
    let _ = unsafe {
        RemoveWindowSubclass(
            HWND(top_level_hwnd as *mut _),
            Some(subclass_proc),
            SUBCLASS_ID,
        )
    };
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    ref_data: usize,
) -> LRESULT {
    if ref_data == 0 {
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &mut *(ref_data as *mut SubclassState) };
    if is_mouse_message(msg) {
        if let Some(inner) = state.inner.upgrade() {
            if forward_mouse(&inner, state, hwnd, msg, wparam, lparam) {
                return LRESULT(0);
            }
        }
    }
    if msg == windows::Win32::UI::WindowsAndMessaging::WM_NCDESTROY {
        let _ = unsafe { Box::from_raw(ref_data as *mut SubclassState) };
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn is_mouse_message(msg: u32) -> bool {
    matches!(
        msg,
        WM_MOUSEMOVE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_RBUTTONDBLCLK
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_MBUTTONDBLCLK
            | WM_XBUTTONDOWN
            | WM_XBUTTONUP
            | WM_XBUTTONDBLCLK
            | WM_MOUSEWHEEL
    )
}

fn forward_mouse(
    inner: &Arc<WebView2ProcessInner>,
    _state: &mut SubclassState,
    _hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> bool {
    if inner
        .guests_covered
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return false;
    }
    let Ok(manager) = inner.guests.try_lock() else {
        return false;
    };
    let Some(target) = manager.composition_hit_test(client_point(lparam)) else {
        return false;
    };

    if matches!(
        msg,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    ) {
        let _ = unsafe {
            target
                .controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        };
    }

    let point = POINT {
        x: client_point(lparam).0 - target.bounds.x,
        y: client_point(lparam).1 - target.bounds.y,
    };
    let kind = COREWEBVIEW2_MOUSE_EVENT_KIND(msg as i32);
    let keys = COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(wparam.0 as i32);
    let mouse_data = if msg == WM_MOUSEWHEEL {
        ((wparam.0 as u32) >> 16) & 0xffff
    } else {
        0
    };
    let _ = unsafe { target.composition.SendMouseInput(kind, keys, mouse_data, point) };

    let mut cursor = windows::Win32::UI::WindowsAndMessaging::HCURSOR::default();
    if unsafe { target.composition.Cursor(&mut cursor) }.is_ok() && !cursor.is_invalid() {
        let _ = unsafe { SetCursor(Some(cursor)) };
    }
    true
}

fn client_point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xffff) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
    (x, y)
}

/// Hit-test result for a composition-hosted guest.
pub(crate) struct CompositionHit {
    pub composition: ICoreWebView2CompositionController,
    pub controller: ICoreWebView2Controller,
    pub bounds: GuestBounds,
}
