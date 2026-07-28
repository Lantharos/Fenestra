// Win32 host windows and per-partition WebView2 environments for guest
// webviews.
//
// Every guest lives in its own `WS_CHILD` HWND parented to the winit
// window. The child HWND owns one `ICoreWebView2Controller`, which keeps
// bounds, clipping, visibility, and z-order independent from both the
// primary webview and the other guests. Positioning a guest is a plain
// `SetWindowPos` on the child; the controller always fills its client
// area.
//
// Storage isolation uses one `ICoreWebView2Environment` per partition
// key, each backed by its own user data folder. WebView2 only allows
// environments in the same process to share a user data folder when
// their options match, so distinct folders are what actually separates
// cookies and cache between partitions.

#![cfg(target_os = "windows")]

use std::{
    path::Path,
    sync::{OnceLock, mpsc::Receiver},
    time::{Duration, Instant},
};

use fenestra_bridge::guest::GuestBounds;
use webview2_com::{
    CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller, ICoreWebView2Environment, ICoreWebView2EnvironmentOptions,
    },
};
use webview2_com_sys::Microsoft::Web::WebView2::Win32 as SysWin32;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GetClientRect, HWND_BOTTOM, HWND_TOP, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage,
            RegisterClassExW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE,
            SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow, TranslateMessage,
            WINDOW_EX_STYLE, WM_QUIT, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
            WS_VISIBLE,
        },
    },
};

use crate::{WebView2Error, WebView2Result, windows::bridge};

const GUEST_WINDOW_CLASS: &str = "FenestraGuestHostWindow";
const PRIMARY_WINDOW_CLASS: &str = "FenestraPrimaryHostWindow";

/// Upper bound on the nested message pump used while a guest
/// environment or controller is created. Guest creation is driven from a
/// WebView2 event callback, so the pump has to be bounded — a hang here
/// would freeze the whole window.
const CREATE_TIMEOUT: Duration = Duration::from_secs(20);

/// Exposed for composition-controller creation (same UI-thread pump).
pub(crate) const CONTROLLER_CREATE_TIMEOUT: Duration = CREATE_TIMEOUT;

/// Upper bound for `ExecuteScript` on a guest. Guests render untrusted
/// content, so a busy renderer must not be able to stall the host.
pub(crate) const SCRIPT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn create_host_window(
    parent: isize,
    bounds: GuestBounds,
    visible: bool,
) -> WebView2Result<isize> {
    create_child_host_window(GUEST_WINDOW_CLASS, parent, bounds, visible)
}

/// Full-client child that owns the primary WebView2 controller.
///
/// Guests are siblings of this window. Keeping the primary controller off
/// the top-level HWND is what lets `SetWindowPos` raise a guest above the
/// app UI instead of leaving it buried under WebView2's chrome HWND.
pub(crate) fn create_primary_host_window(parent: isize) -> WebView2Result<isize> {
    let bounds = client_bounds(parent);
    let hwnd = create_child_host_window(PRIMARY_WINDOW_CLASS, parent, bounds, true)?;
    lower_host_window(hwnd);
    Ok(hwnd)
}

pub(crate) fn resize_primary_host_window(hwnd: isize, parent: isize) {
    if hwnd == 0 || parent == 0 {
        return;
    }
    let bounds = client_bounds(parent);
    move_host_window(hwnd, bounds);
    lower_host_window(hwnd);
}

fn create_child_host_window(
    class_name: &'static str,
    parent: isize,
    bounds: GuestBounds,
    visible: bool,
) -> WebView2Result<isize> {
    if parent == 0 {
        return Err(WebView2Error::Backend(
            "host window needs a parent HWND".to_string(),
        ));
    }
    let class = register_named_host_window_class(class_name)?;
    let class_wide = bridge::wide_pwstr(class);
    let mut style = WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
    if visible {
        style |= WS_VISIBLE;
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::PCWSTR(class_wide.as_ptr()),
            windows::core::PCWSTR::null(),
            style,
            bounds.x,
            bounds.y,
            bounds.width.max(1) as i32,
            bounds.height.max(1) as i32,
            Some(HWND(parent as *mut _)),
            None,
            None,
            None,
        )
    }
    .map_err(|error| WebView2Error::Backend(format!("{class_name} CreateWindowExW: {error}")))?;
    Ok(hwnd.0 as isize)
}

