use crate::render::{DisplayCommand, DisplayList};
use crate::window::style::Color;
use glyphon::{Attrs, Buffer, Family, Metrics, Shaping, TextArea, TextBounds, Wrap};

use super::{GpuRenderer, TextBufferEntry};

impl GpuRenderer {
    pub(super) fn rebuild_text_buffers(&mut self, display_list: &DisplayList) {
        self.text_buffers.clear();
        for command in &display_list.commands {
            let DisplayCommand::Text(command) = command else {
                continue;
            };

            let scale = self.scale_factor;
            let mut buffer = Buffer::new(
                &mut self.font_system,
                Metrics::new(command.size * scale, command.line_height * scale),
            );
            buffer.set_size(Some(command.width * scale), Some(command.height * scale));
            buffer.set_wrap(Wrap::WordOrGlyph);
            buffer.set_text(
                &command.text,
                &Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
                Some(glyphon::cosmic_text::Align::Center),
            );
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.text_buffers.push(TextBufferEntry {
                buffer,
                command: command.clone(),
            });
        }
    }
}

pub(super) fn text_areas(text_buffers: &[TextBufferEntry], scale: f32) -> Vec<TextArea<'_>> {
    text_buffers
        .iter()
        .map(|entry| TextArea {
            buffer: &entry.buffer,
            left: entry.command.x * scale,
            top: entry.command.y * scale,
            scale: 1.0,
            bounds: TextBounds {
                left: (entry.command.x * scale) as i32,
                top: (entry.command.y * scale) as i32,
                right: ((entry.command.x + entry.command.width) * scale) as i32,
                bottom: ((entry.command.y + entry.command.height) * scale) as i32,
            },
            default_color: to_glyphon_color(entry.command.color),
            custom_glyphs: &[],
        })
        .collect()
}

fn to_glyphon_color(color: Color) -> glyphon::Color {
    glyphon::Color::rgba(
        to_u8(color.r),
        to_u8(color.g),
        to_u8(color.b),
        to_u8(color.a),
    )
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
