use std::time::Instant;

use crate::render::{DisplayList, RoundedRectCommand, TextCommand};
use crate::window::style::Color;

use super::native::OsrNativeHost;
use super::types::NativeTooltip;

impl OsrNativeHost {
    pub(super) fn update_tooltip(&mut self, text: String) -> bool {
        let text = text.trim().to_string();
        if text.is_empty() {
            return self.tooltip.take().is_some_and(|tooltip| tooltip.shown);
        }
        let needs_redraw = self.tooltip.as_ref().is_some_and(|tooltip| tooltip.shown);
        self.tooltip = Some(NativeTooltip::new(text, self.cursor_x, self.cursor_y));
        needs_redraw
    }

    pub(super) fn drive_tooltip(&mut self) -> Option<Instant> {
        let tooltip = self.tooltip.as_mut()?;
        if tooltip.shown {
            return None;
        }
        let now = Instant::now();
        if now < tooltip.reveal_at {
            return Some(tooltip.reveal_at);
        }
        tooltip.shown = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        None
    }

    pub(super) fn draw_tooltip(&self, list: &mut DisplayList, width: f32, height: f32) {
        let Some(tooltip) = self.tooltip.as_ref().filter(|tooltip| tooltip.shown) else {
            return;
        };
        let tooltip_width = (tooltip.text.chars().count() as f32 * 7.4 + 20.0)
            .clamp(52.0, (width - 16.0).max(52.0));
        let tooltip_height = 30.0;
        let x = (tooltip.x + 14.0).clamp(8.0, (width - tooltip_width - 8.0).max(8.0));
        let y = (tooltip.y + 18.0).clamp(
            self.titlebar_height() + 8.0,
            (height - tooltip_height - 8.0).max(self.titlebar_height() + 8.0),
        );
        list.push(RoundedRectCommand {
            x,
            y,
            width: tooltip_width,
            height: tooltip_height,
            radius: 7.0,
            color: Color::rgb8(34, 34, 38).opacity(0.96),
        });
        list.push(TextCommand {
            text: tooltip.text.clone(),
            x: x + 10.0,
            y: y + 5.0,
            width: tooltip_width - 20.0,
            height: 20.0,
            size: 12.0,
            line_height: 18.0,
            color: Color::rgb8(245, 245, 246),
        });
    }
}
