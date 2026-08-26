use crate::render::{DisplayCommand, DisplayList};
use crate::window::style::Color;
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};

use super::RendererError;
use crate::render::TextCommand;

pub(super) struct TextRendererState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffers: Vec<TextBufferEntry>,
}

struct TextBufferEntry {
    buffer: Buffer,
    command: TextCommand,
    scale: f32,
}

impl TextRendererState {
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            viewport,
            atlas,
            renderer,
            buffers: Vec::new(),
        }
    }

    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        display_list: &DisplayList,
        scale: f32,
        width: u32,
        height: u32,
    ) -> Result<(), RendererError> {
        self.viewport.update(queue, Resolution { width, height });
        let commands = display_list.commands.iter().filter_map(|command| {
            let DisplayCommand::Text(command) = command else {
                return None;
            };
            Some(command.clone())
        });
        for (index, command) in commands.enumerate() {
            let unchanged = self.buffers.get(index).is_some_and(|entry| {
                entry.command == command && (entry.scale - scale).abs() < f32::EPSILON
            });
            if unchanged {
                continue;
            }
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
            let entry = TextBufferEntry {
                buffer,
                command,
                scale,
            };
            if index < self.buffers.len() {
                self.buffers[index] = entry;
            } else {
                self.buffers.push(entry);
            }
        }
        let text_count = display_list
            .commands
            .iter()
            .filter(|command| matches!(command, DisplayCommand::Text(_)))
            .count();
        self.buffers.truncate(text_count);
        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas(&self.buffers, scale),
                &mut self.swash_cache,
            )
            .map_err(|error| RendererError::Text(error.to_string()))
    }

    pub(super) fn render<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
    ) -> Result<(), RendererError> {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .map_err(|error| RendererError::Text(error.to_string()))
    }

    pub(super) fn trim(&mut self) {
        self.atlas.trim();
    }
}

fn text_areas(text_buffers: &[TextBufferEntry], scale: f32) -> Vec<TextArea<'_>> {
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
