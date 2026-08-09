use std::sync::{Arc, OnceLock};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{
        DWM_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW, DWMSBT_TABBEDWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    },
    System::LibraryLoader::{GetProcAddress, LoadLibraryA},
};
use windows::core::s;
use winit::window::{Theme, Window};

use crate::WindowBackgroundEffect;

const COMPOSITION_ACCENT_POLICY: u32 = 0x13;
const ACCENT_BLUR: u32 = 3;
const ACCENT_ACRYLIC: u32 = 4;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    color: u32,
    animation_id: u32,
}

#[repr(C)]
struct CompositionAttributeData {
    attribute: u32,
    value: *mut std::ffi::c_void,
    size: usize,
}

type SetWindowCompositionAttribute =
    unsafe extern "system" fn(HWND, *mut CompositionAttributeData) -> i32;

pub(super) fn apply(window: &Arc<dyn Window>, effect: WindowBackgroundEffect) -> bool {
    let Some(hwnd) = hwnd(window) else {
        return false;
    };
    let dark = window.theme() == Some(Theme::Dark);
    set_dark_mode(hwnd, dark);
    let applied = match effect {
        WindowBackgroundEffect::None => false,
        WindowBackgroundEffect::Blur => set_accent(hwnd, ACCENT_BLUR, 0, dark),
        WindowBackgroundEffect::Mica => set_backdrop(hwnd, DWMSBT_MAINWINDOW),
        WindowBackgroundEffect::MicaAlt => set_backdrop(hwnd, DWMSBT_TABBEDWINDOW),
        _ => set_accent(hwnd, ACCENT_ACRYLIC, 96, dark),
    };
    if std::env::var_os("SABINE_TRACE").is_some() {
        eprintln!(
            "Sabine window effect: effect={effect:?} theme={:?} applied={applied}",
            window.theme()
        );
    }
    applied
}

fn set_dark_mode(hwnd: HWND, dark: bool) {
    let value = u32::from(dark);
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&value as *const u32).cast(),
            std::mem::size_of_val(&value) as u32,
        );
    }
}

fn set_backdrop(hwnd: HWND, backdrop: DWM_SYSTEMBACKDROP_TYPE) -> bool {
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&backdrop as *const DWM_SYSTEMBACKDROP_TYPE).cast(),
            std::mem::size_of_val(&backdrop) as u32,
        )
    }
    .is_ok()
}

fn set_accent(hwnd: HWND, state: u32, opacity: u8, dark: bool) -> bool {
    let Some(set_window_composition_attribute) = set_window_composition_attribute() else {
        return false;
    };
    let channel = if dark { 18_u32 } else { 243_u32 };
    let mut policy = AccentPolicy {
        state,
        flags: u32::from(state == ACCENT_BLUR) * 2,
        color: channel | (channel << 8) | (channel << 16) | (u32::from(opacity) << 24),
        animation_id: 0,
    };
    let mut data = CompositionAttributeData {
        attribute: COMPOSITION_ACCENT_POLICY,
        value: (&mut policy as *mut AccentPolicy).cast(),
        size: std::mem::size_of::<AccentPolicy>(),
    };
    (unsafe { set_window_composition_attribute(hwnd, &mut data) }) != 0
}

fn set_window_composition_attribute() -> Option<SetWindowCompositionAttribute> {
    static FUNCTION: OnceLock<Option<SetWindowCompositionAttribute>> = OnceLock::new();
    *FUNCTION.get_or_init(|| unsafe {
        let user32 = LoadLibraryA(s!("user32.dll")).ok()?;
        let function = GetProcAddress(user32, s!("SetWindowCompositionAttribute"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            SetWindowCompositionAttribute,
        >(function))
    })
}

fn hwnd(window: &Arc<dyn Window>) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(HWND(handle.hwnd.get() as *mut std::ffi::c_void))
}
