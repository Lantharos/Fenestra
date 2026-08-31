use std::time::{Duration, Instant};

use layershellev::{RefreshRequest, WindowState};

use crate::render::raster_text::blend_rect;

use super::types::{LayerTooltip, OsrLayerHost};

const REVEAL_DELAY: Duration = Duration::from_millis(500);

impl OsrLayerHost {
    pub(super) fn update_tooltip(&mut self, text: String, state: &mut WindowState<()>) {
        if !self.visible || !self.surface_lifecycle.presentation_ready() {
            self.tooltip = None;
            return;
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            if self.tooltip.take().is_some_and(|tooltip| tooltip.shown) {
                self.presentation_buffer.clear();
                self.presentation_full_damage = true;
                let main_id = state.main_window().id();
                state.request_refresh(main_id, RefreshRequest::NextFrame);
            }
            return;
        }
        let reveal_at = Instant::now() + REVEAL_DELAY;
        self.tooltip = Some(LayerTooltip {
            text,
            x: self.cursor_x,
            y: self.cursor_y,
            reveal_at,
            shown: false,
        });
        let main_id = state.main_window().id();
        state.request_refresh(main_id, RefreshRequest::At(reveal_at));
    }

    pub(super) fn drive_tooltip(&mut self, state: &mut WindowState<()>) {
        if !self.visible || !self.surface_lifecycle.presentation_ready() {
            return;
        }
        let Some(tooltip) = self.tooltip.as_mut() else {
            return;
        };
        if tooltip.shown {
            return;
        }
        let now = Instant::now();
        if now < tooltip.reveal_at {
            let main_id = state.main_window().id();
            state.request_refresh(main_id, RefreshRequest::At(tooltip.reveal_at));
            return;
        }
        tooltip.shown = true;
        self.presentation_full_damage = true;
        let main_id = state.main_window().id();
        state.request_refresh(main_id, RefreshRequest::NextFrame);
    }

    pub(super) fn prepare_tooltip_buffer(&mut self) {
        let Some(tooltip) = self.tooltip.as_ref().filter(|tooltip| tooltip.shown) else {
            self.presentation_buffer.clear();
            return;
        };
        let (width, height) = self.buffer_size;
        self.presentation_buffer.clone_from(&self.main_buffer);
        let tooltip_width = (tooltip.text.chars().count() as f32 * 7.4 + 20.0)
            .clamp(52.0, (width as f32 - 16.0).max(52.0)) as u32;
        let tooltip_height = 30_u32;
        let x =
            (tooltip.x + 14.0).clamp(8.0, (width.saturating_sub(tooltip_width + 8)) as f32) as i32;
        let y = (tooltip.y + 18.0).clamp(8.0, (height.saturating_sub(tooltip_height + 8)) as f32)
            as i32;
        blend_rect(
            &mut self.presentation_buffer,
            (width, height),
            (x, y, tooltip_width, tooltip_height),
            [34, 34, 38, 245],
        );
        self.text_renderer.draw_centered(
            &mut self.presentation_buffer,
            (width, height),
            (x + 10, y + 4, tooltip_width.saturating_sub(20), 22),
            &tooltip.text,
            12.0,
            [245, 245, 246, 255],
        );
    }
}
