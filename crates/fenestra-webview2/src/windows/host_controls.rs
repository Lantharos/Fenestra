// Win32 host-control calls: show / hide / focus / minimize / maximize,
// plus DWM glass. These are the simple synchronous calls the launch
// loop uses to drive the window from `WebView2UserEvent`. The
// signatures use the `windows 0.60` series to match what
// `webview2-com 0.36` brings in.

#![cfg(target_os = "windows")]

use std::{
    collections::HashMap,
    sync::Mutex,
};

use fenestra_platform::WindowBackgroundEffect;
use raw_window_handle::HasWindowHandle;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_MAINWINDOW, DWMSBT_NONE,
            DWMSBT_TABBEDWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR,
            DWMWA_COLOR_NONE, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmExtendFrameIntoClientArea,
            DwmSetWindowAttribute,
        },
        Gdi::{
            BLACK_BRUSH, FillRect, GetDC, GetMonitorInfoW, GetStockObject, HBRUSH, InvalidateRect,
            MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromWindow, ReleaseDC, ScreenToClient,
        },
    },
    UI::{
        Controls::MARGINS,
        WindowsAndMessaging::{
            BringWindowToTop, CallWindowProcW, GCLP_HBRBACKGROUND, GWLP_WNDPROC, GWL_EXSTYLE,
            GWL_STYLE, GetClientRect, GetWindowRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT,
            HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed, SW_HIDE, SW_MAXIMIZE,
            SW_MINIMIZE, SW_RESTORE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOZORDER, SetClassLongPtrW, SetForegroundWindow, SetWindowLongPtrW,
            SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCACTIVATE, WM_NCCALCSIZE,
            WM_NCHITTEST, WM_NCPAINT, WNDPROC, WS_EX_LAYERED, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
            WS_POPUP, WS_THICKFRAME,
        },
    },
};

use crate::{WebView2Config, WebView2Result};

static FRAMELESS_ORIG_WNDPROC: Mutex<Option<HashMap<isize, isize>>> = Mutex::new(None);

/// Pixel band used for frameless edge-resize hit testing.
pub(crate) const FRAMELESS_RESIZE_BORDER: i32 = 6;

/// Start a Win32 non-client resize drag for a frameless window.
pub(crate) fn begin_resize(hwnd: isize, hit: &str) -> bool {
    if hwnd == 0 {
        return false;
    }
    use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows::Win32::UI::WindowsAndMessaging::{
        HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT,
        SendMessageW, WM_NCLBUTTONDOWN,
    };
    let ht = match hit {
        "left" | "HTLEFT" => HTLEFT,
        "right" | "HTRIGHT" => HTRIGHT,
        "top" | "HTTOP" => HTTOP,
        "bottom" | "HTBOTTOM" => HTBOTTOM,
        "top-left" | "HTTOPLEFT" => HTTOPLEFT,
        "top-right" | "HTTOPRIGHT" => HTTOPRIGHT,
        "bottom-left" | "HTBOTTOMLEFT" => HTBOTTOMLEFT,
        "bottom-right" | "HTBOTTOMRIGHT" => HTBOTTOMRIGHT,
        _ => return false,
    };
    let win = HWND(hwnd as *mut _);
    unsafe {
        let _ = ReleaseCapture();
        SendMessageW(
            win,
            WM_NCLBUTTONDOWN,
            Some(WPARAM(ht as usize)),
            Some(LPARAM(0)),
        );
    }
    true
}

/// Show the window. Equivalent to `ShowWindow(hwnd, SW_SHOW)`.
pub(crate) fn show_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_SHOW) }.as_bool()
}

/// Hide the window. Equivalent to `ShowWindow(hwnd, SW_HIDE)`.
pub(crate) fn hide_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_HIDE) }.as_bool()
}

