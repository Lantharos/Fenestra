use std::process::Command;

pub(crate) const HOST_CONTROL_PREFIX: &str = "MULLION_HOST_CONTROL";
pub(crate) const DISABLED_CEF_FEATURES: &str = concat!(
    "Vulkan,",
    "DefaultANGLEVulkan,",
    "VulkanFromANGLE,",
    "OptimizationGuideOnDeviceModel,",
    "AutofillServerCommunication,",
    "MediaRouter,",
    "Translate,",
    "InterestFeedContentSuggestions"
);

const DEFAULT_REMOTE_DEVTOOLS_PORT: u16 = 9222;

/// Browser-process launch options (devtools, hardware decode, and related flags).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserOptions {
    pub remote_devtools_port: Option<u16>,
    pub remote_devtools_disabled: bool,
    #[cfg(target_os = "linux")]
    pub vaapi_hardware_decode: bool,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            remote_devtools_port: None,
            remote_devtools_disabled: false,
            #[cfg(target_os = "linux")]
            vaapi_hardware_decode: true,
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
            command.arg("--ozone-platform=wayland");
            if options.vaapi_hardware_decode {
                features.extend([
                    "VaapiVideoDecoder",
                    "VaapiIgnoreDriverChecks",
                    "VaapiOnNvidiaGPUs",
                ]);
            }
            features
        }
        #[cfg(not(target_os = "linux"))]
        {
            Vec::new()
        }
    };
    if !enabled_features.is_empty() {
        command.arg(format!("--enable-features={}", enabled_features.join(",")));
    }
    command
        .arg(format!("--disable-features={DISABLED_CEF_FEATURES}"))
        .arg("--disable-vulkan")
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
    if let Some(port) = options.effective_remote_devtools_port(dev_mode) {
        command.arg(format!("--remote-debugging-port={port}"));
        command.arg("--remote-allow-origins=*");
    }
}
