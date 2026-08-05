use std::time::Instant;

use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::monitor::MonitorHandle;

use crate::osr::protocol::encode_component;

use super::native::OsrNativeHost;
use super::types::{
    FALLBACK_ACTIVE_FRAME_RATE, HostActivity, LIFECYCLE_SUSPEND_DEBOUNCE, LifecycleState,
};

impl OsrNativeHost {
    pub(super) fn send_lifecycle(&self, state: LifecycleState, reason: &str) {
        let (name, frame_rate) = match state {
            LifecycleState::Active => ("active", self.active_frame_rate()),
            LifecycleState::Suspended => (
                "suspended",
                self.config.lifecycle.background_frame_rate.max(1),
            ),
            LifecycleState::Hibernating | LifecycleState::Hibernated => (
                "hibernate",
                self.config.lifecycle.background_frame_rate.max(1),
            ),
        };
        self.send_control(&format!(
            "lifecycle\t{name}\t{frame_rate}\t{}\n",
            encode_component(reason)
        ));
        super::trace_host(
            &self.config,
            format!("lifecycle.{name}.{reason}.fps.{frame_rate}"),
        );
    }

    pub(super) fn active_frame_rate(&self) -> u32 {
        if self.config.lifecycle.active_frame_rate > 0 {
            return self.config.lifecycle.active_frame_rate;
        }
        self.window
            .as_ref()
            .and_then(|window| window.current_monitor())
            .and_then(monitor_frame_rate)
            .unwrap_or(FALLBACK_ACTIVE_FRAME_RATE)
    }

    fn should_suspend(&self) -> bool {
        (self.occluded && self.config.lifecycle.suspend_on_occluded)
            || (!self.focused && self.config.lifecycle.suspend_on_blur)
    }

    pub(super) fn sync_lifecycle(&mut self, reason: &str) {
        if self.closing_deadline.is_some() {
            return;
        }
        if self.should_suspend() {
            self.suspend(reason);
        } else {
            self.resume(reason);
        }
    }

    pub(super) fn schedule_lifecycle_sync(&mut self, reason: &str) {
        if self.closing_deadline.is_some() {
            return;
        }
        if self.should_suspend() {
            // Debounce blur/occlusion — interactive move briefly unfocuses the
            // secondary window and suspending immediately causes a drag-end flash.
            self.pending_suspend_at = Some(Instant::now() + LIFECYCLE_SUSPEND_DEBOUNCE);
            return;
        }
        self.pending_suspend_at = None;
        self.sync_lifecycle(reason);
    }

    pub(super) fn drive_pending_suspend(&mut self, event_loop: &dyn ActiveEventLoop) -> bool {
        let Some(deadline) = self.pending_suspend_at else {
            return false;
        };
        if !self.should_suspend() {
            self.pending_suspend_at = None;
            return false;
        }
        if Instant::now() >= deadline {
            self.pending_suspend_at = None;
            self.suspend("debounced");
            return false;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        true
    }

    pub(super) fn suspend(&mut self, reason: &str) {
        self.pending_suspend_at = None;
        if matches!(
            self.lifecycle_state,
            LifecycleState::Suspended | LifecycleState::Hibernating | LifecycleState::Hibernated
        ) {
            return;
        }
        self.lifecycle_state = LifecycleState::Suspended;
        self.hibernate_commit_deadline = None;
        self.schedule_hibernate_deadline();
        self.send_lifecycle(LifecycleState::Suspended, reason);
    }

    pub(super) fn resume(&mut self, reason: &str) {
        self.pending_suspend_at = None;
        if self.lifecycle_state == LifecycleState::Active {
            return;
        }
        self.lifecycle_state = LifecycleState::Active;
        self.hibernate_deadline = None;
        self.hibernate_commit_deadline = None;
        if self.child.is_none() {
            self.launch_child();
        }
        self.send_lifecycle(LifecycleState::Active, reason);
        // Avoid redraw-on-focus after interactive move: Wayland often marks the
        // surface outdated and a bare redraw flashes transparent glass.
        if self.config.visible && self.main_frame.is_none() {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    pub(super) fn begin_hibernate(&mut self, reason: &str) {
        if self.lifecycle_state != LifecycleState::Suspended
            || self.child.is_none()
            || self.has_hibernation_blockers()
        {
            return;
        }
        self.lifecycle_state = LifecycleState::Hibernating;
        self.hibernate_deadline = None;
        self.hibernate_commit_deadline =
            Some(Instant::now() + self.config.lifecycle.hibernate_grace);
        self.send_lifecycle(LifecycleState::Hibernating, reason);
    }

    pub(super) fn commit_hibernate(&mut self) {
        if !matches!(self.lifecycle_state, LifecycleState::Hibernating) {
            return;
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.socket = None;
        self.main_frame = None;
        self.overlays.clear();
        self.main_buffer.release();
        self.hibernate_commit_deadline = None;
        self.lifecycle_state = LifecycleState::Hibernated;
        if self.config.visible
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    pub(super) fn send_current_lifecycle(&self) {
        match self.lifecycle_state {
            LifecycleState::Active => self.send_lifecycle(LifecycleState::Active, "connect"),
            LifecycleState::Suspended => self.send_lifecycle(LifecycleState::Suspended, "connect"),
            LifecycleState::Hibernating | LifecycleState::Hibernated => {}
        }
    }

    pub(super) fn begin_activity(&mut self, activity: HostActivity) {
        if !activity.prevents_hibernation {
            return;
        }
        self.activity_hibernation_blockers.insert(activity.id);
        self.hibernate_deadline = None;
        if self.lifecycle_state == LifecycleState::Hibernating {
            self.lifecycle_state = LifecycleState::Suspended;
            self.hibernate_commit_deadline = None;
            self.send_lifecycle(LifecycleState::Suspended, "activity");
        }
    }

    pub(super) fn end_activity(&mut self, activity: HostActivity) {
        if !activity.prevents_hibernation {
            return;
        }
        self.activity_hibernation_blockers.remove(&activity.id);
        if self.lifecycle_state == LifecycleState::Suspended {
            self.schedule_hibernate_deadline();
        }
    }

    pub(super) fn has_hibernation_blockers(&self) -> bool {
        !self.activity_hibernation_blockers.is_empty()
    }

    pub(super) fn schedule_hibernate_deadline(&mut self) {
        self.hibernate_deadline = if self.has_hibernation_blockers() {
            None
        } else {
            self.config
                .lifecycle
                .hibernate_after
                .map(|delay| Instant::now() + delay)
        };
    }
}

fn monitor_frame_rate(monitor: MonitorHandle) -> Option<u32> {
    monitor
        .video_modes()
        .filter_map(|mode| {
            mode.refresh_rate_millihertz()
                .map(|millihertz| millihertz.get())
        })
        .max()
        .map(|millihertz| millihertz.saturating_add(999) / 1000)
        .filter(|rate| *rate > 0)
}
