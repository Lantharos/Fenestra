use sabine_bridge::BridgeHandlers;

mod app_chrome;
mod builder;
pub(crate) mod config;
mod launch;
mod manifest;
pub(crate) mod style;

pub use app_chrome::AppChrome;
use config::SabineWindowConfig;
pub use config::{
    SabineLifecyclePolicy, SabineWindowChrome, SabineWindowControlAction, SabineWindowControlRegion,
};

use crate::{error::SabineResult, host::SabineProcess};

/// Cross-platform Sabine window builder.
#[derive(Clone, Debug)]
pub struct SabineWindow {
    pub(crate) config: SabineWindowConfig,
    bridge_handlers: BridgeHandlers,
}

impl SabineWindow {
    /// Runs child host modes when needed, builds the window, then launches it.
    ///
    /// Typical app entry:
    /// ```ignore
    /// fn main() {
    ///     SabineWindow::main(|window| {
    ///         Ok(window.app().title("My App"))
    ///     });
    /// }
    /// ```
    pub fn main(build: impl FnOnce(Self) -> SabineResult<Self>) -> ! {
        Self::main_with_process(build, |_| {})
    }

    /// Runs a Sabine app and exposes the launched process before waiting.
    ///
    /// Use this when application state needs the process event emitter or
    /// launch metrics for the lifetime of the window.
    pub fn main_with_process(
        build: impl FnOnce(Self) -> SabineResult<Self>,
        launched: impl FnOnce(&SabineProcess),
    ) -> ! {
        Self::main_with_process_mut(build, |process| launched(process))
    }

    /// Runs a Sabine app and allows opening same-process windows before the
    /// process enters its wait loop.
    pub fn main_with_process_mut(
        build: impl FnOnce(Self) -> SabineResult<Self>,
        launched: impl FnOnce(&mut SabineProcess),
    ) -> ! {
        let args = std::env::args().collect::<Vec<_>>();
        if crate::dispatch_host_mode_from_args(&args) {
            std::process::exit(0);
        }
        let window = match Self::new().with_framework_config().and_then(build) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("failed to configure Sabine window: {error}");
                std::process::exit(1);
            }
        };
        match window.launch() {
            Ok(mut process) => {
                launched(&mut process);
                match process.wait() {
                    Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                    Err(error) => {
                        eprintln!("Sabine process wait failed: {error}");
                        std::process::exit(1);
                    }
                }
            }
            Err(crate::SabineError::InstanceAlreadyRunning) => std::process::exit(0),
            Err(error) => {
                eprintln!("failed to launch Sabine window: {error}");
                std::process::exit(1);
            }
        }
    }

    /// Conventional desktop app: system chrome, opaque, browser-tab lifecycle.
    pub fn app(self) -> Self {
        self.system_chrome()
            .opaque()
            .lifecycle_policy(SabineLifecyclePolicy::browser_tab())
    }

    /// Frameless glass palette/launcher with hide-on-blur and palette lifecycle.
    pub fn palette(self) -> Self {
        self.frameless()
            .glass()
            .hide_on_blur(true)
            .lifecycle_policy(SabineLifecyclePolicy::hidden_window())
    }

    /// Warm background/tray host: starts hidden with palette lifecycle.
    /// Pair with [`Self::tray_icon`] and [`Self::single_instance_id`].
    pub fn tray_app(self) -> Self {
        self.hidden()
            .lifecycle_policy(SabineLifecyclePolicy::hidden_window())
    }

    pub fn new() -> Self {
        Self {
            config: SabineWindowConfig::default(),
            bridge_handlers: BridgeHandlers::default(),
        }
    }
}

impl Default for SabineWindow {
    fn default() -> Self {
        Self::new()
    }
}
