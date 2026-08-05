use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use winit::{
    application::ApplicationHandler,
    data_transfer::{DataTransferSendBuilder, SendData, TypeHint},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, DndAction},
    keyboard::Key,
    window::WindowId,
};

use crate::osr::protocol::{FileDragRequest, encode_component};

use super::chrome::{activate_control, resize_direction_at};
use super::native::OsrNativeHost;
use super::types::{
    ClickMemory, EVENTFLAG_ALT_DOWN, EVENTFLAG_COMMAND_DOWN, EVENTFLAG_CONTROL_DOWN,
    EVENTFLAG_IS_REPEAT, EVENTFLAG_LEFT_MOUSE_BUTTON, EVENTFLAG_MIDDLE_MOUSE_BUTTON,
    EVENTFLAG_PRECISION_SCROLLING_DELTA, EVENTFLAG_RIGHT_MOUSE_BUTTON, EVENTFLAG_SHIFT_DOWN,
    LifecycleState,
};
use winit::cursor::CursorIcon;

impl OsrNativeHost {
    pub(super) fn start_file_drag(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        request: FileDragRequest,
    ) {
        let FileDragRequest { paths, x: _, y: _ } = request;
        let Some(window) = self.window.clone() else {
            self.finish_file_drag(None);
            return;
        };

        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|path| PathBuf::from(path.trim()))
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
        if paths.is_empty() {
            self.finish_file_drag(None);
            return;
        }

        let transfer = DataTransferSendBuilder::new(paths)
            .with_type(TypeHint::UriList, |paths, _| {
                SendData::from_file_paths(paths.iter())
            })
            .build();

