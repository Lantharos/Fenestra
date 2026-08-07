mod forward;
mod ime;

use std::time::{Duration, Instant};

use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    window::WindowId,
};

use crate::osr::host::chrome::{activate_control, resize_direction_at};
use crate::osr::host::native::OsrNativeHost;
use crate::osr::host::types::LifecycleState;
use winit::cursor::CursorIcon;

impl ApplicationHandler for OsrNativeHost {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if !self.config.visible {
            self.launch_child();
            return;
        }
        self.create_window(event_loop);
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.process_osr_events(event_loop);
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if id != window.id() {
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.begin_close(event_loop),
            WindowEvent::Destroyed if self.config.visible || self.closing_deadline.is_some() => {
                self.begin_close(event_loop)
            }
            WindowEvent::Destroyed => self.drop_hidden_window(),
            WindowEvent::SurfaceResized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                let scale = window.scale_factor();
                // Wayland emits a configure after interactive move even when the
                // size did not change. Reconfiguring wgpu / Invalidating CEF
                // flashes — especially noticeable on the handed-off second window.
                if size == self.surface_size && (scale - self.scale_factor).abs() < f64::EPSILON {
                    return;
                }
                self.surface_size = size;
                self.scale_factor = scale;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height, scale as f32);
                }
                // Defer glass region commit until after a successful present —
                // committing without a buffer flashes transparent windows.
                self.effect_regions_dirty = true;
                self.queue_resize_paint();
                // Fill the new swapchain immediately — configure leaves empty
                // buffers until the next redraw, which flashes transparent glass.
                if self.presented {
                    self.render();
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = window.surface_size();
                self.surface_size = size;
                self.scale_factor = scale_factor;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height, scale_factor as f32);
                }
                self.effect_regions_dirty = true;
                self.queue_resize_paint();
                if self.presented {
                    self.render();
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.send_control(if focused { "focus\t1\n" } else { "focus\t0\n" });
                if !focused && self.config.hide_on_blur && self.config.visible {
                    self.hide_window("blur");
                } else {
                    self.schedule_lifecycle_sync(if focused { "focus" } else { "blur" });
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.schedule_lifecycle_sync(if occluded { "occluded" } else { "visible" });
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                self.send_key_event(&event);
            }
            WindowEvent::Ime(ime) => self.forward_ime(ime),
            WindowEvent::Moved(_) => self.send_screen_origin(),
            WindowEvent::RedrawRequested if self.config.visible && self.presented => self.render(),
            WindowEvent::RedrawRequested => {}
            WindowEvent::PointerMoved {
                position, primary, ..
            } if primary => {
                let scale = window.scale_factor() as f32;
                self.cursor_x = position.x as f32 / scale.max(1.0);
                self.cursor_y = position.y as f32 / scale.max(1.0);
                self.update_titlebar_hover();
                if self.config.resizable
                    && let Some(direction) = resize_direction_at(
                        self.cursor_x,
                        self.cursor_y,
                        self.logical_width(),
                        self.logical_height(),
                    )
                {
                    self.set_native_cursor(CursorIcon::from(direction));
                } else if self.hovered_control.is_some() {
                    self.set_native_cursor(CursorIcon::Pointer);
                    self.forward_mouse_move(false);
                } else if self
                    .content_position(self.cursor_x, self.cursor_y)
                    .is_some()
                {
                    self.clear_native_cursor();
                    self.forward_mouse_move(false);
                } else {
                    self.set_native_cursor(CursorIcon::Default);
                }
                window.request_redraw();
            }
            WindowEvent::PointerLeft {
                position, primary, ..
            } if primary => {
                if let Some(position) = position {
                    let scale = window.scale_factor() as f32;
                    self.cursor_x = position.x as f32 / scale.max(1.0);
                    self.cursor_y = position.y as f32 / scale.max(1.0);
                }
                self.hovered_control = None;
                self.forward_mouse_move(true);
                self.set_native_cursor(CursorIcon::Default);
                window.request_redraw();
            }
            WindowEvent::PointerButton {
                state,
                primary,
                position,
                button,
                ..
            } if primary => {
                let scale = window.scale_factor() as f32;
                self.cursor_x = position.x as f32 / scale.max(1.0);
                self.cursor_y = position.y as f32 / scale.max(1.0);
                let button = button.clone().mouse_button();
                match state {
                    ElementState::Pressed => {
                        if matches!(button, Some(MouseButton::Back | MouseButton::Forward)) {
                            return;
                        }
                        if self.config.resizable
                            && let Some(direction) = resize_direction_at(
                                self.cursor_x,
                                self.cursor_y,
                                self.logical_width(),
                                self.logical_height(),
                            )
                        {
                            if let Err(error) = window.drag_resize_window(direction) {
                                eprintln!("failed to begin native window resize: {error}");
                            }
                            return;
                        }
                        let width = self.logical_width();
                        if let Some(control) = self.control_at(width, self.cursor_x, self.cursor_y)
                        {
                            self.pressed_control = Some(control);
                            window.request_redraw();
                            return;
                        }
                        if self.is_drag_region(width, self.cursor_x, self.cursor_y) {
                            if let Err(error) = window.drag_window() {
                                eprintln!("failed to begin native window drag: {error}");
                            }
                            return;
                        }
                        self.active_click_count = self.next_click_count(button);
                        self.set_mouse_button(button, true);
                        self.forward_mouse_click(button, false, self.active_click_count);
                    }
                    ElementState::Released => {
                        if let Some(pressed) = self.pressed_control.take() {
                            let released =
                                self.control_at(self.logical_width(), self.cursor_x, self.cursor_y);
                            if released == Some(pressed) {
                                activate_control(self, event_loop, &window, pressed);
                            }
                            window.request_redraw();
                            return;
                        }
                        if matches!(button, Some(MouseButton::Back | MouseButton::Forward)) {
                            self.forward_navigation_button(button);
                            return;
                        }
                        self.set_mouse_button(button, false);
                        self.forward_mouse_click(button, true, self.active_click_count);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.forward_mouse_wheel(delta);
            }
            WindowEvent::OutgoingDragDropped { id, action } => {
                if self.active_file_drag == Some(id) {
                    self.finish_file_drag(action);
                }
            }
            WindowEvent::OutgoingDragCanceled { id } => {
                if self.active_file_drag == Some(id) {
                    self.finish_file_drag(None);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        if !self.config.software_osr_fallback
            && crate::osr::accel::should_relaunch_software(&self.accel_fallback)
        {
            self.relaunch_software_osr();
            return;
        }
        if let Some(child) = self.child.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            self.child = None;
            if status.code() == Some(CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED) {
                // Another sabine-host already holds this profile. CEF handed our
                // launch args to that process, which creates a browser on our OSR
                // endpoint — keep the native window and socket listener alive.
                self.cef_handed_off = true;
                self.handoff_deadline =
                    Some(Instant::now() + Duration::from_secs(HANDOFF_CONNECT_TIMEOUT_SECS));
                super::trace_host(&self.config, "cef.handed_off.waiting_for_primary");
                if let Some(deadline) = self.handoff_deadline {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
                }
                return;
            }
            eprintln!("Sabine OSR host: CEF child exited ({status}); shutting down host");
            self.socket = None;
            if matches!(
                self.lifecycle_state,
                LifecycleState::Hibernating | LifecycleState::Hibernated
            ) {
                self.lifecycle_state = LifecycleState::Hibernated;
                return;
            }
            event_loop.exit();
            return;
        }
        if let Some(deadline) = self.closing_deadline {
            if Instant::now() >= deadline {
                self.force_close(event_loop);
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if self.drive_pending_suspend(event_loop) {
            return;
        }
        if let Some(deadline) = self.hibernate_commit_deadline {
            if Instant::now() >= deadline {
                self.commit_hibernate();
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if self.drive_resize_paint(event_loop) {
            return;
        }
        if let Some(deadline) = self.hibernate_deadline {
            if self.has_hibernation_blockers() {
                self.hibernate_deadline = None;
                return;
            }
            if Instant::now() >= deadline {
                self.begin_hibernate("idle");
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if self.cef_handed_off && self.socket.is_none() {
            let deadline = *self.handoff_deadline.get_or_insert_with(|| {
                Instant::now() + Duration::from_secs(HANDOFF_CONNECT_TIMEOUT_SECS)
            });
            if Instant::now() >= deadline {
                eprintln!(
                    "Sabine OSR host: profile handoff succeeded but primary CEF never connected within {HANDOFF_CONNECT_TIMEOUT_SECS}s"
                );
                event_loop.exit();
                return;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(250),
            ));
            return;
        }
        if self.cef_handed_off && self.socket.is_some() {
            self.handoff_deadline = None;
        }
        if let Some(deadline) = self.accel_fallback.paint_watch_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            return;
        }
        if self.started.elapsed() > Duration::from_secs(2)
            && self.child.is_none()
            && !self.cef_handed_off
            && self.lifecycle_state != LifecycleState::Hibernated
        {
            eprintln!("Sabine OSR host: no CEF child after 2s; shutting down host");
            event_loop.exit();
        }
    }
}

/// CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED — second process for the same
/// root_cache_path notified the primary and exited.
const CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED: i32 = 24;
const HANDOFF_CONNECT_TIMEOUT_SECS: u64 = 15;
