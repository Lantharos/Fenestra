use std::time::Instant;

use crate::render::{DisplayList, RectCommand, RoundedRectCommand, TextCommand};
use crate::window::style::Color;

use super::native::OsrNativeHost;
use super::types::{LOADING_ANIMATION_INTERVAL, LoadingKind};

impl OsrNativeHost {
    pub(super) fn draw_loading(&self, list: &mut DisplayList, width: f32, height: f32) {
        let Some(loading) = self.loading else {
            return;
        };
        let content_y = self.titlebar_height();
        let content_height = (height - content_y).max(1.0);
        list.push(RectCommand {
            x: 0.0,
            y: content_y,
            width,
            height: content_height,
            color: self.config.background_color,
        });
        let center_y = content_y + content_height * 0.5;
        list.push(TextCommand {
            text: match loading.kind {
                LoadingKind::Opening => "Opening…",
                LoadingKind::Resuming => "Just a moment…",
            }
            .to_string(),
            x: 24.0,
            y: center_y - 30.0,
            width: (width - 48.0).max(1.0),
            height: 24.0,
            size: 14.0,
            line_height: 20.0,
            color: Color::TEXT.opacity(0.78),
        });
        let track_width = width.clamp(48.0, 112.0);
        let track_x = (width - track_width) * 0.5;
        let phase = (loading.started.elapsed().as_millis() / 100) as usize % 9;
        for index in 0..3 {
            let distance = phase.abs_diff(index * 3).min(9 - phase.abs_diff(index * 3));
            let opacity = if distance <= 1 { 0.82 } else { 0.20 };
            list.push(RoundedRectCommand {
                x: track_x + index as f32 * (track_width + 8.0) / 3.0,
                y: center_y + 4.0,
                width: (track_width - 16.0) / 3.0,
                height: 3.0,
                radius: 2.0,
                color: Color::TEXT.opacity(opacity),
            });
        }
    }

    pub(super) fn drive_loading(&mut self) -> Option<Instant> {
        let mut loading = self.loading?;
        let now = Instant::now();
        if now < loading.reveal_at {
            return Some(loading.reveal_at);
        }
        if now >= loading.next_frame {
            if !self.presented {
                if self.render() {
                    self.present_rendered_surface("native_loading");
                }
            } else if let Some(window) = &self.window {
                window.request_redraw();
            }
            loading.next_frame = now + LOADING_ANIMATION_INTERVAL;
            self.loading = Some(loading);
        }
        Some(loading.next_frame)
    }
}
