use std::time::Instant;

use winit::event_loop::{ActiveEventLoop, ControlFlow};

use super::native::OsrNativeHost;
use super::types::LifecycleState;

impl OsrNativeHost {
    pub(super) fn drive_resize_paint(&mut self, event_loop: &dyn ActiveEventLoop) -> bool {
        let Some(pending) = self.pending_resize_paint else {
            return false;
        };
        if !self.config.visible
            || self.lifecycle_state != LifecycleState::Active
            || self.window.is_none()
        {
            self.pending_resize_paint = None;
            return false;
        }
        if self.main_frame_matches(pending.size) {
            self.pending_resize_paint = None;
            return false;
        }
        let now = Instant::now();
        if now >= pending.deadline {
            self.pending_resize_paint = None;
            if self.presented {
                self.render();
            }
            return false;
        }
        if now >= pending.retry_at {
            self.retry_resize_paint();
        }
        if let Some(pending) = self.pending_resize_paint {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                pending.retry_at.min(pending.deadline),
            ));
            return true;
        }
        false
    }
}
