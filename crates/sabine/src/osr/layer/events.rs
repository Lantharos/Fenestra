use layershellev::{DispatchMessage, LayerShellEvent, ReturnData, WindowState, id};

use crate::osr::host::guest_preview_data_url;
use crate::osr::protocol::{OsrMessage, OsrSurface};

use super::buffer::DamageRect;
use super::input::{axis_delta, cursor_shape_for_wayland};
use super::socket::LayerHostEvent;
use super::types::OsrLayerHost;

impl OsrLayerHost {
    pub(super) fn handle(
        &mut self,
        event: LayerShellEvent<(), LayerHostEvent>,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> ReturnData<()> {
        match event {
            LayerShellEvent::RequestBuffer(file, shm, qh, width, height) => {
                let width = width.max(1);
                let height = height.max(1);
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    let buffer = self.install_popup_buffer(file, shm, qh, width, height);
                    self.ensure_popup_effect(state);
                    self.commit_popup_surface(state, DamageRect::full(width, height));
                    return ReturnData::WlBuffer(buffer);
                }
                self.shm = Some(shm.clone());
                self.queue_handle = Some(qh.clone());
                let size_changed = self.surface_size != (width, height);
                self.surface_size = (width, height);
                if size_changed {
                    self.clear_frames();
                }
                ReturnData::WlBuffer(self.install_wayland_buffer(file, shm, qh, width, height))
            }
            LayerShellEvent::RequestMessages(message) => self.handle_message(message, state, id),
            LayerShellEvent::UserEvent(event) => self.handle_host_event(event, state, id),
            _ => ReturnData::None,
        }
    }

