use sabine_bridge::BridgeHandlers;

pub mod app_chrome;
mod builder;
pub mod config;
mod dev;
pub mod glass;
mod launch;
pub mod manifest;
pub mod style;

pub use app_chrome::AppChrome;
pub use config::{
    DesktopServiceConfig, SabineLifecyclePolicy, SabineWindowChrome, SabineWindowConfig,
    SabineWindowControlAction, SabineWindowControlRegion,
};
pub use dev::{parse_localhost_port, vite_dev_command, vite_dev_url};
pub use glass::GlassSpec;

use crate::error::SabineResult;

/// Cross-platform Sabine window builder.
#[derive(Clone, Debug)]
pub struct SabineWindow {
    pub config: SabineWindowConfig,
    bridge_handlers: BridgeHandlers,
}

impl SabineWindow {
    /// Runs child host modes when needed, builds the window, then launches it.
    ///
    /// Typical app entry:
    /// ```ignore
    /// fn main() {
    ///     SabineWindow::main(|window| {
    ///         Ok(window.app().title("My App").entry("ui/index.html"))
    ///     });
    /// }
    /// ```
    pub fn main(build: impl FnOnce(Self) -> SabineResult<Self>) -> ! {
        let args = std::env::args().collect::<Vec<_>>();
        if crate::launch::run_sabine_host_from_args(&args) {
            std::process::exit(0);
        }
        let window = match build(Self::new()) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("failed to configure Sabine window: {error}");
                std::process::exit(1);
            }
        };
        match window.launch() {
            Ok(process) => match process.wait() {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(error) => {
                    eprintln!("Sabine process wait failed: {error}");
                    std::process::exit(1);
                }
            },
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
