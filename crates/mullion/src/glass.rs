use mullion_platform::WindowBackgroundEffect;

/// Per-platform overrides for the default `.glass()` material.
///
/// `GlassSpec` lets an app ask for a different background effect on
/// each target OS without `cfg`-gating the builder. Pass it to
/// [`MullionWindow::glass_spec`] (or the `MullionWindow` alias of it on
/// the current host).
///
/// String values are parsed through
/// [`WindowBackgroundEffect::parse`](mullion_platform::WindowBackgroundEffect::parse),
/// so unknown names silently fall back to the platform default. The
/// effect names live in Mullion's platform primitives. Use `glass`
/// for the Linux compositor-provided glass material.
///
/// Default per platform (when the spec does not override the field):
///
/// | OS      | Effect      | Notes                                         |
/// | ------- | ----------- | --------------------------------------------- |
/// | Windows | `Mica`      | DWM main-window Mica system backdrop          |
/// | macOS   | `Vibrancy`  | NSVisualEffectView, the most transparent blur |
/// | Linux   | `Blur`      | Wayland `ext_background_effect_v1` blur       |
/// | Asher   | (no default) | Asher is not implemented yet                 |
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
        match mullion_platform::current_desktop_os() {
            mullion_platform::PlatformOs::Windows => {
                self.windows.unwrap_or(WindowBackgroundEffect::Mica)
            }
            mullion_platform::PlatformOs::Macos => {
                self.macos.unwrap_or(WindowBackgroundEffect::Vibrancy)
            }
            mullion_platform::PlatformOs::Linux => {
                self.linux.unwrap_or(WindowBackgroundEffect::Blur)
            }
            _ => WindowBackgroundEffect::None,
        }
    }
}