    fn handle_message(
        &mut self,
        message: &DispatchMessage,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> ReturnData<()> {
        match message {
            DispatchMessage::RequestRefresh {
                width,
                height,
                scale_float,
                ..
            } => {
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    if let Some(popup) = self.popup.as_mut() {
                        popup.size = ((*width).max(1), (*height).max(1));
                        popup.mapped = false;
                    }
                    self.ensure_popup_effect(state);
                    self.refresh_popup_surface(state);
                    return ReturnData::None;
                }
                let surface_size = ((*width).max(1), (*height).max(1));
                let size_changed = self.surface_size != surface_size;
                self.surface_size = surface_size;
                self.scale = scale_float.max(1.0);
                if size_changed {
                    self.recreate_wayland_buffer(surface_size.0, surface_size.1);
                }
                self.ensure_child();
                self.send_resize();
                if self.visible && self.main_frame_ready() {
                    self.refresh_surface(state, id);
                } else if self.visible {
                    self.hide_surface(state);
                }
            }
            DispatchMessage::Focused(_) if self.visible => {
                self.focused = true;
                self.send_control("focus\t1\n");
                self.resume("focus");
            }
            DispatchMessage::Focused(_) => {
                self.focused = false;
                self.send_control("focus\t0\n");
                self.suspend("hidden");
            }
            DispatchMessage::Unfocus => {
                self.focused = false;
                self.send_control("focus\t0\n");
                if !self.visible {
                    self.suspend("hidden");
                }
            }
            DispatchMessage::ModifiersChanged(modifiers) => {
                self.modifiers = *modifiers;
            }
            DispatchMessage::KeyboardInput {
                event,
                is_synthetic: false,
            } if self.visible => self.send_key_event(event),
            DispatchMessage::MouseEnter {
                pointer,
                surface_x,
                surface_y,
                ..
            } if self.visible => {
                self.pointer_inside = true;
                (self.cursor_x, self.cursor_y) =
                    self.pointer_position_for_unit(id, *surface_x, *surface_y);
                self.forward_mouse_move(false);
                return ReturnData::RequestSetCursorShape((
                    cursor_shape_for_wayland(&self.cursor_shape).to_string(),
                    pointer.clone(),
                ));
            }
            DispatchMessage::MouseMotion {
                surface_x,
                surface_y,
                ..
            } if self.visible => {
                self.pointer_inside = true;
                (self.cursor_x, self.cursor_y) =
                    self.pointer_position_for_unit(id, *surface_x, *surface_y);
                self.forward_mouse_move(false);
            }
            DispatchMessage::MouseLeave if self.visible => {
                self.pointer_inside = false;
                self.forward_mouse_move(true);
            }
            DispatchMessage::MouseButton { state, button, .. } if self.visible => {
                self.forward_mouse_button(*button, state);
            }
            DispatchMessage::Axis {
                horizontal,
                vertical,
                ..
            } if self.visible => {
                self.forward_mouse_wheel(axis_delta(horizontal), axis_delta(vertical))
            }
            DispatchMessage::Closed => {
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    self.popup = None;
                    return ReturnData::None;
                }
                self.begin_close();
                return ReturnData::RequestExit;
            }
            _ => {}
        }
        ReturnData::None
    }

    fn handle_host_event(
        &mut self,
        event: LayerHostEvent,
        state: &mut WindowState<()>,
        id: Option<id::Id>,
    ) -> ReturnData<()> {
        match event {
            LayerHostEvent::Connected(stream) => {
                self.socket = Some(std::sync::Arc::new(std::sync::Mutex::new(stream)));
                if self.visible {
                    self.set_surface_alpha(self.surface_alpha, state);
                } else {
                    self.force_suspend("hidden");
                }
                self.send_resize();
                self.force_current_lifecycle("connect");
            }
            LayerHostEvent::Message(OsrMessage::Frame(frame)) => {
                if self.visible {
                    match frame.surface {
                        OsrSurface::Main => {
                            let frame_size = (frame.width, frame.height);
                            if self.main_frame_surface_size != Some(frame_size) {
                                self.close_popup(state);
                            }
                            self.main_frame_surface_size = Some(frame_size);
                            self.main_frame = Some(frame);
                        }
                        OsrSurface::Popup | OsrSurface::Guest(_) => {
                            if let Some(return_data) = self.update_popup_frame(frame, state, id) {
                                return return_data;
                            }
                        }
                    }
                    if self.main_frame_ready() {
                        self.restore_keyboard(state);
                        self.force_resume("first-paint");
                        self.refresh_surface(state, id);
                    } else {
                        self.hide_surface(state);
                    }
                }
            }
            LayerHostEvent::Message(OsrMessage::PaintBatch(batch)) => {
                if self.visible
                    && let Some(return_data) = self.refresh_batch_surface(batch, state, id)
                {
                    return return_data;
                }
            }
            LayerHostEvent::Message(OsrMessage::AccelFrame(frame)) => {
                crate::osr::accel::discard_frame(frame);
            }
            LayerHostEvent::Message(OsrMessage::PopupHidden) => {
                self.close_popup(state);
            }
            LayerHostEvent::Message(OsrMessage::GuestHidden(_id)) => {
                self.close_popup(state);
            }
            LayerHostEvent::Message(OsrMessage::GuestCaptureRequested {
                browser_id,
                request_id,
                ..
            }) => {
                let result = self
                    .popup
                    .as_ref()
                    .and_then(|popup| popup.frame.as_ref().map(|frame| (popup, frame)))
                    .ok_or_else(|| "guest has no frame to capture".to_string())
                    .and_then(|(popup, frame)| {
                        guest_preview_data_url(&popup.buffer, frame.width, frame.height)
                    });
                let (status, payload) = match result {
                    Ok(data_url) => ("ok", serde_json::json!({ "dataUrl": data_url })),
                    Err(message) => ("error", serde_json::json!({ "message": message })),
                };
                self.send_control(&format!(
                    "SABINE_BRIDGE_RESPONSE\t{browser_id}\t{request_id}\t{status}\t{payload}\n"
                ));
            }
            LayerHostEvent::Message(OsrMessage::DraggableRegionsChanged { .. }) => {}
            LayerHostEvent::Message(OsrMessage::Cursor(cursor)) => {
                self.cursor_shape = cursor;
            }
            LayerHostEvent::Message(OsrMessage::CloseRequested) => {
                return ReturnData::RequestExit;
            }
            LayerHostEvent::Message(OsrMessage::StartDragRequested) => {}
            LayerHostEvent::Message(OsrMessage::FileDragRequested(_)) => {}
            LayerHostEvent::Message(OsrMessage::MinimizeRequested) => {}
            LayerHostEvent::Message(OsrMessage::ToggleMaximizeRequested) => {}
            LayerHostEvent::Message(OsrMessage::ShowRequested) => {
                self.set_surface_visible(true, state)
            }
            LayerHostEvent::Message(OsrMessage::HideRequested) => {
                self.set_surface_visible(false, state)
            }
            LayerHostEvent::Message(OsrMessage::FocusRequested) => {
                self.set_surface_visible(true, state)
            }
            LayerHostEvent::Message(OsrMessage::BridgeRequest(line)) => {
                if !line.is_empty() {
                    let mut output = std::io::stdout();
                    use std::io::Write;
                    let _ = writeln!(output, "{line}");
                    let _ = output.flush();
                }
            }
            LayerHostEvent::Visible(visible) => self.set_surface_visible(visible, state),
            LayerHostEvent::Alpha(alpha) => self.set_surface_alpha(alpha, state),
            LayerHostEvent::Margin(margin) => self.set_surface_margin(margin, state),
            LayerHostEvent::ControlLine(line) => {
                let mut line = line;
                if !line.ends_with('\n') {
                    line.push('\n');
                }
                self.send_control(&line);
            }
            LayerHostEvent::Disconnected => {
                self.socket = None;
                return ReturnData::RequestExit;
            }
        }
        ReturnData::None
    }

    pub(super) fn ensure_child(&mut self) {
        if self.child.is_some() {
            return;
        }
        let Some(app_id) = self
            .config
            .app_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            eprintln!("Sabine layer OSR host requires a non-empty app_id");
            return;
        };
        let Ok(authentication_token) = crate::osr::transport::authentication_token() else {
            eprintln!("failed to secure Sabine layer OSR transport");
            return;
        };
        let Some(socket_path) = super::socket::open_socket_reader(
            self.sender.clone(),
            authentication_token.clone(),
            app_id,
        ) else {
            return;
        };

        let (width, height, scale) = self.content_size_for_cef();
        let endpoint = crate::osr::transport::IpcEndpoint::Unix(socket_path);
        let mut command = crate::osr::cef_osr_command(
            &self.config.runtime_dir,
            &self.config.host_binary,
            &endpoint,
            &authentication_token,
            &self.config,
            crate::osr::CefViewport {
                width,
                height,
                scale,
                frame_rate: self.active_frame_rate(),
                accelerated_paint: false,
            },
        );
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to launch Sabine layer OSR child: {error}");
                return;
            }
        };
        self.child = Some(child);
    }
}
