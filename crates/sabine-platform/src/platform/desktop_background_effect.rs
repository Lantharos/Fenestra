use std::sync::Arc;

use winit::window::Window;

use crate::{WindowBackgroundEffect, WindowEffect, WindowOptions};

#[cfg(target_os = "macos")]
#[path = "macos_background_effect.rs"]
mod macos;
#[cfg(target_os = "windows")]
#[path = "windows_background_effect.rs"]
mod windows;

pub fn request(window: &Arc<dyn Window>, options: &WindowOptions) -> Option<WindowEffect> {
    if !options.transparent || options.background_effect == WindowBackgroundEffect::None {
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        windows::apply(window, options.background_effect).then_some(WindowEffect)
    }
    #[cfg(target_os = "macos")]
    {
        macos::apply(window, options.background_effect).then_some(WindowEffect)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = window;
        let _ = options;
        None
    }
}