fn client_bounds(parent: isize) -> GuestBounds {
    let mut rect = RECT::default();
    let ok = unsafe { GetClientRect(HWND(parent as *mut _), &mut rect) }.is_ok();
    if !ok {
        return GuestBounds {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
    }
    GuestBounds {
        x: 0,
        y: 0,
        width: (rect.right - rect.left).max(1) as u32,
        height: (rect.bottom - rect.top).max(1) as u32,
    }
}

pub(crate) fn destroy_host_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let _ = unsafe { DestroyWindow(HWND(hwnd as *mut _)) };
}

/// Convert CSS-pixel / DIP guest bounds into physical pixels for Win32 /
/// WebView2. `dpi_hwnd` should be the top-level window (or any HWND in the
/// same DPI context).
pub(crate) fn physical_bounds(dpi_hwnd: isize, bounds: GuestBounds) -> GuestBounds {
    let scale = dpi_scale(dpi_hwnd);
    GuestBounds {
        x: ((bounds.x as f64) * scale).round() as i32,
        y: ((bounds.y as f64) * scale).round() as i32,
        width: ((bounds.width as f64) * scale).round().max(1.0) as u32,
        height: ((bounds.height as f64) * scale).round().max(1.0) as u32,
    }
}

fn dpi_scale(hwnd: isize) -> f64 {
    if hwnd == 0 {
        return 1.0;
    }
    let dpi = unsafe { GetDpiForWindow(HWND(hwnd as *mut _)) };
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / 96.0
    }
}

/// Move a guest host window and keep it above the primary webview.
pub(crate) fn move_host_window(hwnd: isize, bounds: GuestBounds) {
    if hwnd == 0 {
        return;
    }
    let _ = unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_TOP),
            bounds.x,
            bounds.y,
            bounds.width.max(1) as i32,
            bounds.height.max(1) as i32,
            SWP_NOACTIVATE | SWP_NOCOPYBITS | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
        )
    };
}

pub(crate) fn set_host_window_visible(hwnd: isize, visible: bool) {
    if hwnd == 0 {
        return;
    }
    let command = if visible { SW_SHOWNOACTIVATE } else { SW_HIDE };
    let _ = unsafe { ShowWindow(HWND(hwnd as *mut _), command) };
    if visible {
        raise_host_window(hwnd);
    }
}

/// Keep the primary host under every guest sibling.
pub(crate) fn lower_host_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let _ = unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER,
        )
    };
}

/// Force a guest host above sibling WebView2 chrome widgets.
pub(crate) fn raise_host_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    let _ = unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
        )
    };
}

/// Put `guest` above the primary host (and every other sibling).
pub(crate) fn raise_guest_above_primary(guest: isize, primary: isize) {
    lower_host_window(primary);
    raise_host_window(guest);
}

pub(crate) fn create_environment(
    user_data_dir: &Path,
) -> WebView2Result<ICoreWebView2Environment> {
    std::fs::create_dir_all(user_data_dir).map_err(WebView2Error::Io)?;
    let user_data_dir = user_data_dir
        .to_str()
        .ok_or_else(|| WebView2Error::Backend("guest user data dir is not UTF-8".to_string()))?;
    let user_data_wide = bridge::wide_pwstr(user_data_dir);
    let options: ICoreWebView2EnvironmentOptions = CoreWebView2EnvironmentOptions::default().into();
    let (tx, rx) = std::sync::mpsc::channel();
    let handler =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |status, environment| {
            tx.send(environment_result(status, environment))
                .map_err(|_| send_failed())
        }));
    unsafe {
        SysWin32::CreateCoreWebView2EnvironmentWithOptions(
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR(user_data_wide.as_ptr()),
            &options,
            &handler,
        )
        .map_err(|error| {
            WebView2Error::Backend(format!("guest CreateCoreWebView2Environment: {error}"))
        })?;
    }
    wait_bounded(rx, "guest environment", CREATE_TIMEOUT)?
        .map_err(|error| WebView2Error::Backend(format!("guest environment callback: {error}")))
}

