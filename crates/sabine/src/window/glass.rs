use sabine_platform::WindowBackgroundEffect;

/// Per-platform overrides for the default `.glass()` material.
///
/// `GlassSpec` lets an app ask for a different background effect on
/// each target OS without `cfg`-gating the builder. Pass it to
/// [`SabineWindow::glass_spec`] (or the `SabineWindow` alias of it on
/// the current host).
///
/// String values are parsed through
/// [`WindowBackgroundEffect::parse`](sabine_platform::WindowBackgroundEffect::parse),
/// so unknown names silently fall back to the platform default. The
/// effect names live in Sabine's platform primitives. Use `glass`
/// for the Linux compositor-provided glass material.
///
/// Default per platform (when the spec does not override the field):
///
/// | OS      | Effect      | Notes                                         |
/// | ------- | ----------- | --------------------------------------------- |
/// | Windows | `Acrylic`   | DWM acrylic; use `.windows("mica")` for Mica  |
/// | macOS   | `Vibrancy`  | NSVisualEffectView under-window background    |
/// | Linux   | `Blur`      | Wayland `ext_background_effect_v1` blur       |
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlassSpec {
    windows: Option<WindowBackgroundEffect>,
    macos: Option<WindowBackgroundEffect>,
    linux: Option<WindowBackgroundEffect>,
}

impl GlassSpec {
    /// Empty spec; resolving it falls back to the per-platform
    /// defaults listed in [`GlassSpec`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the Windows effect. Unknown names are ignored.
    pub fn windows(mut self, effect: &str) -> Self {
        self.windows = WindowBackgroundEffect::parse(effect);
        self
    }

    /// Override the macOS effect. Unknown names are ignored.
    pub fn macos(mut self, effect: &str) -> Self {
        self.macos = WindowBackgroundEffect::parse(effect);
        self
    }

    /// Override the Linux effect. Unknown names are ignored.
    pub fn linux(mut self, effect: &str) -> Self {
        self.linux = WindowBackgroundEffect::parse(effect);
        self
    }

    pub fn resolve(self) -> WindowBackgroundEffect {
        match sabine_platform::current_desktop_os() {
            sabine_platform::PlatformOs::Windows => {
                self.windows.unwrap_or(WindowBackgroundEffect::Acrylic)
            }
            sabine_platform::PlatformOs::Macos => {
                self.macos.unwrap_or(WindowBackgroundEffect::Vibrancy)
            }
            sabine_platform::PlatformOs::Linux => {
                self.linux.unwrap_or(WindowBackgroundEffect::Blur)
            }
            _ => WindowBackgroundEffect::None,
        }
    }
}
