use std::sync::Arc;

use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use objc2_foundation::MainThreadMarker;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use crate::WindowBackgroundEffect;

pub(super) fn apply(window: &Arc<dyn Window>, effect: WindowBackgroundEffect) -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return false;
    };
    let material = match effect {
        WindowBackgroundEffect::HudWindow => NSVisualEffectMaterial::HUDWindow,
        WindowBackgroundEffect::Sidebar => NSVisualEffectMaterial::Sidebar,
        WindowBackgroundEffect::None => return false,
        _ => NSVisualEffectMaterial::UnderWindowBackground,
    };

    unsafe {
        let content_view = handle.ns_view.cast::<NSView>().as_ref();
        let effect_view =
            NSVisualEffectView::initWithFrame(main_thread.alloc(), content_view.bounds());
        effect_view.setMaterial(material);
        effect_view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect_view.setState(NSVisualEffectState::FollowsWindowActiveState);
        effect_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content_view.addSubview_positioned_relativeTo(
            &effect_view,
            NSWindowOrderingMode::Below,
            None,
        );
    }
    true
}