/// Bring the window to the foreground. Calls
/// `SetForegroundWindow(hwnd)` after a `BringWindowToTop(hwnd)` for
/// older Win32 compatibility.
pub(crate) fn focus_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let hwnd = HWND(hwnd as *mut _);
    let _ = unsafe { BringWindowToTop(hwnd) };
    unsafe { SetForegroundWindow(hwnd) }.as_bool()
}

/// Minimize the window. Equivalent to `ShowWindow(hwnd, SW_MINIMIZE)`.
pub(crate) fn minimize_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_MINIMIZE) }.as_bool()
}

/// Maximize the window. Equivalent to `ShowWindow(hwnd, SW_MAXIMIZE)`.
pub(crate) fn maximize_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_MAXIMIZE) }.as_bool()
}

/// Restore a minimized or maximized window to its previous size.
/// Equivalent to `ShowWindow(hwnd, SW_RESTORE)`.
pub(crate) fn unmaximize_window(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { ShowWindow(HWND(hwnd as *mut _), SW_RESTORE) }.as_bool()
}

/// Whether Win32 currently considers the window maximized (`IsZoomed`).
pub(crate) fn is_zoomed(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe { IsZoomed(HWND(hwnd as *mut _)) }.as_bool()
}

/// Maximize a borderless window to the monitor work area.
///
/// Do **not** use `SW_MAXIMIZE` here — on Win11 it reintroduces the
/// default caption even with `WM_NCCALCSIZE` handling, and with a fully
/// client-area frame it covers the taskbar. Returns the previous outer
/// rect so the caller can restore it.
pub(crate) fn maximize_frameless(hwnd: isize) -> Option<RECT> {
    if hwnd == 0 {
        return None;
    }
    if is_zoomed(hwnd) {
        let _ = unmaximize_window(hwnd);
    }
    let win = HWND(hwnd as *mut _);
    let mut previous = RECT::default();
    unsafe { GetWindowRect(win, &mut previous) }.ok()?;
    let work = monitor_work_area(win)?;
    let _ = unsafe {
        SetWindowPos(
            win,
            None,
            work.left,
            work.top,
            work.right - work.left,
            work.bottom - work.top,
            SWP_NOZORDER | SWP_FRAMECHANGED,
        )
    };
    apply_frameless_window(hwnd);
    Some(previous)
}

/// Restore a borderless window to a previously saved outer rect.
pub(crate) fn restore_frameless(hwnd: isize, rect: RECT) -> bool {
    if hwnd == 0 {
        return false;
    }
    if is_zoomed(hwnd) {
        let _ = unmaximize_window(hwnd);
    }
    let win = HWND(hwnd as *mut _);
    let ok = unsafe {
        SetWindowPos(
            win,
            None,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOZORDER | SWP_FRAMECHANGED,
        )
    }
    .is_ok();
    apply_frameless_window(hwnd);
    ok
}

/// If Windows snapped/maximized the HWND (`IsZoomed`), convert it to a
/// work-area fill so the system caption and taskbar-cover cannot stick.
pub(crate) fn suppress_system_maximize(hwnd: isize) -> Option<RECT> {
    if hwnd == 0 {
        return None;
    }
    if !is_zoomed(hwnd) {
        apply_frameless_window(hwnd);
        return None;
    }
    maximize_frameless(hwnd)
}

/// Whether this config should use DWM system backdrop glass.
pub(crate) fn wants_dwm_glass(config: &WebView2Config) -> bool {
    backdrop_for_effect(config.effective_background_effect()).is_some()
}

