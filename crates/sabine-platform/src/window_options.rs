use crate::regions::WindowRegions;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlatformOs {
    Linux,
    Windows,
    Macos,
    Android,
    Ios,
    Web,
    #[default]
    Unknown,
}

pub fn current_desktop_os() -> PlatformOs {
    if cfg!(target_os = "linux") {
        PlatformOs::Linux
    } else if cfg!(target_os = "windows") {
        PlatformOs::Windows
    } else if cfg!(target_os = "macos") {
        PlatformOs::Macos
    } else {
        PlatformOs::Unknown
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowChrome {
    #[default]
    System,
    Sabine,
    Frameless,
    None,
}

impl WindowChrome {
    pub fn uses_native_decorations(self) -> bool {
        matches!(self, Self::System)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowBackgroundEffect {
    #[default]
    None,
    Blur,
    Glass,
    Acrylic,
    Mica,
    MicaAlt,
    Vibrancy,
    HudWindow,
    Sidebar,
    UnderWindowBackground,
}

impl WindowBackgroundEffect {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "blur" => Some(Self::Blur),
            "glass" => Some(Self::Glass),
            "acrylic" => Some(Self::Acrylic),
            "mica" => Some(Self::Mica),
            "mica-alt" => Some(Self::MicaAlt),
            "vibrancy" => Some(Self::Vibrancy),
            "hud-window" => Some(Self::HudWindow),
            "sidebar" => Some(Self::Sidebar),
            "under-window-background" => Some(Self::UnderWindowBackground),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blur => "blur",
            Self::Glass => "glass",
            Self::Acrylic => "acrylic",
            Self::Mica => "mica",
            Self::MicaAlt => "mica-alt",
            Self::Vibrancy => "vibrancy",
            Self::HudWindow => "hud-window",
            Self::Sidebar => "sidebar",
            Self::UnderWindowBackground => "under-window-background",
        }
    }

    pub fn requires_transparency(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub chrome: WindowChrome,
    pub resizable: bool,
    pub visible: bool,
    pub active: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub background_effect: WindowBackgroundEffect,
    pub regions: WindowRegions,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Sabine".to_string(),
            width: 760,
            height: 520,
            min_width: 420,
            min_height: 280,
            chrome: WindowChrome::System,
            resizable: true,
            visible: true,
            active: true,
            always_on_top: false,
            transparent: false,
            background_effect: WindowBackgroundEffect::None,
            regions: WindowRegions::default(),
        }
    }
}
