use std::time::Instant;

use layershellev::{RefreshRequest, WindowState};

use crate::osr::host::types::{LOADING_ANIMATION_INTERVAL, LoadingKind, NativeLoading};
use crate::render::raster_text::{blend_rect, fill_bgra};

use super::buffer::DamageRect;
use super::types::OsrLayerHost;

impl OsrLayerHost {
    pub(super) fn begin_loading(&mut self, kind: LoadingKind, state: &mut WindowState<()>) {
        self.loading = Some(NativeLoading::new(kind));
        self.schedule_loading(state);
    }

    pub(super) fn finish_loading(&mut self, state: &mut WindowState<()>) {
        self.loading = None;
        self.presentation_buffer.clear();
        self.presentation_full_damage = true;
        if self.visible && self.surface_lifecycle.presentation_ready() {
            let main_id = state.main_window().id();
            state.request_refresh(main_id, RefreshRequest::NextFrame);
        }
    }

    pub(super) fn refresh_loading(&mut self, state: &mut WindowState<()>) -> bool {
        if !self.visible || !self.surface_lifecycle.presentation_ready() {
            return false;
        }
        let Some(mut loading) = self.loading else {
            return false;
        };
        let now = Instant::now();
        if now < loading.reveal_at {
            let main_id = state.main_window().id();
            state.request_refresh(main_id, RefreshRequest::At(loading.reveal_at));
            return true;
        }
        let (width, height) = self.buffer_size;
        self.presentation_buffer
            .resize((width * height * 4) as usize, 0);
        fill_bgra(
            &mut self.presentation_buffer,
            self.config.background_color.to_rgba8(),
        );
        let center_y = height as i32 / 2;
        let track_width = width.saturating_sub(48).clamp(1, 112);
        let track_x = (width - track_width) as i32 / 2;
        let phase = (loading.started.elapsed().as_millis() / 100) as usize % 9;
        for index in 0..3 {
            let marker = index * 3;
            let distance = phase.abs_diff(marker).min(9 - phase.abs_diff(marker));
            let alpha = if distance <= 1 { 210 } else { 52 };
            let segment_width = track_width.saturating_sub(16) / 3;
            let x = track_x + index as i32 * ((track_width + 8) / 3) as i32;
            blend_rect(
                &mut self.presentation_buffer,
                (width, height),
                (x, center_y - 1, segment_width, 3),
                [245, 245, 246, alpha],
            );
        }
        let damage = DamageRect::full(width, height);
        self.commit_surface(state, damage);
        loading.next_frame = now + LOADING_ANIMATION_INTERVAL;
        self.loading = Some(loading);
        let main_id = state.main_window().id();
        state.request_refresh(main_id, RefreshRequest::At(loading.next_frame));
        true
    }

    fn schedule_loading(&self, state: &mut WindowState<()>) {
        if self.visible
            && self.surface_lifecycle.presentation_ready()
            && let Some(loading) = self.loading
        {
            let main_id = state.main_window().id();
            state.request_refresh(main_id, RefreshRequest::At(loading.reveal_at));
        }
    }
}
