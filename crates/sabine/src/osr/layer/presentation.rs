use layershellev::WindowState;

use super::shell::{anchor_for_shell, keyboard_for_shell, layer_for_shell};
use super::types::OsrLayerHost;

impl OsrLayerHost {
    pub(super) fn complete_remap_sync(&mut self, token: u64, state: &mut WindowState<()>) {
        if !self.surface_lifecycle.complete_sync(token) || !self.visible {
            return;
        }
        self.request_layer_configure(state);
    }

    pub(super) fn request_layer_configure(&mut self, state: &mut WindowState<()>) {
        self.layer_layout_dirty = true;
        if !self.visible || !self.surface_lifecycle.presentation_ready() {
            return;
        }
        let Some(shell_surface) = self.config.shell_surface.clone() else {
            self.layer_layout_dirty = false;
            return;
        };

        self.surface_lifecycle
            .wait_for_configure(self.configure_generation);
        self.layer_layout_dirty = false;
        self.presentation_dirty = false;
        self.apply_alpha(state);

        let main_id = state.main_window().id();
        let (width, height) = self.layer_commit_size();
        let unit = state
            .get_mut_unit_with_id(main_id)
            .expect("main layer surface must exist");
        unit.set_layout(
            anchor_for_shell(shell_surface.anchor),
            super::layer_size_for_shell((width, height)),
        );
        unit.set_layer(layer_for_shell(shell_surface.layer));
        unit.set_exclusive_zone(shell_surface.exclusive_zone.unwrap_or_default());
        unit.set_keyboard_interactivity(keyboard_for_shell(shell_surface.keyboard_interactivity));
        unit.set_margin((
            shell_surface.margin.top,
            shell_surface.margin.right,
            shell_surface.margin.bottom,
            shell_surface.margin.left,
        ));
        if !super::surface::flush_surface(unit.get_wlsurface()) {
            self.wayland_failed = true;
        }
    }

    pub(super) fn apply_pending_presentation(&mut self, state: &mut WindowState<()>) {
        if !self.presentation_dirty || !self.visible || !self.surface_lifecycle.presentation_ready()
        {
            return;
        }
        let Some(shell_surface) = self.config.shell_surface.clone() else {
            self.presentation_dirty = false;
            return;
        };

        self.apply_alpha(state);
        let main_id = state.main_window().id();
        let unit = state
            .get_mut_unit_with_id(main_id)
            .expect("main layer surface must exist");
        unit.set_margin((
            shell_surface.margin.top,
            shell_surface.margin.right,
            shell_surface.margin.bottom,
            shell_surface.margin.left,
        ));
        if !super::surface::flush_surface(unit.get_wlsurface()) {
            self.wayland_failed = true;
            return;
        }
        self.presentation_dirty = false;
    }

    fn apply_alpha(&mut self, state: &WindowState<()>) {
        if self.alpha_modifier.is_none()
            && let Some(manager_name) = self.alpha_manager_name
        {
            self.alpha_modifier = super::alpha::LayerAlphaModifier::bind(state, manager_name);
        }
        if let Some(modifier) = &self.alpha_modifier {
            let _ = modifier.set_alpha(self.surface_alpha);
        }
    }

    fn layer_commit_size(&self) -> (u32, u32) {
        if let Some(shell_surface) = &self.config.shell_surface
            && let Some((width, height)) = shell_surface.size
        {
            let width = if width == 0 && shell_surface.anchor.left && shell_surface.anchor.right {
                0
            } else {
                width.max(1)
            };
            return (width, height.max(1));
        }
        (self.surface_size.0.max(1), self.surface_size.1.max(1))
    }
}
