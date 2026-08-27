use glyphon::cosmic_text::Align;
use glyphon::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};

#[derive(Debug)]
pub(crate) struct RasterText {
    font_system: FontSystem,
    cache: SwashCache,
    buffer: Buffer,
}

impl RasterText {
    pub(crate) fn new() -> Self {
        let mut font_system = FontSystem::new();
        let buffer = Buffer::new(&mut font_system, Metrics::new(14.0, 20.0));
        Self {
            font_system,
            cache: SwashCache::new(),
            buffer,
        }
    }

    pub(crate) fn draw_centered(
        &mut self,
        pixels: &mut [u8],
        surface: (u32, u32),
        bounds: (i32, i32, u32, u32),
        text: &str,
        size: f32,
        color: [u8; 4],
    ) {
        let (left, top, width, height) = bounds;
        self.buffer.set_metrics_and_size(
            Metrics::new(size, size + 6.0),
            Some(width as f32),
            Some(height as f32),
        );
        self.buffer.set_wrap(Wrap::None);
        self.buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            Some(Align::Center),
        );
        self.buffer.draw(
            &mut self.font_system,
            &mut self.cache,
            Color::rgba(color[0], color[1], color[2], color[3]),
            |x, y, glyph_width, glyph_height, glyph_color| {
                blend_rect(
                    pixels,
                    surface,
                    (left + x, top + y, glyph_width, glyph_height),
                    glyph_color.as_rgba(),
                );
            },
        );
    }
}

pub(crate) fn fill_bgra(pixels: &mut [u8], color: [u8; 4]) {
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[color[2], color[1], color[0], color[3]]);
    }
}

pub(crate) fn blend_rect(
    pixels: &mut [u8],
    surface: (u32, u32),
    rect: (i32, i32, u32, u32),
    color: [u8; 4],
) {
    let (width, height) = surface;
    let (x, y, rect_width, rect_height) = rect;
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = x.saturating_add_unsigned(rect_width).max(0) as u32;
    let y1 = y.saturating_add_unsigned(rect_height).max(0) as u32;
    let source_alpha = u32::from(color[3]);
    for py in y0..y1.min(height) {
        for px in x0..x1.min(width) {
            let offset = ((py * width + px) * 4) as usize;
            let pixel = &mut pixels[offset..offset + 4];
            for (channel, source) in [color[2], color[1], color[0]].into_iter().enumerate() {
                let destination = u32::from(pixel[channel]);
                pixel[channel] = ((u32::from(source) * source_alpha
                    + destination * (255 - source_alpha))
                    / 255) as u8;
            }
            pixel[3] = 255;
        }
    }
}
