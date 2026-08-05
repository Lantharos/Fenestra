use std::time::Duration;

use mullion_bridge::{BridgeRegistry, ContentSecurity};
use mullion_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    ShellSurfaceOptions, SingleInstancePolicy, TrayIcon, WindowBackgroundEffect, WindowRegionRect,
    WindowRegions,
};
use mullion_runtime::RuntimeConfig;

use crate::browser::BrowserOptions;

#[derive(Clone, Debug)]
pub struct MullionWindowConfig {
    pub entry: Option<String>,
    pub url: Option<String>,
    pub dev_url: Option<String>,
    pub dev_command: Option<String>,
    pub app_id: Option<String>,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub resizable: bool,
    pub visible: bool,
    pub shell_surface_alpha: f32,
    pub active: bool,
    pub hide_on_blur: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub frameless: bool,
    pub chrome: MullionWindowChrome,
    pub background_effect: WindowBackgroundEffect,
    pub low_power_background_effect: Option<WindowBackgroundEffect>,
    pub regions: WindowRegions,
    pub shell_surface: Option<ShellSurfaceOptions>,
    pub drag_regions: Vec<WindowRegionRect>,
    pub drag_exclusion_regions: Vec<WindowRegionRect>,
    pub control_regions: Vec<MullionWindowControlRegion>,
    pub desktop_services: DesktopServiceConfig,
    pub lifecycle: MullionLifecyclePolicy,
    pub runtime: RuntimeConfig,
    pub bridge: BridgeRegistry,
    pub security: ContentSecurity,
    pub browser: BrowserOptions,
}

impl Default for MullionWindowConfig {
    fn default() -> Self {
        Self {
            entry: None,
            url: None,
            dev_url: None,
            dev_command: None,
            app_id: None,
            title: "Mullion".to_string(),
            width: 900,
            height: 640,
            min_width: 420,
            min_height: 280,
            resizable: true,
            visible: true,
            shell_surface_alpha: 1.0,
            active: true,
            hide_on_blur: false,
            always_on_top: false,
            transparent: false,
            frameless: false,
            chrome: MullionWindowChrome::System,
            background_effect: WindowBackgroundEffect::None,
            low_power_background_effect: None,
            regions: WindowRegions::default(),
            shell_surface: None,
            drag_regions: Vec::new(),
            drag_exclusion_regions: Vec::new(),
            control_regions: Vec::new(),
            desktop_services: DesktopServiceConfig::default(),
            lifecycle: MullionLifecyclePolicy::default(),
            runtime: RuntimeConfig::default(),
            bridge: BridgeRegistry::default(),
            security: ContentSecurity::default(),
            browser: BrowserOptions::default(),
        }
    }
}

impl MullionWindowConfig {
    pub fn effective_background_effect(&self) -> WindowBackgroundEffect {
        if let Some(effect) = self.low_power_background_effect
            && low_power_glass_requested()
        {
            return effect;
        }
        self.background_effect
    }

    pub fn dev_mode(&self) -> bool {
        self.dev_url.is_some()
    }

    pub fn effective_remote_devtools_port(&self) -> Option<u16> {
        self.browser.effective_remote_devtools_port(self.dev_mode())
    }

    pub fn browser_options(&self) -> BrowserOptions {
        self.browser.clone()
    }
}

pub(crate) fn low_power_glass_requested() -> bool {
    env_flag("MULLION_LOW_POWER_GLASS")
        || env_flag("ASHER_LOW_POWER_MODE")
        || std::env::var("ASHER_POWER_PROFILE")
            .map(|value| matches!(value.as_str(), "battery" | "low-power" | "power-saver"))
            .unwrap_or(false)
}

pub(crate) fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MullionLifecyclePolicy {
    pub active_frame_rate: u32,
    pub background_frame_rate: u32,
    pub suspend_on_minimize: bool,
    pub suspend_on_occluded: bool,
    pub suspend_on_blur: bool,
    pub hibernate_after: Option<Duration>,
    pub hibernate_grace: Duration,
    pub retain_hidden_frame: bool,
}

impl Default for MullionLifecyclePolicy {
    fn default() -> Self {
        Self {
            active_frame_rate: 0,
            background_frame_rate: 5,
            suspend_on_minimize: true,
            suspend_on_occluded: true,
            suspend_on_blur: false,
            hibernate_after: None,
            hibernate_grace: Duration::from_millis(750),
            retain_hidden_frame: false,
        }
    }
}

impl MullionLifecyclePolicy {
    pub fn browser_tab() -> Self {
        Self {
            suspend_on_blur: true,
            hibernate_after: Some(Duration::from_secs(300)),
            ..Self::default()
        }
    }

    pub fn hidden_window() -> Self {
        Self {
            background_frame_rate: 1,
            suspend_on_blur: true,
            hibernate_grace: Duration::from_millis(150),
            retain_hidden_frame: true,
            ..Self::default()
        }
    }

    pub fn memory_saver_hidden_window() -> Self {
        Self {
            hibernate_after: Some(Duration::from_secs(5)),
            retain_hidden_frame: false,
            ..Self::hidden_window()
        }
    }

    pub fn with_hibernate_after(mut self, duration: Duration) -> Self {
        self.hibernate_after = Some(duration);
        self
    }

    pub fn without_hibernation(mut self) -> Self {
        self.hibernate_after = None;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct DesktopServiceConfig {
    pub tray_icon: Option<TrayIcon>,
    pub autostart: Vec<AutostartEntry>,
    pub global_shortcuts: Vec<GlobalShortcutRegistration>,
    pub deep_links: Vec<DeepLinkRegistration>,
    pub native_messaging_hosts: Vec<NativeMessagingHost>,
    pub single_instance_id: Option<String>,
    pub single_instance_policy: Option<SingleInstancePolicy>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MullionWindowChrome {
    #[default]
    System,
    Mullion,
    Frameless,
    None,
}

impl MullionWindowChrome {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "mullion" | "custom" => Some(Self::Mullion),
            "frameless" => Some(Self::Frameless),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Mullion => "mullion",
            Self::Frameless => "frameless",
            Self::None => "none",
        }
    }

    pub fn uses_native_decorations(self) -> bool {
        matches!(self, Self::System)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MullionWindowControlAction {
    Minimize,
    Maximize,
    Close,
}

impl MullionWindowControlAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "minimize" => Some(Self::Minimize),
            "maximize" => Some(Self::Maximize),
            "close" => Some(Self::Close),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MullionWindowControlRegion {
    pub action: MullionWindowControlAction,
    pub rect: WindowRegionRect,
}

impl MullionWindowControlRegion {
    pub fn new(action: MullionWindowControlAction, rect: WindowRegionRect) -> Self {
        Self { action, rect }
    }
}
