use layershellev::{
    WindowState,
    blur::{BlurOption, BlurRegion},
};
use sabine_platform::WindowBackgroundEffect;

use super::types::OsrLayerHost;

impl OsrLayerHost {
    pub(super) fn update_main_effect(&mut self, state: &mut WindowState<()>) {
        if !self.visible || !self.surface_lifecycle.presentation_ready() {
            return;
        }
        let (width, height) = self.surface_size;
        let blur = blur_option(
            self.config.transparent,
            self.config.background_effect,
            self.config.regions.blur.as_ref(),
            width as i32,
            height as i32,
        );
        if self.blur_option.as_ref() == Some(&blur) {
            return;
        }
        let main_id = state.main_window().id();
        if let Some(unit) = state.get_mut_unit_with_id(main_id) {
            unit.set_blur_option(blur.clone());
            self.blur_option = Some(blur);
        }
    }
}

fn blur_option(
    transparent: bool,
    background_effect: WindowBackgroundEffect,
    region: Option<&sabine_platform::WindowRegion>,
    width: i32,
    height: i32,
) -> BlurOption {
    if !transparent || background_effect == WindowBackgroundEffect::None {
        return BlurOption::None;
    }
    let Some(region) = region else {
        return BlurOption::FullRegion;
    };
    BlurOption::Region(
        region
            .resolved_rects(width, height)
            .into_iter()
            .map(|rect| BlurRegion {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
            .collect(),
    )
}
