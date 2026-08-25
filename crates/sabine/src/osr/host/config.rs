use std::path::PathBuf;

use sabine_platform::ShellSurfaceOptions;
use sabine_platform::{WindowBackgroundEffect, WindowRegionRect, WindowRegions};

use crate::osr::protocol::{
    control_regions_from_json, lifecycle_from_json, rects_from_json, regions_from_json,
    shell_surface_from_json,
};
use crate::{SabineLifecyclePolicy, SabineWindowChrome, SabineWindowControlRegion};

#[derive(Clone, Debug)]
pub(crate) struct OsrHostConfig {
    pub runtime_dir: PathBuf,
    pub host_binary: PathBuf,
    pub url: String,
    pub app_id: Option<String>,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub min_width: u32,
    pub min_height: u32,
    pub resizable: bool,
    pub visible: bool,
    #[cfg(target_os = "linux")]
    pub shell_surface_alpha: f32,
    pub active: bool,
    pub hide_on_blur: bool,
    pub hide_on_close: bool,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub skip_taskbar: bool,
    pub always_on_top: bool,
    pub transparent: bool,
    pub shell_surface: Option<ShellSurfaceOptions>,
    pub background_effect: WindowBackgroundEffect,
    pub chrome: SabineWindowChrome,
    pub bridge_commands: Vec<String>,
    pub regions: WindowRegions,
    pub drag_regions: Vec<WindowRegionRect>,
    pub drag_exclusion_regions: Vec<WindowRegionRect>,
    pub control_regions: Vec<SabineWindowControlRegion>,
    pub lifecycle: SabineLifecyclePolicy,
    pub dev_mode: bool,
    pub remote_devtools_port: Option<u16>,
    pub remote_devtools_disabled: bool,
    #[cfg(target_os = "linux")]
    pub vaapi_hardware_decode: bool,
}

impl OsrHostConfig {
    pub(crate) fn browser_options(&self) -> crate::launch::browser::BrowserOptions {
        crate::launch::browser::BrowserOptions {
            remote_devtools_port: self.remote_devtools_port,
            remote_devtools_disabled: self.remote_devtools_disabled,
            memory_saver: self.lifecycle.memory_saver,
            #[cfg(target_os = "linux")]
            vaapi_hardware_decode: self.vaapi_hardware_decode,
        }
    }

    pub(super) fn read(config_path: PathBuf) -> Result<Self, String> {
        let text = std::fs::read_to_string(&config_path).map_err(|error| error.to_string())?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_file(config_path);
        Ok(Self {
            runtime_dir: path_value(&value, "runtime_dir")?,
            host_binary: path_value(&value, "host_binary")?,
            url: string_value(&value, "url")?,
            app_id: value
                .get("app_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
            title: value
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Sabine")
                .to_string(),
            width: value
                .get("width")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(900) as u32,
            height: value
                .get("height")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(640) as u32,
            min_width: value
                .get("min_width")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(420) as u32,
            min_height: value
                .get("min_height")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(280) as u32,
            resizable: value
                .get("resizable")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            visible: value
                .get("visible")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            #[cfg(target_os = "linux")]
            shell_surface_alpha: value
                .get("shell_surface_alpha")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32,
            active: value
                .get("active")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            hide_on_blur: value
                .get("hide_on_blur")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            hide_on_close: value
                .get("hide_on_close")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            skip_taskbar: value
                .get("skip_taskbar")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            always_on_top: value
                .get("always_on_top")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            transparent: value
                .get("transparent")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            shell_surface: shell_surface_from_json(value.get("shell_surface")),
            background_effect: value
                .get("background_effect")
                .and_then(serde_json::Value::as_str)
                .and_then(WindowBackgroundEffect::parse)
                .unwrap_or(WindowBackgroundEffect::None),
            chrome: value
                .get("chrome")
                .and_then(serde_json::Value::as_str)
                .and_then(SabineWindowChrome::parse)
                .unwrap_or(SabineWindowChrome::Frameless),
            bridge_commands: value
                .get("bridge_commands")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            regions: regions_from_json(value.get("regions")),
            drag_regions: rects_from_json(value.get("drag_regions")),
            drag_exclusion_regions: rects_from_json(value.get("drag_exclusion_regions")),
            control_regions: control_regions_from_json(value.get("control_regions")),
            lifecycle: lifecycle_from_json(value.get("lifecycle")),
            dev_mode: value
                .get("dev_mode")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            remote_devtools_port: value
                .get("remote_devtools_port")
                .and_then(serde_json::Value::as_u64)
                .and_then(|port| u16::try_from(port).ok()),
            remote_devtools_disabled: value
                .get("remote_devtools_disabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            #[cfg(target_os = "linux")]
            vaapi_hardware_decode: value
                .get("vaapi_hardware_decode")
                .or_else(|| value.get("hardware_decode"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

pub(super) fn path_value(value: &serde_json::Value, key: &str) -> Result<PathBuf, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| format!("OSR host config missing {key}"))
}

pub(super) fn string_value(value: &serde_json::Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("OSR host config missing {key}"))
}
