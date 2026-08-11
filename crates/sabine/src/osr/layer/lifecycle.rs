use super::shell::keyboard_for_shell;
use super::types::{LayerLifecycleState, OsrLayerHost};
use layershellev::WindowState;

impl OsrLayerHost {
    pub(super) fn send_control(&self, line: &str) {
        let Some(writer) = &self.control_writer else {
            return;
        };
        writer.send(line.to_string());
    }

    pub(super) fn send_mouse_motion(&self, line: String) {
        let Some(writer) = &self.control_writer else {
            return;
        };
        writer.send_motion(line);
    }

    pub(super) fn send_lifecycle(&self, state: LayerLifecycleState, reason: &str) {
        let (name, frame_rate) = match state {
            LayerLifecycleState::Active => ("active", self.active_frame_rate()),
            LayerLifecycleState::Suspended => (
                "suspended",
                self.config.lifecycle.background_frame_rate.max(1),
            ),
        };
        self.send_control(&format!(
            "lifecycle\t{name}\t{frame_rate}\t{}\n",
            crate::osr::protocol::encode_component(reason)
        ));
    }

    pub(super) fn active_frame_rate(&self) -> u32 {
        if self.config.lifecycle.active_frame_rate > 0 {
            self.config.lifecycle.active_frame_rate
        } else {
            60
        }
    }

    pub(super) fn content_size_for_cef(&self) -> (u32, u32, f64) {
        (
            self.surface_size.0.max(1),
            self.surface_size.1.max(1),
            self.scale.max(1.0),
        )
    }

    pub(super) fn send_resize(&self) {
        let (width, height, scale) = self.content_size_for_cef();
        self.send_control(&format!("resize\t{width}\t{height}\t{scale:.4}\n"));
    }

    pub(super) fn suspend(&mut self, reason: &str) {
        if self.lifecycle_state == LayerLifecycleState::Suspended {
            return;
        }
        self.force_suspend(reason);
    }

    pub(super) fn resume(&mut self, reason: &str) {
        if self.lifecycle_state == LayerLifecycleState::Active {
            return;
        }
        self.force_resume(reason);
    }

    pub(super) fn force_suspend(&mut self, reason: &str) {
        self.lifecycle_state = LayerLifecycleState::Suspended;
        self.send_lifecycle(LayerLifecycleState::Suspended, reason);
    }

    pub(super) fn force_resume(&mut self, reason: &str) {
        self.lifecycle_state = LayerLifecycleState::Active;
        self.send_lifecycle(LayerLifecycleState::Active, reason);
    }

    pub(super) fn force_current_lifecycle(&mut self, reason: &str) {
        match self.lifecycle_state {
            LayerLifecycleState::Active => self.force_resume(reason),
            LayerLifecycleState::Suspended => self.force_suspend(reason),
        }
    }

    pub(super) fn set_surface_visible(&mut self, visible: bool, state: &mut WindowState<()>) {
        self.visible = visible;
        if visible {
            self.show_surface(state);
        } else {
            self.hide_shell_surface(state);
        }
    }

    pub(super) fn show_surface(&mut self, state: &mut WindowState<()>) {
        self.restore_keyboard(state);
        self.force_resume("visible");
        self.send_resize();
        if self.pointer_inside {
            self.forward_mouse_move(false);
        }
        if self.main_frame_ready() {
            self.refresh_surface(state, None);
        } else {
            self.hide_surface(state);
        }
    }

    pub(super) fn hide_shell_surface(&mut self, state: &mut WindowState<()>) {
        self.close_popup(state);
        if self.pointer_inside {
            self.forward_mouse_move(true);
            self.pointer_inside = false;
        }
        self.send_control("focus\t0\n");
        self.force_suspend("hidden");
        self.send_resize();
        self.set_surface_alpha(0.0, state);
        self.hide_surface(state);
        if !self.config.lifecycle.retain_hidden_frame {
            self.release_hidden_frame_memory();
        }
    }

    pub(super) fn set_surface_alpha(&mut self, alpha: f32, state: &WindowState<()>) {
        let alpha = alpha.clamp(0.0, 1.0);
        if self.alpha_modifier.is_some() && (self.surface_alpha - alpha).abs() <= 0.001 {
            return;
        }
        if self.alpha_modifier.is_none() {
            self.alpha_modifier = super::alpha::LayerAlphaModifier::bind(state);
        }
        self.surface_alpha = alpha;
        if let Some(modifier) = &self.alpha_modifier {
            let _ = modifier.set_alpha(alpha);
        }
    }

    pub(super) fn set_surface_margin(
        &mut self,
        margin: sabine_platform::ShellSurfaceMargin,
        state: &WindowState<()>,
    ) {
        let Some(shell_surface) = self.config.shell_surface.as_mut() else {
            return;
        };
        if shell_surface.margin == margin {
            return;
        }
        shell_surface.margin = margin;
        state
            .main_window()
            .set_margin((margin.top, margin.right, margin.bottom, margin.left));
    }

    pub(super) fn restore_keyboard(&self, state: &mut WindowState<()>) {
        let Some(shell_surface) = self.config.shell_surface.as_ref() else {
            return;
        };
        state
            .main_window()
            .set_keyboard_interactivity(keyboard_for_shell(shell_surface.keyboard_interactivity));
    }

    pub(super) fn begin_close(&mut self) {
        self.send_control("close\n");
        // Detach CEF — shared process-singleton windows must survive sibling close.
        if let Some(mut child) = self.child.take() {
            let _ = child.try_wait();
        }
    }
}
