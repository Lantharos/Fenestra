use std::time::{Duration, Instant};

use layershellev::{
    DispatchMessage, ExWlShellEvent as LayerShellEvent, ReturnData, WindowState, id,
};

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
        if self.wayland_failed {
            return ReturnData::RequestExit;
        }
        match event {
            LayerShellEvent::InitRequest => ReturnData::RequestBind,
            LayerShellEvent::BindProvide(globals, qh) => {
                let Ok(shm) =
                    globals.bind::<layershellev::reexport::wl_shm::WlShm, _, _>(qh, 1..=1, ())
                else {
                    self.wayland_failed = true;
                    return ReturnData::RequestExit;
                };
                self.install_shm(shm, qh.clone());
                ReturnData::None
            }
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
                let buffer = self.install_wayland_buffer(file, shm, qh, width, height);
                self.surface_mapped = true;
                if self.visible {
                    self.update_main_effect(state);
                }
                ReturnData::WlBuffer(buffer)
            }
            LayerShellEvent::RequestMessages(message) => self.handle_message(message, state, id),
            LayerShellEvent::UserEvent(event) => self.handle_host_event(event, state, id),
            LayerShellEvent::NormalDispatch => {
                self.drive_child();
                self.refresh_loading(state);
                self.drive_tooltip(state);
                self.commit_pending_surface(state);
                self.commit_pending_popup_surface(state);
                ReturnData::None
            }
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
                configure_generation,
            } => {
                if self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| Some(popup.id) == id)
                {
                    if let Some(popup) = self.popup.as_mut() {
                        let size = ((*width).max(1), (*height).max(1));
                        if popup.size != size {
                            popup.size = size;
                            popup.pool = None;
                            popup.buffers.clear();
                            popup.pending_refresh = false;
                        }
                        popup.mapped = false;
                    }
                    self.ensure_popup_effect(state);
                    self.refresh_popup_surface(state);
                    return ReturnData::None;
                }
                self.configure_generation = *configure_generation;
                if self.remap_sync_token.is_some() {
                    return ReturnData::None;
                }
                if self
                    .remap_configure_generation
                    .is_some_and(|generation| *configure_generation <= generation)
                {
                    return ReturnData::None;
                }
                self.remap_configure_generation = None;
                let surface_size = ((*width).max(1), (*height).max(1));
                let size_changed = self.surface_size != surface_size;
                self.surface_size = surface_size;
                self.scale = scale_float.max(1.0);
                if size_changed {
                    self.recreate_wayland_buffer(surface_size.0, surface_size.1);
                }
                if self.visible {
                    self.update_main_effect(state);
                }
                self.ensure_child();
                self.send_resize();
                self.drive_tooltip(state);
                if !self.visible {
                    self.hide_surface(state);
                    return ReturnData::None;
                }
                if self.visible && self.main_frame_ready() && self.loading.is_some() {
                    self.finish_loading(state);
                }
                if self.visible && self.loading.is_some() {
                    self.refresh_loading(state);
                } else if self.visible && self.main_frame_ready() {
                    self.refresh_surface(state);
                } else if self.visible {
                    self.hide_surface(state);
                }
            }
            DispatchMessage::SyncDone { token } if self.remap_sync_token == Some(*token) => {
                self.remap_sync_token = None;
                self.remap_configure_generation = Some(self.configure_generation);
                if self.visible {
                    self.restore_layer_state(state);
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
            DispatchMessage::TouchDown {
                id: touch_id, x, y, ..
            } if self.visible => {
                let (x, y) = self.pointer_position_for_unit(id, *x, *y);
                self.forward_touch(*touch_id, x, y, "pressed");
            }
            DispatchMessage::TouchMotion {
                id: touch_id, x, y, ..
            } if self.visible => {
                let (x, y) = self.pointer_position_for_unit(id, *x, *y);
                self.forward_touch(*touch_id, x, y, "moved");
            }
            DispatchMessage::TouchUp {
                id: touch_id, x, y, ..
            } if self.visible => {
                let (x, y) = self.pointer_position_for_unit(id, *x, *y);
                self.forward_touch(*touch_id, x, y, "released");
            }
            DispatchMessage::TouchCancel { id: touch_id, x, y } if self.visible => {
                let (x, y) = self.pointer_position_for_unit(id, *x, *y);
                self.forward_touch(*touch_id, x, y, "cancelled");
            }
            DispatchMessage::Ime(ime) if self.visible => self.forward_ime(ime),
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
                self.child_retry_at = None;
                self.child_handoff_deadline = None;
                self.control_writer = Some(super::socket::ControlWriter::start(stream));
                if !self.visible {
                    self.force_suspend("hidden");
                }
                self.send_resize();
                self.force_current_lifecycle("connect");
            }
            LayerHostEvent::MessagesReady(messages) => {
                for message in messages.drain() {
                    if let Some(return_data) = self.handle_osr_message(message, state, id) {
                        return return_data;
                    }
                }
            }
            LayerHostEvent::Visible {
                visible,
                request_id,
            } => self.set_surface_visible(visible, request_id, state),
            LayerHostEvent::Alpha(alpha) => self.set_surface_alpha(alpha, state),
            LayerHostEvent::Margin(margin) => self.set_surface_margin(margin, state),
            LayerHostEvent::Size(width, height) => self.set_surface_size(width, height, state),
            LayerHostEvent::FrameRate(frame_rate) => self.set_active_frame_rate(frame_rate),
            LayerHostEvent::Quit => return ReturnData::RequestExit,
            LayerHostEvent::ControlLine(line) => {
                let mut line = line;
                if !line.ends_with('\n') {
                    line.push('\n');
                }
                self.send_control(&line);
            }
            LayerHostEvent::Disconnected => {
                self.control_writer = None;
                return ReturnData::RequestExit;
            }
        }
        ReturnData::None
    }

    fn handle_osr_message(
        &mut self,
        message: OsrMessage,
        state: &mut WindowState<()>,
        id: Option<id::Id>,
    ) -> Option<ReturnData<()>> {
        match message {
            OsrMessage::Frame(frame) => {
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
                                return Some(return_data);
                            }
                        }
                    }
                    if self.main_frame_ready() && self.loading.is_some() {
                        self.finish_loading(state);
                    }
                    if self.loading.is_some() {
                        self.refresh_loading(state);
                    } else if self.main_frame_ready() {
                        self.force_resume("first-paint");
                        self.refresh_surface(state);
                    } else {
                        self.hide_surface(state);
                    }
                } else {
                    self.cache_hidden_main_frame(frame);
                }
            }
            OsrMessage::PaintBatch(batch) => {
                if self.visible {
                    if let Some(return_data) = self.refresh_batch_surface(batch, state, id) {
                        return Some(return_data);
                    }
                } else {
                    self.cache_hidden_main_batch(batch);
                }
            }
            OsrMessage::AccelFrame(frame) => {
                crate::osr::accel::discard_frame(frame);
            }
            OsrMessage::PopupHidden => {
                self.close_popup(state);
            }
            OsrMessage::GuestHidden(_id) => {
                self.close_popup(state);
            }
            OsrMessage::GuestCaptureRequested {
                browser_id,
                request_id,
                ..
            } => {
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
            OsrMessage::DraggableRegionsChanged { .. } => {}
            OsrMessage::Cursor(cursor) => {
                self.cursor_shape = cursor;
            }
            OsrMessage::CloseRequested => {
                return Some(ReturnData::RequestExit);
            }
            OsrMessage::StartDragRequested => {}
            OsrMessage::FileDragRequested(_) => {
                self.send_control(&format!(
                    "file_drag_ended\t{:.0}\t{:.0}\tnone\n",
                    self.cursor_x, self.cursor_y
                ));
            }
            OsrMessage::MinimizeRequested => {}
            OsrMessage::ToggleMaximizeRequested => {}
            OsrMessage::FullscreenRequested(_) => {}
            OsrMessage::MainLoadStarted => {
                self.main_load_ready = false;
                self.begin_loading(crate::osr::host::types::LoadingKind::Opening, state);
            }
            OsrMessage::MainLoadReady => {
                self.main_load_ready = true;
                if self.main_frame.is_some() {
                    self.finish_loading(state);
                    self.refresh_surface(state);
                }
            }
            OsrMessage::ImeStateChanged(mode) => self.update_ime_state(mode, state),
            OsrMessage::ImeCursorAreaChanged {
                x,
                y,
                width,
                height,
            } => self.update_ime_cursor_area(state, id, x, y, width, height),
            OsrMessage::TooltipChanged(text) => self.update_tooltip(text, state),
            OsrMessage::ImeSurroundingChanged { .. } => {}
            OsrMessage::ShowRequested => self.set_surface_visible(true, None, state),
            OsrMessage::HideRequested => self.set_surface_visible(false, None, state),
            OsrMessage::FocusRequested(_) => self.set_surface_visible(true, None, state),
            OsrMessage::BridgeRequest(line) => {
                if !line.is_empty() {
                    let mut output = std::io::stdout();
                    use std::io::Write;
                    let _ = writeln!(output, "{line}");
                    let _ = output.flush();
                }
            }
        }
        None
    }

    pub(super) fn ensure_child(&mut self) {
        if self.control_writer.is_some()
            || self.child.is_some()
            || self
                .child_retry_at
                .is_some_and(|deadline| Instant::now() < deadline)
            || self
                .child_handoff_deadline
                .is_some_and(|deadline| Instant::now() < deadline)
        {
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
        let mut command = match crate::osr::cef_osr_command(
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
        ) {
            Ok(command) => command,
            Err(error) => {
                eprintln!("failed to prepare Sabine layer OSR child: {error}");
                return;
            }
        };
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                eprintln!("failed to launch Sabine layer OSR child: {error}");
                return;
            }
        };
        self.child_retry_at = None;
        self.child_handoff_deadline = None;
        self.child = Some(child);
    }

    fn drive_child(&mut self) {
        if self.control_writer.is_some() {
            return;
        }
        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(None) => {
                    self.child = Some(child);
                    return;
                }
                Ok(Some(status))
                    if status.code() == Some(CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED) =>
                {
                    self.child_handoff_deadline = Some(Instant::now() + PROFILE_HANDOFF_TIMEOUT);
                }
                Ok(Some(status)) => {
                    eprintln!(
                        "Sabine layer OSR child exited ({status}); retrying shared-profile launch"
                    );
                    self.child_retry_at = Some(Instant::now() + CHILD_RETRY_DELAY);
                }
                Err(error) => {
                    eprintln!("failed to inspect Sabine layer OSR child: {error}");
                    self.child_retry_at = Some(Instant::now() + CHILD_RETRY_DELAY);
                }
            }
        }
        if self
            .child_handoff_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            eprintln!("Sabine layer OSR profile handoff timed out; retrying launch");
            self.child_handoff_deadline = None;
        }
        self.ensure_child();
    }
}

const CEF_RESULT_CODE_NORMAL_EXIT_PROCESS_NOTIFIED: i32 = 24;
const CHILD_RETRY_DELAY: Duration = Duration::from_millis(150);
const PROFILE_HANDOFF_TIMEOUT: Duration = Duration::from_secs(15);
