use std::process::Command;

pub(crate) const HOST_CONTROL_PREFIX: &str = "SABINE_HOST_CONTROL";

/// CEF features Sabine does not use.
const DISABLED_CEF_FEATURES: &str = concat!(
    "OptimizationGuideOnDeviceModel,",
    "AutofillServerCommunication,",
    "MediaRouter,",
    "Translate,",
    "InterestFeedContentSuggestions"
);

const DEFAULT_REMOTE_DEVTOOLS_PORT: u16 = 9222;

/// Browser-process launch options (devtools, hardware decode, and related flags).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BrowserOptions {
    pub remote_devtools_port: Option<u16>,
    pub remote_devtools_disabled: bool,
    pub memory_saver: bool,
    #[cfg(target_os = "linux")]
    pub vaapi_hardware_decode: bool,
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
    let disabled_features = if options.memory_saver {
        format!("{DISABLED_CEF_FEATURES},SpareRendererForSitePerProcess")
    } else {
        DISABLED_CEF_FEATURES.to_string()
    };
    command
        .arg(format!("--disable-features={disabled_features}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(options: &BrowserOptions) -> Vec<String> {
        let mut command = Command::new("sabine-host");
        apply_browser_launch_args(&mut command, options, false);
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn normal_policy_keeps_chromiums_spare_renderer() {
        assert!(
            !args(&BrowserOptions::default())
                .iter()
                .any(|arg| arg.contains("SpareRendererForSitePerProcess"))
        );
    }

    #[test]
    fn memory_saver_disables_chromiums_spare_renderer() {
        let options = BrowserOptions {
            memory_saver: true,
            ..BrowserOptions::default()
        };
        assert!(
            args(&options)
                .iter()
                .any(|arg| arg.contains("SpareRendererForSitePerProcess"))
        );
    }
}