/// Strip `WS_EX_LAYERED` that winit's `with_transparent(true)` installs.
/// Layered windows fight `DWMWA_SYSTEMBACKDROP_TYPE` on many builds.
pub(crate) fn disable_layered_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let hwnd = HWND(hwnd as *mut _);
    let previous =
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let cleared = WINDOW_EX_STYLE(previous as u32) & !WS_EX_LAYERED;
    if cleared.0 as isize == previous {
        return;
    }
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, cleared.0 as isize);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Force a true borderless Win32 frame. winit's `with_decorations(false)`
/// is not always enough on Windows 11 — caption chrome can remain until
/// the style is rewritten to a popup + thick-frame combination and the
/// non-client frame is recalculated.
pub(crate) fn force_frameless_styles(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let hwnd = HWND(hwnd as *mut _);
    let previous =
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_STYLE) };
    let style = WINDOW_STYLE(previous as u32);
    // Keep visibility / clipping bits, drop every caption chrome flag
    // (including WS_MAXIMIZE — work-area maximize must not look zoomed).
    let preserved = style
        & (windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE
            | windows::Win32::UI::WindowsAndMessaging::WS_CLIPSIBLINGS
            | windows::Win32::UI::WindowsAndMessaging::WS_CLIPCHILDREN
            | windows::Win32::UI::WindowsAndMessaging::WS_MINIMIZE);
    let next = preserved
        | WS_POPUP
        | WS_THICKFRAME
        | WS_MINIMIZEBOX
        | WS_MAXIMIZEBOX;
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_STYLE, next.0 as isize);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    apply_frameless_dwm(hwnd);
}

fn apply_frameless_dwm(hwnd: HWND) {
    let corners = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corners as *const _ as *const _,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
    let none = DWMWA_COLOR_NONE;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &none as *const _ as *const _,
            std::mem::size_of_val(&none) as u32,
        )
    };
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &none as *const _ as *const _,
            std::mem::size_of_val(&none) as u32,
        )
    };
    // Win11 draws a 1px accent-colored top strip on borderless windows
    // (especially when maximized). Zero the visible frame border.
    const DWMWA_VISIBLE_FRAME_BORDER_THICKNESS: i32 = 37;
    let thickness: u32 = 0;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(
                DWMWA_VISIBLE_FRAME_BORDER_THICKNESS,
            ),
            &thickness as *const _ as *const _,
            std::mem::size_of_val(&thickness) as u32,
        )
    };
}

/// Apply frameless chrome without requiring a glass background effect.
pub(crate) fn apply_frameless_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    force_frameless_styles(hwnd);
    install_frameless_wndproc(hwnd);
}

fn install_frameless_wndproc(hwnd: isize) {
    let hwnd_value = hwnd;
    let hwnd = HWND(hwnd as *mut _);
    let mut guard = FRAMELESS_ORIG_WNDPROC
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if map.contains_key(&hwnd_value) {
        return;
    }
    let proc_ptr: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        frameless_wnd_proc;
    let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc_ptr as *const () as isize) };
    map.insert(hwnd_value, previous);
}

/// Eat the non-client area so even if caption styles flicker back during
/// resize/maximize, Windows cannot paint the default title bar. Edge resize
/// for frameless windows is driven by page-side hit strips that call
/// `begin_resize` (WebView2 stays edge-to-edge).
unsafe extern "system" fn frameless_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 => {
            // Entire window is client area — no system caption strip.
            return LRESULT(0);
        }
        WM_NCPAINT => {
            return LRESULT(0);
        }
        WM_NCACTIVATE => {
            return LRESULT(1);
        }
        WM_NCHITTEST => {
            if let Some(hit) = frameless_resize_hit_test(hwnd, lparam) {
                return LRESULT(hit);
            }
        }
        _ => {}
    }
    let hwnd_value = hwnd.0 as isize;
    let original = FRAMELESS_ORIG_WNDPROC
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .and_then(|map| map.get(&hwnd_value).copied())
        .unwrap_or(0);
    if original == 0 {
        return LRESULT(0);
    }
    let proc: WNDPROC = Some(unsafe {
        std::mem::transmute::<
            isize,
            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        >(original)
    });
    unsafe { CallWindowProcW(proc, hwnd, msg, wparam, lparam) }
}

