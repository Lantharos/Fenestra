use std::process::Command;

pub(crate) const HOST_CONTROL_PREFIX: &str = "SABINE_HOST_CONTROL";

/// Features disabled for the normal (accelerated) CEF host profile.
pub(crate) const DISABLED_CEF_FEATURES: &str = concat!(
    // Linux shared-texture OSR uses ANGLE GL-EGL; keep Vulkan out of the GPU process.
    "Vulkan,",
    "DefaultANGLEVulkan,",
    "VulkanFromANGLE,",
    "OptimizationGuideOnDeviceModel,",
    "AutofillServerCommunication,",
    "MediaRouter,",
    "Translate,",
    "InterestFeedContentSuggestions"
);

/// Extra features disabled only when silently falling back to software OSR.
pub(crate) const SOFTWARE_FALLBACK_CEF_FEATURES: &str = DISABLED_CEF_FEATURES;

const DEFAULT_REMOTE_DEVTOOLS_PORT: u16 = 9222;

/// Browser-process launch options (devtools, hardware decode, and related flags).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserOptions {
    pub remote_devtools_port: Option<u16>,
    pub remote_devtools_disabled: bool,
    /// Internal: force software OSR (no shared textures). Not a public API.
    pub software_osr_fallback: bool,
    /// Linux only: opt into CEF shared-texture (DMA-BUF) OSR. Off by default —
    /// Chromium's path still fails SkSurface init on NVIDIA and some Mesa builds.
    #[cfg(target_os = "linux")]
    pub shared_texture_osr: bool,
    #[cfg(target_os = "linux")]
    pub vaapi_hardware_decode: bool,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            remote_devtools_port: None,
            remote_devtools_disabled: false,
            software_osr_fallback: false,
            #[cfg(target_os = "linux")]
            shared_texture_osr: false,
            // Off by default: VaapiIgnoreDriverChecks / Nvidia paths crash many GPU processes.
            #[cfg(target_os = "linux")]
            vaapi_hardware_decode: false,
        }
    }
}

impl BrowserOptions {
    pub fn effective_remote_devtools_port(&self, dev_mode: bool) -> Option<u16> {
        if self.remote_devtools_disabled {
            return None;
        }
        if let Some(port) = self.remote_devtools_port {
            return Some(port);
        }
        if dev_mode {
            return Some(DEFAULT_REMOTE_DEVTOOLS_PORT);
        }
        None
    }

    pub fn hardware_decode_enabled(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.vaapi_hardware_decode
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
}

pub(crate) fn apply_browser_launch_args(
    command: &mut Command,
    options: &BrowserOptions,
    dev_mode: bool,
) {
    let enabled_features: Vec<&str> = {
        #[cfg(target_os = "linux")]
        {
            let mut features = vec!["UseOzonePlatform"];
            // Default: Wayland ozone (matches the Sabine shell). Shared-texture
            // OSR still needs X11 + ANGLE GL-EGL per CEF #3953/#3954.
            let use_shared = options.shared_texture_osr && !options.software_osr_fallback;
            if use_shared {
                command.arg("--ozone-platform=x11");
                command.arg("--use-gl=angle");
                command.arg("--use-angle=gl-egl");
            } else {
                command.arg("--ozone-platform=wayland");
            }
            // Wayland ozone rejects Vulkan; keep it off on Linux CEF generally.
            command.arg("--disable-vulkan");
            if options.vaapi_hardware_decode {
                features.push("VaapiVideoDecoder");
            }
            features
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = options;
            Vec::new()
        }
    };
    if !enabled_features.is_empty() {
        command.arg(format!("--enable-features={}", enabled_features.join(",")));
    }
    let disabled = if options.software_osr_fallback {
        SOFTWARE_FALLBACK_CEF_FEATURES
    } else {
        DISABLED_CEF_FEATURES
    };
    command
        .arg(format!("--disable-features={disabled}"))
        .arg("--disable-background-networking")
        .arg("--disable-component-update")
        .arg("--disable-component-extensions-with-background-pages")
        .arg("--disable-default-apps")
        .arg("--disable-domain-reliability")
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg("--disable-translate")
        .arg("--disable-breakpad")
        .arg("--disable-crash-reporter")
        .arg("--metrics-recording-only")
        .arg("--no-default-browser-check")
        .arg("--no-first-run")
        .arg("--password-store=basic");
    if options.software_osr_fallback {
        command.arg("--disable-vulkan");
        command.arg("--sabine-software-osr");
    }
    #[cfg(target_os = "linux")]
    if options.shared_texture_osr && !options.software_osr_fallback {
        command.arg("--sabine-shared-texture");
    }
    if let Some(port) = options.effective_remote_devtools_port(dev_mode) {
        command.arg(format!("--remote-debugging-port={port}"));
        command.arg("--remote-allow-origins=*");
    }
}
