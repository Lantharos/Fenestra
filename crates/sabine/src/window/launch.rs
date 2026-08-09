use std::{
    net::{TcpStream, ToSocketAddrs},
    thread,
    time::{Duration, Instant},
};

use sabine_bridge::{BridgeError, LaunchMetrics};
use sabine_runtime::{RuntimeInfo, resolve_runtime};

use super::{SabineWindow, SabineWindowConfig};
use crate::desktop::apply_desktop_services;
use crate::error::{SabineError, SabineResult};
use crate::host::SabineProcess;
use crate::launch::{
    allow_dev_origins, allow_url_origin, bootstrap, canonical_entry, dev_server_candidates,
    metrics_label, split_entry_suffix,
};
use crate::osr;

impl SabineWindow {
    pub fn launch(self) -> SabineResult<SabineProcess> {
        self.config.validate()?;
        bootstrap::prepare(&self.config)?;
        let runtime = resolve_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    /// Resolve entry URL and config for [`SabineProcess::open_window`].
    /// Does not start desktop services or a second process island.
    pub(crate) fn into_open_window_parts(mut self) -> SabineResult<(SabineWindowConfig, String)> {
        self.config.validate()?;
        self.ensure_default_bridge_handlers();
        self.allow_configured_url_origins();
        let url = self.entry_url()?;
        Ok((self.config, url))
    }

    pub(crate) fn launch_with_runtime(
        mut self,
        runtime: RuntimeInfo,
    ) -> SabineResult<SabineProcess> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = runtime;
            return Err(SabineError::MobileUnsupported);
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let metrics = LaunchMetrics::new(metrics_label(&self.config));
            metrics.mark("launch.start");
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            let desktop_services = Some(
                apply_desktop_services(
                    self.config.desktop_services.tray_icon.as_ref(),
                    &self.config.desktop_services.autostart,
                    &self.config.desktop_services.global_shortcuts,
                    &self.config.desktop_services.deep_links,
                    &self.config.desktop_services.native_messaging_hosts,
                    self.config.desktop_services.single_instance_id.as_deref(),
                    self.config.desktop_services.single_instance_policy,
                )
                .map_err(|message| {
                    if message == crate::desktop::INSTANCE_ALREADY_RUNNING {
                        SabineError::InstanceAlreadyRunning
                    } else {
                        SabineError::CreationFailed { message }
                    }
                })?,
            );
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            let desktop_services = None;
            metrics.mark("desktop_services.ready");
            self.ensure_default_bridge_handlers();
            self.apply_dev_env_overrides();
            self.allow_configured_url_origins();
            let mut url = self.entry_url()?;
            if self.config.dev_url.is_some() {
                match self.wait_for_dev_server(&url) {
                    Ok(ready_url) => {
                        url = ready_url;
                        allow_dev_origins(&mut self.config.security, &url);
                        metrics.mark("dev_server.ready");
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut process = osr::launch_process(
                runtime.location.path(),
                &self.config,
                &self.bridge_handlers,
                &url,
                metrics.clone(),
            )?;
            process.desktop_services = desktop_services;
            process.start_desktop_event_forwarder();
            metrics.mark("launch.ready");
            Ok(process)
        }
    }

    fn apply_dev_env_overrides(&mut self) {
        if let Ok(url) = std::env::var("SABINE_DEV_URL") {
            let url = url.trim();
            if !url.is_empty() {
                allow_dev_origins(&mut self.config.security, url);
                self.config.dev_url = Some(url.to_string());
            }
        }
    }

    fn wait_for_dev_server(&self, url: &str) -> SabineResult<String> {
        let candidates = dev_server_candidates(url);
        if candidates.is_empty() {
            return Ok(url.to_string());
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last_error = None;
        while Instant::now() < deadline {
            for candidate in &candidates {
                match (candidate.host.as_str(), candidate.port).to_socket_addrs() {
                    Ok(addresses) => {
                        for socket in addresses {
                            match TcpStream::connect_timeout(&socket, Duration::from_millis(150)) {
                                Ok(_) => return Ok(candidate.url.clone()),
                                Err(error) => last_error = Some(error),
                            }
                        }
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            thread::sleep(Duration::from_millis(50));
        }

        Err(SabineError::CreationFailed {
            message: format!(
                "timed out waiting for dev server `{url}`{}",
                last_error
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            ),
        })
    }

    pub(crate) fn entry_url(&self) -> SabineResult<String> {
        if let Some(url) = &self.config.dev_url {
            return Ok(url.clone());
        }
        if let Some(url) = &self.config.url {
            return Ok(url.clone());
        }
        let Some(entry) = &self.config.entry else {
            return Err(SabineError::CreationFailed {
                message: "CEF window has no entry, URL, or dev URL".to_string(),
            });
        };
        let (entry_path, suffix) = split_entry_suffix(entry);
        let path = canonical_entry(entry_path)?;
        Ok(format!("{}{}", crate::launch::file_url(&path)?, suffix))
    }

    fn ensure_default_bridge_handlers(&mut self) {
        for command in self.config.bridge.commands() {
            if self.bridge_handlers.contains(&command) {
                continue;
            }
            let command_name = command.clone();
            self.bridge_handlers.register(command, move |_| {
                Err(BridgeError::new(format!(
                    "Bridge command `{command_name}` has no Rust handler"
                )))
            });
        }
    }

    fn allow_configured_url_origins(&mut self) {
        if let Some(url) = self.config.url.clone() {
            allow_url_origin(&mut self.config.security, &url);
        }
        if let Some(url) = self.config.dev_url.clone() {
            allow_dev_origins(&mut self.config.security, &url);
        }
    }
}
