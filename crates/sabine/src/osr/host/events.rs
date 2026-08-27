use winit::{cursor::CursorIcon, event_loop::ActiveEventLoop};

use crate::osr::protocol::{OsrMessage, POPUP_OVERLAY_ID};

use super::native::{OsrNativeHost, present_window};
pub(super) use super::types::HostActivity;
use super::types::HostControl;

impl OsrNativeHost {
    pub(super) fn process_osr_events(&mut self, event_loop: &dyn ActiveEventLoop) {
        let mut needs_redraw = false;
        let mut needs_initial_present = false;
        let mut resize_frame_ready = false;
        while let Ok(event) = self.receiver.try_recv() {
            if event
                .connection_generation()
                .is_some_and(|generation| generation != self.connection_generation)
            {
                continue;
            }
            match event {
                super::types::OsrHostEvent::Connected(_, stream) => {
                    self.socket = Some(std::sync::Arc::new(std::sync::Mutex::new(stream)));
                    self.awaiting_connection = false;
                    let mut output = std::io::stdout();
                    use std::io::Write;
                    let _ = writeln!(output, "SABINE_OSR_READY");
                    let _ = output.flush();
                    self.handoff_deadline = None;
                    self.send_resize();
                    self.send_current_lifecycle();
                    self.send_control(if self.focused {
                        "focus\t1\n"
                    } else {
                        "focus\t0\n"
                    });
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::Frame(frame)) => {
                    if self.accepts_paint() {
                        let was_presented = self.presented;
                        let was_resize_pending = self.pending_resize_paint.is_some();
                        let updated = self.update_frame_texture(frame);
                        needs_redraw |= updated;
                        resize_frame_ready |=
                            was_resize_pending && self.pending_resize_paint.is_none();
                        needs_initial_present |= !was_presented && self.main_surface_ready();
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::PaintBatch(batch)) => {
                    if self.accepts_paint() {
                        let was_presented = self.presented;
                        let was_resize_pending = self.pending_resize_paint.is_some();
                        let updated = self.update_paint_batch(batch);
                        needs_redraw |= updated;
                        resize_frame_ready |=
                            was_resize_pending && self.pending_resize_paint.is_none();
                        needs_initial_present |= !was_presented && self.main_surface_ready();
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::AccelFrame(frame)) => {
                    if self.accepts_paint() {
                        let was_presented = self.presented;
                        let was_resize_pending = self.pending_resize_paint.is_some();
                        let updated = self.update_accel_frame(frame);
                        needs_redraw |= updated;
                        resize_frame_ready |=
                            was_resize_pending && self.pending_resize_paint.is_none();
                        needs_initial_present |= !was_presented && self.main_surface_ready();
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::PopupHidden) => {
                    self.clear_overlay(POPUP_OVERLAY_ID);
                    needs_redraw = true;
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::GuestHidden(id)) => {
                    if !id.is_empty() {
                        self.clear_overlay(&id);
                        needs_redraw = true;
                    }
                }
                super::types::OsrHostEvent::Message(
                    _,
                    OsrMessage::GuestCaptureRequested {
                        browser_id,
                        request_id,
                        guest_id,
                    },
                ) => self.capture_guest(&browser_id, &request_id, &guest_id),
                super::types::OsrHostEvent::Message(
                    _,
                    OsrMessage::DraggableRegionsChanged { drag, exclusion },
                ) => {
                    self.page_drag_regions = drag;
                    self.page_drag_exclusion_regions = exclusion;
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::Cursor(cursor)) => {
                    self.set_content_cursor(cursor_for_cef(&cursor));
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::CloseRequested) => {
                    if self.config.hide_on_close {
                        self.hide_window("close");
                    } else {
                        self.begin_close(event_loop);
                        return;
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::StartDragRequested) => {
                    let button = Some(winit::event::MouseButton::Left);
                    if self.mouse_button_pressed(button) {
                        self.set_mouse_button(button, false);
                        self.forward_mouse_click(button, true, self.active_click_count);
                    }
                    if let Some(window) = &self.window
                        && let Err(error) = window.drag_window()
                    {
                        eprintln!("failed to begin native window drag: {error}");
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::FileDragRequested(request)) => {
                    self.start_file_drag(event_loop, request);
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::MinimizeRequested) => {
                    if self.config.lifecycle.suspend_on_minimize {
                        self.suspend("minimize");
                        if self.config.lifecycle.hibernate_after.is_some() {
                            self.begin_hibernate("minimize");
                        }
                    }
                    if let Some(window) = &self.window {
                        window.set_minimized(true);
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::ToggleMaximizeRequested) => {
                    if let Some(window) = &self.window {
                        window.set_maximized(!window.is_maximized());
                    }
                }
                super::types::OsrHostEvent::Message(
                    _,
                    OsrMessage::FullscreenRequested(enabled),
                ) => {
                    if let Some(window) = &self.window {
                        window.set_fullscreen(
                            enabled.then_some(winit::monitor::Fullscreen::Borderless(None)),
                        );
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::ShowRequested) => {
                    self.ensure_window(event_loop);
                    self.show_window("show");
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::HideRequested) => {
                    self.hide_window("hide")
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::FocusRequested(token)) => {
                    self.activate_window(event_loop, token);
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::BridgeRequest(line)) => {
                    if !line.is_empty() {
                        let mut output = std::io::stdout();
                        use std::io::Write;
                        let _ = writeln!(output, "{line}");
                        let _ = output.flush();
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::MainLoadStarted) => {
                    self.main_load_ready = false;
                    if self.config.visible && self.loading.is_none() {
                        self.loading = Some(super::types::NativeLoading::new(
                            super::types::LoadingKind::Opening,
                        ));
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::MainLoadReady) => {
                    self.main_load_ready = true;
                    if self.main_frame.is_some() {
                        self.loading = None;
                        needs_redraw = true;
                        needs_initial_present |= !self.presented;
                    }
                }
                super::types::OsrHostEvent::Message(_, OsrMessage::ImeStateChanged(mode)) => {
                    self.update_ime_state(mode);
                }
                super::types::OsrHostEvent::Message(
                    _,
                    OsrMessage::ImeCursorAreaChanged {
                        x,
                        y,
                        width,
                        height,
                    },
                ) => self.update_ime_cursor_area(x, y, width, height),
                super::types::OsrHostEvent::HostControl(HostControl::Show) => {
                    self.ensure_window(event_loop);
                    self.show_window("show");
                }
                super::types::OsrHostEvent::HostControl(HostControl::Hide) => {
                    self.hide_window("hide")
                }
                super::types::OsrHostEvent::HostControl(HostControl::Focus(token)) => {
                    self.activate_window(event_loop, token);
                }
                super::types::OsrHostEvent::HostControl(HostControl::Visible(true)) => {
                    self.ensure_window(event_loop);
                    self.show_window("visible")
                }
                super::types::OsrHostEvent::HostControl(HostControl::Visible(false)) => {
                    self.hide_window("hidden")
                }
                super::types::OsrHostEvent::HostControl(HostControl::Quit) => {
                    self.begin_close(event_loop);
                    return;
                }
                super::types::OsrHostEvent::HostControl(HostControl::ActivityBegin(activity)) => {
                    self.begin_activity(activity)
                }
                super::types::OsrHostEvent::HostControl(HostControl::ActivityEnd(activity)) => {
                    self.end_activity(activity)
                }
                super::types::OsrHostEvent::ControlLine(line) => {
                    let mut line = line;
                    if !line.ends_with('\n') {
                        line.push('\n');
                    }
                    self.send_control(&line);
                }
                super::types::OsrHostEvent::Disconnected(_) => {
                    self.socket = None;
                    self.awaiting_connection = false;
                    if self.closing_deadline.is_some() {
                        continue;
                    }
                    if matches!(
                        self.lifecycle_state,
                        super::types::LifecycleState::Hibernating
                            | super::types::LifecycleState::Hibernated
                    ) {
                        self.lifecycle_state = super::types::LifecycleState::Hibernated;
                        continue;
                    }
                    self.begin_recovery();
                }
            }
        }
        if needs_initial_present {
            if self.render() {
                self.present_rendered_surface("first_paint");
            }
            return;
        }
        if self.config.visible
            && needs_redraw
            && let Some(window) = &self.window
        {
            if resize_frame_ready && self.presented {
                self.render();
            } else {
                window.request_redraw();
            }
        }
    }

    pub(super) fn show_window(&mut self, reason: &str) {
        self.config.visible = true;
        if let Some(window) = &self.window {
            if self.presented {
                window.set_visible(true);
                window.set_minimized(false);
                window.request_redraw();
            } else {
                window.set_visible(false);
            }
        }
        self.resume(reason);
        self.send_resize();
    }

    pub(super) fn hide_window(&mut self, reason: &str) {
        self.config.visible = false;
        self.focused = false;
        self.overlays.clear();
        self.send_control("focus\t0\n");
        self.unmap_window();
        self.suspend(reason);
    }

    pub(super) fn focus_window(&mut self, reason: &str) {
        self.config.visible = true;
        self.focused = true;
        if let Some(window) = &self.window {
            if self.presented {
                present_window(window);
            } else {
                window.set_visible(false);
            }
        }
        self.send_control("focus\t1\n");
        self.resume(reason);
        self.send_resize();
    }

    pub(super) fn activate_window(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        token: Option<String>,
    ) {
        if let Some(token) = activation_token_value(token) {
            self.pending_activation_token = Some(winit::window::ActivationToken::from_raw(token));
            if self.window.is_some() {
                self.drop_presented_window();
            }
        }
        self.ensure_window(event_loop);
        self.focus_window("focus");
    }

    pub(super) fn capture_guest(&self, browser_id: &str, request_id: &str, guest_id: &str) {
        let result = self
            .overlays
            .get(guest_id)
            .ok_or_else(|| "guest has no frame to capture".to_string())
            .and_then(|overlay| {
                super::guest_preview::guest_preview_data_url(
                    overlay.buffer.bytes(),
                    overlay.frame.width,
                    overlay.frame.height,
                )
            });
        let (status, payload) = match result {
            Ok(data_url) => ("ok", serde_json::json!({ "dataUrl": data_url })),
            Err(message) => ("error", serde_json::json!({ "message": message })),
        };
        self.send_control(&format!(
            "SABINE_BRIDGE_RESPONSE\t{browser_id}\t{request_id}\t{status}\t{payload}\n"
        ));
    }
}

pub(super) fn host_control_from_parts(command: &str, value: &str) -> Option<HostControl> {
    match command {
        "visible" => bool_control_value(value).map(HostControl::Visible),
        "show" => Some(HostControl::Show),
        "hide" => Some(HostControl::Hide),
        "quit" => Some(HostControl::Quit),
        "focus" => Some(HostControl::Focus(activation_token_value(Some(
            value.to_string(),
        )))),
        "activity.begin" => activity_control_value(value).map(HostControl::ActivityBegin),
        "activity.end" => activity_control_value(value).map(HostControl::ActivityEnd),
        _ => None,
    }
}

fn bool_control_value(value: &str) -> Option<bool> {
    match value {
        "1" | "true" | "yes" | "show" | "visible" => Some(true),
        "0" | "false" | "no" | "hide" | "hidden" => Some(false),
        _ => None,
    }
}

fn activation_token_value(token: Option<String>) -> Option<String> {
    token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .filter(|token| bool_control_value(token).is_none())
}

fn activity_control_value(value: &str) -> Option<HostActivity> {
    let value = serde_json::from_str::<serde_json::Value>(value).ok()?;
    Some(HostActivity {
        id: value.get("id")?.as_str()?.to_string(),
        prevents_hibernation: value
            .get("preventsHibernation")
            .or_else(|| value.get("prevents_hibernation"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    })
}

fn cursor_for_cef(cursor: &str) -> CursorIcon {
    match cursor {
        "pointer" | "hand" => CursorIcon::Pointer,
        "text" | "vertical-text" => CursorIcon::Text,
        "crosshair" => CursorIcon::Crosshair,
        "move" => CursorIcon::Move,
        "wait" => CursorIcon::Wait,
        "help" => CursorIcon::Help,
        "not-allowed" => CursorIcon::NotAllowed,
        "col-resize" | "ew-resize" => CursorIcon::EwResize,
        "row-resize" | "ns-resize" => CursorIcon::NsResize,
        "ne-resize" => CursorIcon::NeResize,
        "nw-resize" => CursorIcon::NwResize,
        "se-resize" => CursorIcon::SeResize,
        "sw-resize" => CursorIcon::SwResize,
        _ => CursorIcon::Default,
    }
}