fn frameless_resize_hit_test(hwnd: HWND, lparam: LPARAM) -> Option<isize> {
    if unsafe { IsZoomed(hwnd) }.as_bool() {
        return None;
    }
    let mut point = POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
        return None;
    }
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= FRAMELESS_RESIZE_BORDER * 2 || height <= FRAMELESS_RESIZE_BORDER * 2 {
        return None;
    }
    let border = FRAMELESS_RESIZE_BORDER;
    let left = point.x <= border;
    let right = point.x >= width - border;
    let top = point.y <= border;
    let bottom = point.y >= height - border;
    let hit = match (left, right, top, bottom) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, _, _, _) => HTLEFT,
        (_, true, _, _) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ => return None,
    };
    Some(hit as isize)
}

fn monitor_work_area(hwnd: HWND) -> Option<RECT> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    Some(info.rcWork)
}

/// Prepare the host HWND so DWM materials can show through.
pub(crate) fn prepare_transparent_host(hwnd: isize, frameless: bool) {
    if hwnd == 0 {
        return;
    }
    disable_layered_window(hwnd);
    if frameless {
        apply_frameless_window(hwnd);
    }
    let hwnd = HWND(hwnd as *mut _);
    // Use a real black brush — a null class brush has caused WebView2
    // controller creation to fail with E_INVALIDARG on some runtimes.
    unsafe {
        let brush = GetStockObject(BLACK_BRUSH);
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, brush.0 as isize);
    }
}

fn fill_client_black(hwnd: HWND) {
    let mut rect = RECT::default();
    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    unsafe {
        let hdc = GetDC(Some(hwnd));
        if !hdc.is_invalid() {
            let brush = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            let _ = FillRect(hdc, &rect, brush);
            let _ = ReleaseDC(Some(hwnd), hdc);
        }
    }
}

