/// An sRGB color used by Sabine's native window surfaces.
///
/// Prefer [`Self::rgb8`] or [`Self::rgba8`] when matching a CSS color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub(crate) r: f32,
    pub(crate) g: f32,
    pub(crate) b: f32,
    pub(crate) a: f32,
}

impl Color {
    pub(crate) const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    pub(crate) const TEXT: Self = Self::rgb(0.95, 0.95, 0.95);
    pub(crate) const WINDOW: Self = Self::rgb8(17, 17, 19);

    /// Creates an opaque color from the same 8-bit channels used by CSS.
    pub const fn rgb8(r: u8, g: u8, b: u8) -> Self {
        Self::rgba8(r, g, b, 255)
    }

    /// Creates a color from 8-bit sRGB and alpha channels.
    pub const fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::rgba(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        )
    }

    pub(crate) const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub(crate) const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Multiplies this color's alpha without changing its RGB channels.
    pub fn opacity(self, alpha: f32) -> Self {
        Self {
            a: self.a * alpha.clamp(0.0, 1.0),
            ..self
        }
    }

    pub(crate) fn to_rgba8(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
            .map(|component| (component.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    pub(crate) fn to_opaque_argb_hex(self) -> String {
        let [r, g, b, _] = self.to_rgba8();
        format!("0xFF{r:02X}{g:02X}{b:02X}")
    }

    pub(crate) fn linear_rgb(self) -> [f32; 3] {
        [self.r, self.g, self.b].map(srgb_to_linear)
    }
}

fn srgb_to_linear(component: f32) -> f32 {
    let component = component.clamp(0.0, 1.0);
    if component <= 0.04045 {
        component / 12.92
    } else {
        ((component + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::Color;

    #[test]
    fn rgb8_round_trips_and_converts_to_linear_gpu_values() {
        let color = Color::rgb8(17, 34, 51);
        assert_eq!(color.to_rgba8(), [17, 34, 51, 255]);
        assert_eq!(color.to_opaque_argb_hex(), "0xFF112233");
        let linear = color.linear_rgb();
        assert!(linear[0] < color.r);
        assert!(linear[1] < color.g);
        assert!(linear[2] < color.b);
    }
}
