use std::{
    process::ExitStatus,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use mullion_bridge::{
    ActivityOptions, ActivityRecord, LaunchMetrics, MullionActivityLease,
    MullionLaunchMetricsSnapshot,
};
use mullion_platform::{PlatformEvent, ShellSurfaceMargin, SingleInstancePolicy};

use crate::bridge::{BridgeEventEmitter, platform_event_payload};
use crate::desktop::{DesktopServiceState, start_desktop_event_forwarder};
use crate::host::process_tree::ManagedChild;
use crate::osr::launch::OpenWindowContext;
use crate::{MullionResult, MullionWindow, MullionWindowConfig};

pub struct MullionProcess {
    pub(crate) child: ManagedChild,
    pub(crate) primary_alive: bool,
    pub(crate) primary_status: Option<ExitStatus>,
    pub(crate) sidecars: Vec<ManagedChild>,
    pub(crate) extra_windows: Vec<ManagedChild>,
    pub(crate) bridge_thread: Option<JoinHandle<()>>,
    pub(crate) extra_bridge_threads: Vec<JoinHandle<()>>,
    pub(crate) bridge_emitter: Option<BridgeEventEmitter>,
    pub(crate) desktop_services: Option<DesktopServiceState>,
    pub(crate) desktop_event_thread: Option<JoinHandle<()>>,
    pub(crate) desktop_event_running: Option<Arc<AtomicBool>>,
    pub(crate) activity: mullion_bridge::ActivityRegistry,
    pub(crate) metrics: LaunchMetrics,
    pub(crate) open_window: Option<OpenWindowContext>,
}

/// Identifier for a window opened via [`MullionProcess::open_window`].
/// Matches the OSR-host child process id.
pub type WindowId = u32;

impl MullionProcess {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Open another OSR window in this process. Shares this process's bridge
    /// handlers and `app_id`. Handlers registered on `window` are ignored.
    pub fn open_window(&mut self, window: MullionWindow) -> MullionResult<WindowId> {
        let (config, url) = window.into_open_window_parts()?;
        crate::osr::launch::attach_open_window(self, &config, &url)
    }

    /// Open another window from a config and already-resolved URL.
    pub fn open_window_with_config(
        &mut self,
        config: MullionWindowConfig,
        url: impl Into<String>,
    ) -> MullionResult<WindowId> {
        crate::osr::launch::attach_open_window(self, &config, &url.into())
    }

    /// Close one OSR window. Returns `false` if the id is unknown. Closing the
    /// last remaining window leaves `wait()` free to return.
    pub fn close_window(&mut self, window_id: WindowId) -> bool {
        if self.primary_alive && self.child.id() == window_id {
            if let Some(emitter) = &self.bridge_emitter {
                emitter.detach(window_id);
            }
            self.primary_status = self.child.terminate();
            self.primary_alive = false;
            return true;
        }
        if let Some(index) = self
            .extra_windows
            .iter()
            .position(|window| window.id() == window_id)
        {
            let mut window = self.extra_windows.remove(index);
            if let Some(emitter) = &self.bridge_emitter {
                emitter.detach(window_id);
            }
            let _ = window.terminate();
            true
        } else {
            false
        }
    }

    /// Block until every OSR window has exited, then tear down sidecars/bridge.
    pub fn wait(mut self) -> std::io::Result<ExitStatus> {
        loop {
            if self.primary_alive {
                match self.child.try_wait()? {
                    Some(status) => {
                        if let Some(emitter) = &self.bridge_emitter {
                            emitter.detach(self.child.id());
                        }
                        self.primary_alive = false;
                        self.primary_status = Some(status);
                    }
                    None => {}
                }
            }

            let emitter = self.bridge_emitter.clone();
            self.extra_windows
                .retain_mut(|window| match window.try_wait() {
                    Ok(Some(_)) => {
                        if let Some(emitter) = &emitter {
                            emitter.detach(window.id());
                        }
                        false
                    }
                    Ok(None) => true,
                    Err(_) => {
                        if let Some(emitter) = &emitter {
                            emitter.detach(window.id());
                        }
                        false
                    }
                });

            if !self.primary_alive && self.extra_windows.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        self.cleanup_sidecars();
        self.stop_desktop_event_forwarder();
        self.join_bridge_threads();
        self.primary_status.ok_or_else(|| {
            std::io::Error::other("Mullion process exited without a primary window status")
        })
    }

    pub fn take_desktop_events(&self) -> Vec<PlatformEvent> {
        self.desktop_services
            .as_ref()
            .map(DesktopServiceState::take_events)
            .unwrap_or_default()
    }

    pub fn emit_bridge_event(&self, name: impl Into<String>, payload: serde_json::Value) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.emit(name, payload))
    }

    /// Drive a guest surface from Rust via host control. See
    /// [`BridgeEventEmitter::guest_control`].
    pub fn guest_control(&self, control: &mullion_bridge::GuestHostControl) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.guest_control(control))
    }

    pub fn set_shell_surface_visible(&self, visible: bool) -> bool {
        self.set_visible(visible)
    }

    pub fn set_shell_surface_alpha(&self, alpha: f32) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.set_alpha(alpha))
    }

    pub fn set_shell_surface_margin(&self, margin: ShellSurfaceMargin) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.set_margin(margin))
    }

    pub fn set_visible(&self, visible: bool) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(|emitter| emitter.set_visible(visible))
    }

    pub fn show(&self) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(BridgeEventEmitter::show)
    }

    pub fn hide(&self) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(BridgeEventEmitter::hide)
    }

    pub fn focus_window(&self) -> bool {
        self.bridge_emitter
            .as_ref()
            .is_some_and(BridgeEventEmitter::focus_window)
    }

    pub fn begin_activity(&self, name: impl Into<String>) -> MullionActivityLease {
        self.begin_activity_with(ActivityOptions::new(name))
    }

    pub fn begin_activity_with(&self, options: ActivityOptions) -> MullionActivityLease {
        let emitter = self.bridge_emitter.clone().map(|emitter| {
            std::sync::Arc::new(emitter) as std::sync::Arc<dyn mullion_bridge::ActivityEventEmitter>
        });
        self.activity.lease(options, emitter)
    }

    pub fn activities(&self) -> Vec<ActivityRecord> {
        self.activity.list()
    }

    pub fn bridge_event_emitter(&self) -> Option<BridgeEventEmitter> {
        self.bridge_emitter.clone()
    }

    pub fn metrics(&self) -> MullionLaunchMetricsSnapshot {
        self.metrics.snapshot()
    }

    pub(crate) fn start_desktop_event_forwarder(&mut self) {
        let (Some(services), Some(emitter)) =
            (self.desktop_services.as_ref(), self.bridge_emitter.clone())
        else {
            return;
        };
        let running = Arc::new(AtomicBool::new(true));
        self.desktop_event_running = Some(Arc::clone(&running));
        self.desktop_event_thread = Some(start_desktop_event_forwarder(
            services,
            running,
            move |event| {
                if let PlatformEvent::SingleInstance(activation) = &event
                    && activation.policy == SingleInstancePolicy::FocusExisting
                {
                    let _ = emitter.show();
                    let _ = emitter
                        .focus_window_with_activation_token(activation.activation_token.as_deref());
                }
                let (name, payload) = platform_event_payload(event);
                let _ = emitter.emit(name, payload);
            },
        ));
    }

    fn cleanup_extra_windows(&mut self) {
        for window in &mut self.extra_windows {
            if let Some(emitter) = &self.bridge_emitter {
                emitter.detach(window.id());
            }
            let _ = window.terminate();
        }
        self.extra_windows.clear();
    }

    fn cleanup_sidecars(&mut self) {
        for sidecar in &mut self.sidecars {
            let _ = sidecar.terminate();
        }
        self.sidecars.clear();
    }

    fn stop_desktop_event_forwarder(&mut self) {
        if let Some(running) = &self.desktop_event_running {
            running.store(false, Ordering::Relaxed);
        }
        if let Some(thread) = self.desktop_event_thread.take() {
            let _ = thread.join();
        }
        self.desktop_event_running = None;
    }

    fn join_bridge_threads(&mut self) {
        if let Some(thread) = self.bridge_thread.take() {
            let _ = thread.join();
        }
        for thread in self.extra_bridge_threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for MullionProcess {
    fn drop(&mut self) {
        self.cleanup_extra_windows();
        self.cleanup_sidecars();
        self.stop_desktop_event_forwarder();
        if self.primary_alive {
            let _ = self.child.terminate();
            self.primary_alive = false;
        }
        self.join_bridge_threads();
    }
}
