use std::{future::Future, time::Duration};

use sabine_bridge::{
    BridgeCommand, BridgeCommandDescriptor, BridgeError, BridgeResponse, BridgeResult,
    ContentSecurity,
};
use sabine_platform::{
    AutostartEntry, DeepLinkRegistration, GlobalShortcutRegistration, NativeMessagingHost,
    ShellSurfaceOptions, SingleInstancePolicy, TrayIcon, WindowBackgroundEffect, WindowRegion,
    WindowRegionRect, WindowRegions,
};
use sabine_runtime::RuntimeConfig;

use super::{
    SabineLifecyclePolicy, SabineWindow, SabineWindowChrome, SabineWindowControlAction,
    SabineWindowControlRegion,
};
use crate::launch::{allow_dev_origins, allow_origin, allow_url_origin};

impl SabineWindow {
    pub fn entry(mut self, path: impl Into<String>) -> Self {
        self.config.entry = Some(path.into());
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        allow_url_origin(&mut self.config.security, &url);
        self.config.url = Some(url);
        self
    }

    pub fn dev_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        allow_dev_origins(&mut self.config.security, &url);
        self.config.dev_url = Some(url);
        self
    }

    /// Appends a query string or fragment to the configured web entry.
    pub fn content_suffix(mut self, suffix: impl AsRef<str>) -> Self {
        let suffix = suffix.as_ref();
        if let Some(value) = self.config.dev_url.as_mut() {
            value.push_str(suffix);
        } else if let Some(value) = self.config.url.as_mut() {
            value.push_str(suffix);
        } else if let Some(value) = self.config.entry.as_mut() {
            value.push_str(suffix);
        }
        self
    }

    /// Enables Chrome remote debugging on the given port.
    ///
    /// When `dev_url` is configured, remote DevTools are enabled automatically on port 9222.
    /// Attach from Chrome at `chrome://inspect` or open `http://127.0.0.1:9222`.
    pub fn debug(mut self, port: u16) -> Self {
        self.config.browser.remote_devtools_port = Some(port);
        self.config.browser.remote_devtools_disabled = false;
        self
    }

    /// Disables remote DevTools even when `dev_url` is configured.
    pub fn without_debug(mut self) -> Self {
        self.config.browser.remote_devtools_disabled = true;
        self
    }

    /// Linux only: enable VA-API hardware video decode for WebCodecs/MSE.
    ///
    /// Disabled by default (driver paths are crash-prone on many GPUs).
    #[cfg(target_os = "linux")]
    pub fn vaapi_hardware_decode(mut self, enabled: bool) -> Self {
        self.config.browser.vaapi_hardware_decode = enabled;
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.config.app_id = Some(app_id.into());
        self
    }

    pub fn app_version(mut self, version: impl Into<String>) -> Self {
        self.config.app_version = Some(version.into());
        self
    }

    pub fn app_updates(mut self, updates: sabine_service::AppUpdateConfig) -> Self {
        self.config.app_update = Some(updates);
        self
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self
    }

    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.config.min_width = width;
        self.config.min_height = height;
        self
    }

    pub fn fixed_size(mut self, width: u32, height: u32) -> Self {
        self.config.width = width;
        self.config.height = height;
        self.config.min_width = width;
        self.config.min_height = height;
        self.config.resizable = false;
        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.config.resizable = resizable;
        self
    }

    pub fn visible(mut self, visible: bool) -> Self {
        self.config.visible = visible;
        if !visible {
            self.apply_hidden_lifecycle_defaults();
        }
        self
    }

    pub fn shell_surface_alpha(mut self, alpha: f32) -> Self {
        self.config.shell_surface_alpha = alpha.clamp(0.0, 1.0);
        self
    }

    pub fn hidden(self) -> Self {
        self.visible(false).active(false)
    }

    pub fn active(mut self, active: bool) -> Self {
        self.config.active = active;
        self
    }

    pub fn hide_on_blur(mut self, enabled: bool) -> Self {
        self.config.hide_on_blur = enabled;
        if enabled {
            self.apply_hidden_lifecycle_defaults();
        }
        self
    }

    pub fn hide_on_close(mut self, enabled: bool) -> Self {
        self.config.hide_on_close = enabled;
        self
    }

    pub fn skip_taskbar(mut self, enabled: bool) -> Self {
        self.config.skip_taskbar = enabled;
        self
    }

    pub fn always_on_top(mut self, always_on_top: bool) -> Self {
        self.config.always_on_top = always_on_top;
        self
    }

    pub fn transparent(mut self, transparent: bool) -> Self {
        self.config.transparent = transparent;
        if !transparent {
            self.config.background_effect = WindowBackgroundEffect::None;
            self.config.regions.blur = None;
        }
        self
    }

    pub fn opaque(mut self) -> Self {
        self.config.transparent = false;
        self.config.background_effect = WindowBackgroundEffect::None;
        self.config.regions.blur = None;
        self
    }

    pub fn frameless(mut self) -> Self {
        self.config.chrome = SabineWindowChrome::Frameless;
        self
    }

    pub fn sabine_chrome(mut self) -> Self {
        self.config.chrome = SabineWindowChrome::Sabine;
        self
    }

    pub fn system_chrome(mut self) -> Self {
        self.config.chrome = SabineWindowChrome::System;
        self
    }

    pub fn no_chrome(mut self) -> Self {
        self.config.chrome = SabineWindowChrome::None;
        self
    }

    pub fn chrome(mut self, chrome: SabineWindowChrome) -> Self {
        self.config.chrome = chrome;
        self
    }

    pub fn glass(self) -> Self {
        let effect = match sabine_platform::current_desktop_os() {
            sabine_platform::PlatformOs::Windows => WindowBackgroundEffect::Acrylic,
            sabine_platform::PlatformOs::Macos => WindowBackgroundEffect::Vibrancy,
            sabine_platform::PlatformOs::Linux => WindowBackgroundEffect::Blur,
            _ => WindowBackgroundEffect::None,
        };
        self.background_effect(effect)
    }

    pub fn background_effect(mut self, effect: WindowBackgroundEffect) -> Self {
        self.config.background_effect = effect;
        if effect.requires_transparency() {
            self.config.transparent = true;
        }
        self
    }

    pub fn regions(mut self, regions: WindowRegions) -> Self {
        self.config.regions = regions;
        self
    }

    pub fn shell_surface(mut self, shell_surface: ShellSurfaceOptions) -> Self {
        self.config.shell_surface = Some(shell_surface);
        self.config.chrome = SabineWindowChrome::None;
        self.config.transparent = true;
        self
    }

    pub fn blur_region(mut self, region: WindowRegion) -> Self {
        self.config.regions.blur = Some(region);
        self
    }

    pub fn opaque_region(mut self, region: WindowRegion) -> Self {
        self.config.regions.opaque = Some(region);
        self
    }

    pub fn input_region(mut self, region: WindowRegion) -> Self {
        self.config.regions.input = Some(region);
        self
    }

    pub fn drag_region(mut self, rect: WindowRegionRect) -> Self {
        self.config.drag_regions.push(rect);
        self
    }

    pub fn drag_exclusion_region(mut self, rect: WindowRegionRect) -> Self {
        self.config.drag_exclusion_regions.push(rect);
        self
    }

    pub fn titlebar_drag_region(self, height: i32) -> Self {
        self.drag_region(WindowRegionRect::new(0, 0, i32::MAX, height))
    }

    pub fn control_region(
        mut self,
        action: SabineWindowControlAction,
        rect: WindowRegionRect,
    ) -> Self {
        self.config
            .control_regions
            .push(SabineWindowControlRegion::new(action, rect));
        self
    }

    pub fn tray_icon(mut self, icon: TrayIcon) -> Self {
        self.config.desktop_services.tray_icon = Some(icon);
        self
    }

    pub fn autostart(mut self, entry: AutostartEntry) -> Self {
        self.config.desktop_services.autostart.push(entry);
        self
    }

    pub fn global_shortcut(mut self, registration: GlobalShortcutRegistration) -> Self {
        self.config
            .desktop_services
            .global_shortcuts
            .push(registration);
        self
    }

    pub fn deep_link(mut self, registration: DeepLinkRegistration) -> Self {
        self.config.desktop_services.deep_links.push(registration);
        self
    }

    pub fn native_messaging_host(mut self, host: NativeMessagingHost) -> Self {
        self.config
            .desktop_services
            .native_messaging_hosts
            .push(host);
        self
    }

    pub fn single_instance(mut self, policy: SingleInstancePolicy) -> Self {
        self.config.desktop_services.single_instance_policy = Some(policy);
        self
    }

    pub fn single_instance_id(mut self, id: impl Into<String>) -> Self {
        self.config.desktop_services.single_instance_id = Some(id.into());
        self
    }

    pub fn lifecycle_policy(mut self, lifecycle: SabineLifecyclePolicy) -> Self {
        self.config.lifecycle = lifecycle;
        self
    }

    pub fn active_frame_rate(mut self, frame_rate: u32) -> Self {
        self.config.lifecycle.active_frame_rate = frame_rate;
        self
    }

    pub fn background_frame_rate(mut self, frame_rate: u32) -> Self {
        self.config.lifecycle.background_frame_rate = frame_rate.max(1);
        self
    }

    pub fn suspend_on_minimize(mut self, enabled: bool) -> Self {
        self.config.lifecycle.suspend_on_minimize = enabled;
        self
    }

    pub fn suspend_on_occluded(mut self, enabled: bool) -> Self {
        self.config.lifecycle.suspend_on_occluded = enabled;
        self
    }

    pub fn suspend_on_blur(mut self, enabled: bool) -> Self {
        self.config.lifecycle.suspend_on_blur = enabled;
        self
    }

    pub fn hibernate_after(mut self, duration: Duration) -> Self {
        self.config.lifecycle.hibernate_after = Some(duration);
        self
    }

    pub fn disable_hibernation(mut self) -> Self {
        self.config.lifecycle.hibernate_after = None;
        self
    }

    pub fn retain_hidden_frame(mut self, enabled: bool) -> Self {
        self.config.lifecycle.retain_hidden_frame = enabled;
        self
    }

    pub fn memory_saver(mut self, enabled: bool) -> Self {
        self.config.lifecycle.memory_saver = enabled;
        self
    }

    fn apply_hidden_lifecycle_defaults(&mut self) {
        self.config.lifecycle.suspend_on_blur = true;
        self.config.lifecycle.background_frame_rate = 1;
        self.config.lifecycle.retain_hidden_frame = true;
        self.config.lifecycle.hibernate_grace = self
            .config
            .lifecycle
            .hibernate_grace
            .min(Duration::from_millis(150));
    }

    pub fn runtime(mut self, runtime: RuntimeConfig) -> Self {
        self.config.runtime = runtime;
        self
    }

    pub fn security(mut self, security: ContentSecurity) -> Self {
        self.config.security = security;
        self
    }

    pub fn allowed_origin(mut self, origin: impl Into<String>) -> Self {
        allow_origin(&mut self.config.security, origin.into());
        self
    }

    pub fn bridge_command_descriptor(mut self, descriptor: BridgeCommandDescriptor) -> Self {
        self.config.bridge.register_descriptor(descriptor);
        self
    }

    pub fn bridge_handler<F>(mut self, command_name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        let name = command_name.into();
        self.config.bridge.register(name.clone());
        self.bridge_handlers.register(name, handler);
        self
    }

    /// Registers a bridge command that deserializes params into `Req` and
    /// serializes the handler return value as JSON.
    pub fn bridge_typed<Req, Res, F>(self, command_name: impl Into<String>, handler: F) -> Self
    where
        Req: serde::de::DeserializeOwned,
        Res: serde::Serialize,
        F: Fn(Req) -> Result<Res, BridgeError> + Send + Sync + 'static,
    {
        self.bridge_handler(command_name, move |command| {
            let request = serde_json::from_value(command.params)
                .map_err(|error| BridgeError::new(format!("invalid bridge params: {error}")))?;
            let response = handler(request)?;
            let value = serde_json::to_value(response).map_err(|error| {
                BridgeError::new(format!("failed to encode bridge result: {error}"))
            })?;
            Ok(BridgeResponse::json(value))
        })
    }

    pub fn bridge_descriptor_handler<F>(
        mut self,
        descriptor: BridgeCommandDescriptor,
        handler: F,
    ) -> Self
    where
        F: Fn(BridgeCommand) -> BridgeResult + Send + Sync + 'static,
    {
        let name = descriptor.name.clone();
        self.config.bridge.register_descriptor(descriptor);
        self.bridge_handlers.register(name, handler);
        self
    }

    pub fn bridge_handler_async<F, Fut>(
        mut self,
        command_name: impl Into<String>,
        handler: F,
    ) -> Self
    where
        F: Fn(BridgeCommand) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BridgeResult> + Send + 'static,
    {
        let name = command_name.into();
        self.config.bridge.register(name.clone());
        self.bridge_handlers
            .register(name, move |command| pollster::block_on(handler(command)));
        self
    }

    pub fn bridge_descriptor_handler_async<F, Fut>(
        mut self,
        descriptor: BridgeCommandDescriptor,
        handler: F,
    ) -> Self
    where
        F: Fn(BridgeCommand) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = BridgeResult> + Send + 'static,
    {
        let name = descriptor.name.clone();
        self.config.bridge.register_descriptor(descriptor);
        self.bridge_handlers
            .register(name, move |command| pollster::block_on(handler(command)));
        self
    }
}
