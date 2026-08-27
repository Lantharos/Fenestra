use layershellev::WindowState;
use sabine_platform::WindowOptions;

use super::types::OsrLayerHost;

impl OsrLayerHost {
    pub(super) fn update_main_effect(&mut self, state: &WindowState<()>) {
        let options = self.main_effect_options();
        let (width, height) = self.surface_size;
        if let Some(effect) = &self.effect {
            let _ = effect.update(&options, width as i32, height as i32);
            return;
        }
        self.effect = sabine_platform::request_surface_effect(
            state.main_window(),
            &options,
            width as i32,
            height as i32,
        );
    }

    fn main_effect_options(&self) -> WindowOptions {
        WindowOptions {
            title: self.config.title.clone(),
            width: self.surface_size.0,
            height: self.surface_size.1,
            transparent: self.config.transparent,
            background_effect: self.config.background_effect,
            regions: self.config.regions.clone(),
            ..WindowOptions::default()
        }
    }
}