pub(crate) fn create_controller(
    environment: &ICoreWebView2Environment,
    hwnd: isize,
) -> WebView2Result<ICoreWebView2Controller> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handler =
        CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |status, controller| {
            tx.send(controller_result(status, controller))
                .map_err(|_| send_failed())
        }));
    unsafe {
        environment
            .CreateCoreWebView2Controller(HWND(hwnd as *mut _), &handler)
            .map_err(|error| {
                WebView2Error::Backend(format!("guest CreateCoreWebView2Controller: {error}"))
            })?;
    }
    wait_bounded(rx, "guest controller", CREATE_TIMEOUT)?
        .map_err(|error| WebView2Error::Backend(format!("guest controller callback: {error}")))
}

/// Pump Win32 messages until `rx` produces a value or `timeout` expires.
///
/// `webview2_com::wait_with_pump` blocks in `GetMessage` with no upper
/// bound. Guest work runs inside WebView2 event callbacks, so an
/// unbounded wait there can deadlock the UI thread; this variant always
/// gives the caller control back.
pub(crate) fn wait_bounded<T>(
    rx: Receiver<T>,
    label: &str,
    timeout: Duration,
) -> WebView2Result<T> {
    let deadline = Instant::now() + timeout;
    let mut msg = MSG::default();
    loop {
        if let Ok(value) = rx.try_recv() {
            return Ok(value);
        }
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                unsafe { PostQuitMessage(msg.wParam.0 as i32) };
                return Err(WebView2Error::Backend(format!("{label} was cancelled")));
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if let Ok(value) = rx.try_recv() {
                return Ok(value);
            }
        }
        if Instant::now() >= deadline {
            return Err(WebView2Error::Backend(format!(
                "{label} timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn environment_result(
    status: windows::core::Result<()>,
    environment: Option<ICoreWebView2Environment>,
) -> windows::core::Result<ICoreWebView2Environment> {
    status?;
    environment.ok_or_else(no_interface)
}

fn controller_result(
    status: windows::core::Result<()>,
    controller: Option<ICoreWebView2Controller>,
) -> windows::core::Result<ICoreWebView2Controller> {
    status?;
    controller.ok_or_else(no_interface)
}

fn no_interface() -> windows::core::Error {
    windows::core::Error::from(windows::core::HRESULT(0x80004003u32 as i32))
}

fn send_failed() -> windows::core::Error {
    windows::core::Error::from(windows::core::HRESULT(0x80000004u32 as i32))
}

fn register_named_host_window_class(class_name: &'static str) -> WebView2Result<&'static str> {
    static GUEST_REGISTERED: OnceLock<bool> = OnceLock::new();
    static PRIMARY_REGISTERED: OnceLock<bool> = OnceLock::new();
    let registered = if class_name == PRIMARY_WINDOW_CLASS {
        &PRIMARY_REGISTERED
    } else {
        &GUEST_REGISTERED
    };
    let ok = registered.get_or_init(|| register_class(class_name));
    if *ok {
        Ok(class_name)
    } else {
        Err(WebView2Error::Backend(format!(
            "RegisterClassExW for {class_name} failed"
        )))
    }
}

fn register_class(class_name: &str) -> bool {
    let module = unsafe { GetModuleHandleW(windows::core::PCWSTR::null()) };
    let Ok(module) = module else {
        return false;
    };
    let class_wide = bridge::wide_pwstr(class_name);
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(guest_window_proc),
        hInstance: HINSTANCE::from(module),
        lpszClassName: windows::core::PCWSTR(class_wide.as_ptr()),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassExW(&class) };
    atom != 0
}

unsafe extern "system" fn guest_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
