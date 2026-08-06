use std::sync::Arc;

use winit::window::Window;

use crate::{WindowBackgroundEffect, WindowEffect, WindowOptions};

pub fn request(window: &Arc<dyn Window>, options: &WindowOptions) -> Option<WindowEffect> {
    if !options.transparent || options.background_effect == WindowBackgroundEffect::None {
        return None;
    }

    #[cfg(target_os = "windows")]
    {
        apply_windows(window, options.background_effect).then_some(WindowEffect)
    }
    #[cfg(target_os = "macos")]
    {
        apply_macos(window, options.background_effect).then_some(WindowEffect)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = window;
        let _ = options;
        None
    }
}

#[cfg(target_os = "windows")]
fn apply_windows(window: &Arc<dyn Window>, effect: WindowBackgroundEffect) -> bool {
    use window_vibrancy::{apply_acrylic, apply_blur, apply_mica, apply_tabbed};

    let result = match effect {
        WindowBackgroundEffect::Acrylic | WindowBackgroundEffect::Glass => {
            apply_acrylic(window, None)
        }
        WindowBackgroundEffect::Mica => apply_mica(window, None),
        WindowBackgroundEffect::MicaAlt => apply_tabbed(window, None),
        WindowBackgroundEffect::Blur => apply_blur(window, None),
        WindowBackgroundEffect::None => return false,
        _ => apply_acrylic(window, None),
    };
    if let Err(error) = &result {
        eprintln!("failed to apply Windows glass effect: {error}");
    }
    result.is_ok()
}

#[cfg(target_os = "macos")]
fn apply_macos(window: &Arc<dyn Window>, effect: WindowBackgroundEffect) -> bool {
    use window_vibrancy::{NSVisualEffectMaterial, apply_vibrancy};

    let material = match effect {
        WindowBackgroundEffect::Vibrancy
        | WindowBackgroundEffect::Glass
        | WindowBackgroundEffect::Blur => NSVisualEffectMaterial::UnderWindowBackground,
        WindowBackgroundEffect::HudWindow => NSVisualEffectMaterial::HudWindow,
        WindowBackgroundEffect::Sidebar => NSVisualEffectMaterial::Sidebar,
        WindowBackgroundEffect::UnderWindowBackground => {
            NSVisualEffectMaterial::UnderWindowBackground
        }
        WindowBackgroundEffect::None => return false,
        _ => NSVisualEffectMaterial::UnderWindowBackground,
    };
    let result = apply_vibrancy(window, material, None, None);
    if let Err(error) = &result {
        eprintln!("failed to apply macOS vibrancy: {error}");
    }
    result.is_ok()
}