/// Apply DWM system backdrop for glass windows. Prefers the same path
/// Tauri uses (`window-vibrancy`) and also extends the frame + paints
/// the client black so a transparent WebView2 can reveal Mica/Acrylic.
pub(crate) fn apply_dwm_backdrop(hwnd: isize, config: &WebView2Config) -> WebView2Result<()> {
    if hwnd == 0 {
        return Ok(());
    }
    let effect = config.effective_background_effect();
    let Some(backdrop) = backdrop_for_effect(effect) else {
        return Ok(());
    };

    let frameless = config.frameless || !config.chrome.uses_native_decorations();
    prepare_transparent_host(hwnd, frameless);

    let win = HWND(hwnd as *mut _);
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    if let Err(error) = unsafe { DwmExtendFrameIntoClientArea(win, &margins) } {
        eprintln!("fenestra: DwmExtendFrameIntoClientArea failed: {error}");
    }
    fill_client_black(win);

    let dark_mode = windows::core::BOOL(1);
    let _ = unsafe {
        DwmSetWindowAttribute(
            win,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_mode as *const _ as *const _,
            std::mem::size_of_val(&dark_mode) as u32,
        )
    };

    let hr = unsafe {
        DwmSetWindowAttribute(
            win,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
    };
    if hr.is_err() {
        // Windows 11 21H2 undocumented mica flag.
        const DWMWA_MICA_EFFECT: u32 = 1029;
        let enable: u32 = 1;
        let _ = unsafe {
            DwmSetWindowAttribute(
                win,
                windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(DWMWA_MICA_EFFECT as i32),
                &enable as *const _ as *const _,
                std::mem::size_of_val(&enable) as u32,
            )
        };
        eprintln!(
            "fenestra: DWMWA_SYSTEMBACKDROP_TYPE failed ({hr:?}); tried DWMWA_MICA_EFFECT fallback"
        );
    }

    let _ = unsafe { InvalidateRect(Some(win), None, true) };
    Ok(())
}

/// Apply glass through `window-vibrancy` for a Win32 HWND. This mirrors
/// Tauri's working WebView2 + Mica setup on top of our DWM path.
pub(crate) fn apply_window_vibrancy(hwnd: isize, config: &WebView2Config) -> WebView2Result<()> {
    if hwnd == 0 {
        return Ok(());
    }
    let window = HwndHandle(hwnd);
    let effect = config.effective_background_effect();
    let result = match effect {
        WindowBackgroundEffect::Mica | WindowBackgroundEffect::Glass => {
            window_vibrancy::apply_mica(&window, Some(true))
        }
        WindowBackgroundEffect::MicaAlt => window_vibrancy::apply_tabbed(&window, Some(true)),
        WindowBackgroundEffect::Acrylic
        | WindowBackgroundEffect::Blur
        | WindowBackgroundEffect::Vibrancy
        | WindowBackgroundEffect::HudWindow
        | WindowBackgroundEffect::Sidebar
        | WindowBackgroundEffect::UnderWindowBackground => {
            window_vibrancy::apply_acrylic(&window, None)
        }
        WindowBackgroundEffect::None => return Ok(()),
    };
    if let Err(error) = result {
        eprintln!("fenestra: window-vibrancy glass failed: {error}");
    }
    Ok(())
}

struct HwndHandle(isize);

impl HasWindowHandle for HwndHandle {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let mut handle = raw_window_handle::Win32WindowHandle::new(
            std::num::NonZeroIsize::new(self.0)
                .ok_or(raw_window_handle::HandleError::Unavailable)?,
        );
        handle.hinstance = None;
        let raw = raw_window_handle::RawWindowHandle::Win32(handle);
        // SAFETY: the HWND remains valid for the duration of the
        // window-vibrancy call on the UI thread.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

pub(crate) fn client_rect(hwnd: isize) -> Option<RECT> {
    if hwnd == 0 {
        return None;
    }
    let mut rect = RECT::default();
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetClientRect(HWND(hwnd as *mut _), &mut rect)
    }
    .ok()?;
    Some(rect)
}

fn backdrop_for_effect(effect: WindowBackgroundEffect) -> Option<DWM_SYSTEMBACKDROP_TYPE> {
    match effect {
        WindowBackgroundEffect::None => None,
        WindowBackgroundEffect::Mica | WindowBackgroundEffect::Glass => Some(DWMSBT_MAINWINDOW),
        WindowBackgroundEffect::MicaAlt => Some(DWMSBT_TABBEDWINDOW),
        WindowBackgroundEffect::Acrylic
        | WindowBackgroundEffect::Blur
        | WindowBackgroundEffect::Vibrancy
        | WindowBackgroundEffect::HudWindow
        | WindowBackgroundEffect::Sidebar
        | WindowBackgroundEffect::UnderWindowBackground => Some(DWMSBT_TRANSIENTWINDOW),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mica_alt_uses_tabbed_backdrop() {
        assert_eq!(
            backdrop_for_effect(WindowBackgroundEffect::MicaAlt),
            Some(DWMSBT_TABBEDWINDOW)
        );
        assert_eq!(
            backdrop_for_effect(WindowBackgroundEffect::Mica),
            Some(DWMSBT_MAINWINDOW)
        );
        assert_eq!(
            backdrop_for_effect(WindowBackgroundEffect::Glass),
            Some(DWMSBT_MAINWINDOW)
        );
        assert_eq!(
            backdrop_for_effect(WindowBackgroundEffect::Acrylic),
            Some(DWMSBT_TRANSIENTWINDOW)
        );
        assert_eq!(backdrop_for_effect(WindowBackgroundEffect::None), None);
    }
}

#[allow(dead_code)]
pub(crate) fn clear_dwm_backdrop(hwnd: isize) -> WebView2Result<()> {
    if hwnd == 0 {
        return Ok(());
    }
    let backdrop = DWMSBT_NONE;
    let _ = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        )
    };
    Ok(())
}