        match event_loop.start_drag(
            window.id(),
            transfer,
            &[DndAction::Copy, DndAction::Move],
            None,
        ) {
            Ok(id) => {
                self.active_file_drag = Some(id);
            }
            Err(error) => {
                eprintln!("failed to start native file drag: {error}");
                self.finish_file_drag(None);
            }
        }
    }

    pub(super) fn finish_file_drag(&mut self, action: Option<DndAction>) {
        self.active_file_drag = None;
        let operation = match action {
            Some(DndAction::Copy) => "copy",
            Some(DndAction::Move) => "move",
            Some(DndAction::Link) => "link",
            _ => "none",
        };
        let (x, y) = self
            .content_position(self.cursor_x, self.cursor_y)
            .unwrap_or((self.cursor_x, self.cursor_y));
        self.send_control(&format!(
            "file_drag_ended\t{:.0}\t{:.0}\t{operation}\n",
            x, y
        ));
    }

    pub(super) fn forward_mouse_move(&self, leave: bool) {
        if let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) {
            self.send_control(&format!(
                "mouse_move\t{:.2}\t{:.2}\t{}\t{}\n",
                x,
                y,
                self.input_modifiers(),
                i32::from(leave)
            ));
        }
    }

    pub(super) fn forward_mouse_click(
        &self,
        button: Option<MouseButton>,
        up: bool,
        click_count: i32,
    ) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let Some(button) = cef_mouse_button(button) else {
            return;
        };
        self.send_control(&format!(
            "mouse_click\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}\n",
            x,
            y,
            button,
            self.input_modifiers(),
            i32::from(up),
            click_count.max(1)
        ));
    }

    pub(super) fn forward_navigation_button(&self, button: Option<MouseButton>) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let button = match button {
            Some(MouseButton::Back) => 3,
            Some(MouseButton::Forward) => 4,
            _ => return,
        };
        self.send_control(&format!(
            "mouse_navigation\t{:.2}\t{:.2}\t{}\t{}\n",
            x,
            y,
            button,
            self.input_modifiers()
        ));
    }

    pub(super) fn forward_mouse_wheel(&self, delta: MouseScrollDelta) {
        let Some((x, y)) = self.content_position(self.cursor_x, self.cursor_y) else {
            return;
        };
        let (dx, dy, precision) = match delta {
            MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32, false),
            MouseScrollDelta::PixelDelta(position) => (position.x as i32, position.y as i32, true),
            _ => return,
        };
        self.send_control(&format!(
            "mouse_wheel\t{:.2}\t{:.2}\t{}\t{}\t{}\n",
            x,
            y,
            dx,
            dy,
            self.input_modifiers()
                | if precision {
                    EVENTFLAG_PRECISION_SCROLLING_DELTA
                } else {
                    0
                }
        ));
    }

    pub(super) fn send_key_event(&self, event: &KeyEvent) {
        let pressed = event.state == ElementState::Pressed;
        let text = if pressed {
            event
                .text
                .as_deref()
                .filter(|text| should_send_char_text(text))
                .unwrap_or("")
        } else {
            ""
        };
        self.send_control(&format!(
            "key\t{}\t{}\t{}\t{}\t{}\n",
            i32::from(pressed),
            encode_component(&key_name(event)),
            encode_component(text),
            self.input_modifiers() | if event.repeat { EVENTFLAG_IS_REPEAT } else { 0 },
            i32::from(event.repeat)
        ));
    }

    pub(super) fn input_modifiers(&self) -> u32 {
        let mut modifiers = 0;
        if self.modifiers.shift_key() {
            modifiers |= EVENTFLAG_SHIFT_DOWN;
        }
        if self.modifiers.control_key() {
            modifiers |= EVENTFLAG_CONTROL_DOWN;
        }
        if self.modifiers.alt_key() {
            modifiers |= EVENTFLAG_ALT_DOWN;
        }
        if self.modifiers.meta_key() {
            modifiers |= EVENTFLAG_COMMAND_DOWN;
        }
        if self.mouse.left {
            modifiers |= EVENTFLAG_LEFT_MOUSE_BUTTON;
        }
        if self.mouse.middle {
            modifiers |= EVENTFLAG_MIDDLE_MOUSE_BUTTON;
        }
        if self.mouse.right {
            modifiers |= EVENTFLAG_RIGHT_MOUSE_BUTTON;
        }
        modifiers
    }

    pub(super) fn set_mouse_button(&mut self, button: Option<MouseButton>, pressed: bool) {
        match button {
            Some(MouseButton::Left) => self.mouse.left = pressed,
            Some(MouseButton::Middle) => self.mouse.middle = pressed,
            Some(MouseButton::Right) => self.mouse.right = pressed,
            _ => {}
        }
    }

    pub(super) fn next_click_count(&mut self, button: Option<MouseButton>) -> i32 {
        let Some(button) = button else {
            return 1;
        };
        let now = Instant::now();
        let count = self
            .last_click
            .filter(|last| {
                last.button == button
                    && now.duration_since(last.at) <= Duration::from_millis(500)
                    && (last.x - self.cursor_x).abs() <= 4.0
                    && (last.y - self.cursor_y).abs() <= 4.0
            })
            .map(|last| (last.count + 1).min(3))
            .unwrap_or(1);
        self.last_click = Some(ClickMemory {
            button,
            x: self.cursor_x,
            y: self.cursor_y,
            at: now,
            count,
        });
        count
    }
}

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
                self.surface_size = size;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height, window.scale_factor() as f32);
                }
                self.update_effect_regions();
                self.queue_resize_paint();
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = window.surface_size();
                self.surface_size = size;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height, scale_factor as f32);
                }
                self.update_effect_regions();
                self.queue_resize_paint();
                window.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.send_control(if focused { "focus\t1\n" } else { "focus\t0\n" });
                if !focused && self.config.hide_on_blur && self.config.visible {
                    self.hide_window("blur");
                } else {
                    self.sync_lifecycle(if focused { "focus" } else { "blur" });
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.sync_lifecycle(if occluded { "occluded" } else { "visible" });
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
        if let Some(child) = self.child.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            self.child = None;
            if status.code() == Some(CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED) {
                // Another mullion-host already holds this profile. CEF handed our
                // launch args to that process, which creates a browser on our OSR
                // endpoint — keep the native window and socket listener alive.
                self.cef_handed_off = true;
                super::trace_host(&self.config, "cef.handed_off.waiting_for_primary");
                return;
            }
            eprintln!("Mullion OSR host: CEF child exited ({status}); shutting down host");
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
            if self.started.elapsed() > Duration::from_secs(10) {
                eprintln!(
                    "Mullion OSR host: profile handoff succeeded but primary CEF never connected"
                );
                event_loop.exit();
            }
            return;
        }
        if self.started.elapsed() > Duration::from_secs(2)
            && self.child.is_none()
            && !self.cef_handed_off
            && self.lifecycle_state != LifecycleState::Hibernated
        {
            eprintln!("Mullion OSR host: no CEF child after 2s; shutting down host");
            event_loop.exit();
        }
    }
}

/// CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED — second process for the same
/// root_cache_path notified the primary and exited.
const CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED: i32 = 24;

fn cef_mouse_button(button: Option<MouseButton>) -> Option<&'static str> {
    match button {
        Some(MouseButton::Left) => Some("left"),
        Some(MouseButton::Middle) => Some("middle"),
        Some(MouseButton::Right) => Some("right"),
        _ => None,
    }
}

fn key_name(event: &KeyEvent) -> String {
    match event.logical_key.as_ref() {
        Key::Character(value) if !value.is_empty() => value.to_string(),
        Key::Named(named) => named.to_string(),
        _ => match &event.physical_key {
            winit::keyboard::PhysicalKey::Code(code) => format!("{code:?}"),
            _ => "Unidentified".to_string(),
        },
    }
}

fn should_send_char_text(text: &str) -> bool {
    !text.chars().any(char::is_control)
}
