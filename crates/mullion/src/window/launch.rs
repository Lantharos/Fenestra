use std::{
    net::{TcpStream, ToSocketAddrs},
    process::{Child, Stdio},
    thread,
    time::{Duration, Instant},
};

use mullion_bridge::{BridgeError, LaunchMetrics};
use mullion_runtime::{RuntimeInfo, resolve_runtime};

use super::{MullionWindow, MullionWindowConfig};
use crate::desktop::apply_desktop_services;
use crate::error::{MullionError, MullionResult};
use crate::host::{ManagedChild, MullionProcess, prepare_child_command};
use crate::launch::{
    allow_dev_origins, allow_url_origin, bootstrap, canonical_entry, dev_server_candidates,
    metrics_label, shell_command, split_entry_suffix,
};
use crate::osr;

impl MullionWindow {
    pub fn launch(self) -> MullionResult<MullionProcess> {
        bootstrap::prepare(&self.config)?;
        let runtime = resolve_runtime(&self.config.runtime)?;
        self.launch_with_runtime(runtime)
    }

    /// Resolve entry URL and config for [`MullionProcess::open_window`].
    /// Does not start desktop services or a second process island.
    pub(crate) fn into_open_window_parts(mut self) -> MullionResult<(MullionWindowConfig, String)> {
        self.ensure_default_bridge_handlers();
        self.allow_configured_url_origins();
        let url = self.entry_url()?;
        Ok((self.config, url))
    }

    pub(crate) fn launch_with_runtime(
        mut self,
        runtime: RuntimeInfo,
    ) -> MullionResult<MullionProcess> {
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = runtime;
            return Err(MullionError::MobileUnsupported);
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
                .map_err(|message| MullionError::CreationFailed { message })?,
            );
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            let desktop_services = None;
            metrics.mark("desktop_services.ready");
            self.ensure_default_bridge_handlers();
            self.allow_configured_url_origins();
            let mut dev_server = self.start_dev_command(&metrics)?;
            let mut url = self.entry_url()?;
            if self.config.dev_url.is_some() || dev_server.is_some() {
                match self.wait_for_dev_server(dev_server.as_mut(), &url) {
                    Ok(ready_url) => {
                        url = ready_url;
                        allow_dev_origins(&mut self.config.security, &url);
                        metrics.mark("dev_server.ready");
                    }
                    Err(error) => {
                        if let Some(child) = dev_server {
                            let _ = ManagedChild::new(child).terminate();
                        }
                        return Err(error);
                    }
                }
            }
            let mut process = osr::launch_process(
                runtime.location.path(),
                &self.config,
                &self.bridge_handlers,
                &url,
                metrics.clone(),
            )?;
            if let Some(dev_server) = dev_server {
                metrics.mark("dev_server.attached");
                process.sidecars.push(ManagedChild::new(dev_server));
            }
            process.desktop_services = desktop_services;
            process.start_desktop_event_forwarder();
            metrics.mark("launch.ready");
            Ok(process)
        }
    }

    fn start_dev_command(&self, metrics: &LaunchMetrics) -> MullionResult<Option<Child>> {
        let Some(command) = &self.config.dev_command else {
            return Ok(None);
        };
        if command.trim().is_empty() {
            return Ok(None);
        }
        let mut process = shell_command(command);
        prepare_child_command(&mut process);
        process
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let child = process
            .spawn()
            .map_err(|error| MullionError::CreationFailed {
                message: format!("failed to start dev command `{command}`: {error}"),
            })?;
        metrics.mark("dev_command.spawned");
        Ok(Some(child))
    }

    fn wait_for_dev_server(
        &self,
        mut dev_server: Option<&mut Child>,
        url: &str,
    ) -> MullionResult<String> {
        let candidates = dev_server_candidates(url);
        if candidates.is_empty() {
            return Ok(url.to_string());
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last_error = None;
        while Instant::now() < deadline {
            if let Some(child) = dev_server.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(MullionError::CreationFailed {
                        message: format!(
                            "dev command exited before `{url}` became available: {status}"
                        ),
                    });
                }
            }
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

        Err(MullionError::CreationFailed {
            message: format!(
                "timed out waiting for dev server `{url}`{}",
                last_error
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            ),
        })
    }

    pub(crate) fn entry_url(&self) -> MullionResult<String> {
        if let Some(url) = &self.config.dev_url {
            return Ok(url.clone());
        }
        if let Some(url) = &self.config.url {
            return Ok(url.clone());
        }
        let Some(entry) = &self.config.entry else {
            return Err(MullionError::CreationFailed {
                message: "CEF window has no entry, URL, or dev URL".to_string(),
            });
        };
        let (entry_path, suffix) = split_entry_suffix(entry);
        let path = canonical_entry(entry_path)?;
        Ok(format!("file://{}{}", path.display(), suffix))
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
