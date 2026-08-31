use std::fs::File;

use layershellev::blur::BlurOption;
use layershellev::reexport::xdg_positioner::{Anchor, ConstraintAdjustment, Gravity};
use layershellev::{NewPopUpSettings, PixelSize, PopupPlacement, ReturnData, WindowState, id};
use smithay_client_toolkit::shm::{Shm, slot::SlotPool};
use wayland_client::{QueueHandle, protocol::wl_buffer::WlBuffer};

use crate::osr::frame_buffer::buffer_len;
use crate::osr::protocol::{OsrFrame, OsrPaintBatch, OsrSurface};

use super::buffer::{
    DamageRect, compose_frames_buffer, copy_pixels_to_canvas, paint_buffer_file,
    paint_frames_buffer_file, pixel_stride,
};
use super::surface::create_buffer;
use super::types::{OsrLayerHost, PopupSurface};

const MAX_POPUP_BUFFERS: usize = 4;

impl OsrLayerHost {
    pub(super) fn install_popup_buffer(
        &mut self,
        file: &mut File,
        shm: &wayland_client::protocol::wl_shm::WlShm,
        qh: &QueueHandle<WindowState<()>>,
        width: u32,
        height: u32,
    ) -> WlBuffer {
        let Some(popup) = self.popup.as_mut() else {
            return create_buffer(file, shm, qh, width, height);
        };
        popup.size = (width, height);
        popup.mapped = false;
        let byte_len = buffer_len(width, height);
        let paint_result = if popup.pending_frames.is_empty() {
            paint_buffer_file(
                file,
                width,
                height,
                popup.frame.as_ref(),
                None,
                &mut popup.buffer,
                &mut popup.scratch,
            )
        } else {
            paint_frames_buffer_file(
                file,
                width,
                height,
                &popup.pending_frames,
                &[],
                &mut popup.buffer,
                &mut popup.scratch,
            )
        };
        popup.pending_frames.clear();
        if paint_result.is_err() {
            let _ = file.set_len(byte_len as u64);
        }
        let buffer = create_buffer(file, shm, qh, width, height);
        match SlotPool::new(byte_len.max(1), &Shm::from(shm.clone())) {
            Ok(pool) => {
                popup.pool = Some(pool);
                popup.pool_error = None;
            }
            Err(error) => {
                popup.pool = None;
                popup.pool_error = Some(error.to_string());
            }
        }
        popup.buffers.clear();
        popup.pending_refresh = false;
        popup.pending_damage = None;
        buffer
    }

    pub(super) fn update_popup_frame(
        &mut self,
        frame: OsrFrame,
        state: &mut WindowState<()>,
        parent_id: Option<id::Id>,
    ) -> Option<ReturnData<()>> {
        self.main_frame.as_ref()?;
        let position = (frame.x, frame.y);
        let size = (frame.width.max(1), frame.height.max(1));
        let local_frame = local_popup_frame(frame);
        if self
            .popup
            .as_ref()
            .is_none_or(|popup| popup.position != position || popup.size != size)
        {
            return Some(self.create_popup_surface(position, size, local_frame, state, parent_id));
        }
        if let Some(popup) = self.popup.as_mut() {
            popup.frame = Some(local_frame);
        }
        self.refresh_popup_surface(state);
        None
    }

    pub(super) fn update_popup_batch(
        &mut self,
        batch: OsrPaintBatch,
        state: &mut WindowState<()>,
        parent_id: Option<id::Id>,
    ) -> Option<ReturnData<()>> {
        self.main_frame.as_ref()?;
        let position = (batch.x, batch.y);
        let size = (batch.width.max(1), batch.height.max(1));
        if self
            .popup
            .as_ref()
            .is_none_or(|popup| popup.position != position || popup.size != size)
        {
            let local_frames = batch
                .frames
                .iter()
                .cloned()
                .map(local_popup_frame)
                .collect::<Vec<_>>();
            let frame = local_frames
                .last()
                .cloned()
                .unwrap_or_else(|| empty_popup_frame(size));
            let return_data = self.create_popup_surface(position, size, frame, state, parent_id);
            if let Some(popup) = self.popup.as_mut() {
                popup.pending_frames = local_frames;
                popup.frame = popup.pending_frames.last().cloned();
            }
            return Some(return_data);
        }
        self.paint_popup_batch(&batch, state);
        None
    }

    pub(super) fn create_popup_surface(
        &mut self,
        position: (i32, i32),
        size: (u32, u32),
        frame: OsrFrame,
        state: &mut WindowState<()>,
        parent_id: Option<id::Id>,
    ) -> ReturnData<()> {
        self.close_popup(state);
        let parent_id = parent_id.unwrap_or_else(|| state.main_window().id());
        let mut popup_id = id::Id::unique();
        if popup_id == parent_id {
            popup_id = id::Id::unique();
        }
        self.popup = Some(PopupSurface {
            id: popup_id,
            position,
            size,
            frame: Some(frame),
            pending_frames: Vec::new(),
            pool: None,
            pool_error: None,
            buffers: Vec::new(),
            pending_refresh: false,
            pending_damage: None,
            buffer: Vec::new(),
            scratch: Vec::new(),
            mapped: false,
            blur_configured: false,
        });
        ReturnData::NewPopUp((
            NewPopUpSettings {
                size: PixelSize::px(size.0, size.1),
                id: parent_id,
                placement: PopupPlacement::Position(position),
                anchor: Anchor::TopLeft,
                gravity: Gravity::BottomRight,
                constraint_adjustment: ConstraintAdjustment::SlideX
                    | ConstraintAdjustment::SlideY
                    | ConstraintAdjustment::FlipX
                    | ConstraintAdjustment::FlipY,
                grab_serial: None,
            },
            popup_id,
            None,
        ))
    }

    pub(super) fn refresh_popup_surface(&mut self, state: &mut WindowState<()>) {
        self.ensure_popup_pool();
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let damage = compose_frames_buffer(
            popup.size.0,
            popup.size.1,
            popup.frame.as_slice(),
            &mut popup.buffer,
        );
        self.schedule_popup_surface(state, damage);
    }

    fn schedule_popup_surface(&mut self, state: &mut WindowState<()>, damage: DamageRect) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        popup.pending_damage = Some(
            popup
                .pending_damage
                .map_or(damage, |pending| pending.union(damage)),
        );
        popup.pending_refresh = true;
        state.request_refresh(popup.id, layershellev::RefreshRequest::NextFrame);
    }

    pub(super) fn commit_popup_surface(&mut self, state: &mut WindowState<()>, damage: DamageRect) {
        self.ensure_popup_effect(state);
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(unit) = state.get_unit_with_id(popup.id) else {
            return;
        };
        let buffer_index = match prepare_popup_buffer(popup) {
            BufferPreparation::Ready(index) => index,
            BufferPreparation::Busy => {
                popup.pending_refresh = true;
                let popup_id = popup.id;
                let surface = unit.get_wlsurface().clone();
                state.request_next_present(popup_id);
                surface.commit();
                if !super::surface::flush_surface(&surface) {
                    self.wayland_failed = true;
                    return;
                }
                state.request_refresh(popup_id, layershellev::RefreshRequest::NextFrame);
                return;
            }
            BufferPreparation::Fatal(error) => {
                eprintln!("Sabine layer popup buffer failed: {error}");
                self.wayland_failed = true;
                return;
            }
        };
        let damage = if popup.mapped {
            popup.pending_damage.take().unwrap_or(damage)
        } else {
            DamageRect::full(popup.size.0, popup.size.1)
        };
        let surface = unit.get_wlsurface().clone();
        if let Err(error) = popup.buffers[buffer_index].attach_to(&surface) {
            eprintln!("Sabine layer popup buffer attach failed: {error}");
            self.wayland_failed = true;
            return;
        }
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        state.request_next_present(popup.id);
        surface.commit();
        if !super::surface::flush_surface(&surface) {
            self.wayland_failed = true;
        }
        popup.pending_refresh = false;
        popup.pending_damage = None;
        popup.mapped = true;
    }

    pub(super) fn commit_pending_popup_surface(&mut self, state: &mut WindowState<()>) {
        let Some(popup) = self.popup.as_ref() else {
            return;
        };
        if !popup.pending_refresh {
            return;
        }
        let damage = popup
            .pending_damage
            .unwrap_or_else(|| DamageRect::full(popup.size.0, popup.size.1));
        self.commit_popup_surface(state, damage);
    }

    pub(super) fn ensure_popup_effect(&mut self, state: &mut WindowState<()>) {
        let Some(popup) = self.popup.as_ref() else {
            return;
        };
        if popup.blur_configured {
            return;
        }
        let popup_id = popup.id;
        if let Some(unit) = state.get_mut_unit_with_id(popup_id) {
            let blur = if self.config.transparent
                && self.config.background_effect != sabine_platform::WindowBackgroundEffect::None
            {
                BlurOption::FullRegion
            } else {
                BlurOption::None
            };
            unit.set_blur_option(blur);
            if let Some(popup) = self.popup.as_mut() {
                popup.blur_configured = true;
            }
        }
    }

    pub(super) fn close_popup(&mut self, state: &mut WindowState<()>) {
        if let Some(popup) = self.popup.take() {
            state.request_close(popup.id);
        }
    }

    fn ensure_popup_pool(&mut self) {
        let Some(shm) = self.shm.as_ref() else {
            return;
        };
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        if popup.pool.is_some() {
            return;
        }
        match SlotPool::new(
            buffer_len(popup.size.0, popup.size.1).max(1),
            &Shm::from(shm.clone()),
        ) {
            Ok(pool) => {
                popup.pool = Some(pool);
                popup.pool_error = None;
            }
            Err(error) => popup.pool_error = Some(error.to_string()),
        }
    }

    pub(super) fn pointer_position_for_unit(
        &self,
        id: Option<id::Id>,
        surface_x: f64,
        surface_y: f64,
    ) -> (f32, f32) {
        if let Some(popup) = &self.popup
            && Some(popup.id) == id
        {
            return (
                surface_x as f32 + popup.position.0 as f32,
                surface_y as f32 + popup.position.1 as f32,
            );
        }
        (surface_x as f32, surface_y as f32)
    }

    fn paint_popup_batch(&mut self, batch: &OsrPaintBatch, state: &mut WindowState<()>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let local_frames = batch
            .frames
            .iter()
            .cloned()
            .map(local_popup_frame)
            .collect::<Vec<_>>();
        let damage =
            compose_frames_buffer(popup.size.0, popup.size.1, &local_frames, &mut popup.buffer);
        popup.frame = local_frames.last().cloned();
        self.schedule_popup_surface(state, damage);
    }
}

enum BufferPreparation {
    Ready(usize),
    Busy,
    Fatal(String),
}

fn prepare_popup_buffer(popup: &mut PopupSurface) -> BufferPreparation {
    let Some(pool) = popup.pool.as_mut() else {
        return BufferPreparation::Fatal(
            popup
                .pool_error
                .clone()
                .unwrap_or_else(|| "SHM pool is unavailable".to_string()),
        );
    };
    let pixels = popup.buffer.as_slice();
    if pixels.len() != buffer_len(popup.size.0, popup.size.1) {
        return BufferPreparation::Fatal("popup pixel buffer has an invalid size".to_string());
    }
    let stride = pixel_stride(popup.size.0);

    for (index, buffer) in popup.buffers.iter().enumerate() {
        if let Some(canvas) = buffer.canvas(pool) {
            return if copy_pixels_to_canvas(canvas, pixels, popup.size.0, popup.size.1, stride) {
                BufferPreparation::Ready(index)
            } else {
                BufferPreparation::Fatal("popup SHM canvas has an invalid layout".to_string())
            };
        }
    }

    if popup.buffers.len() >= MAX_POPUP_BUFFERS {
        return BufferPreparation::Busy;
    }
    let (buffer, canvas) = match pool.create_buffer(
        popup.size.0 as i32,
        popup.size.1 as i32,
        stride as i32,
        wayland_client::protocol::wl_shm::Format::Argb8888,
    ) {
        Ok(buffer) => buffer,
        Err(error) => return BufferPreparation::Fatal(error.to_string()),
    };
    if !copy_pixels_to_canvas(canvas, pixels, popup.size.0, popup.size.1, stride) {
        return BufferPreparation::Fatal("popup SHM canvas has an invalid layout".to_string());
    }
    popup.buffers.push(buffer);
    BufferPreparation::Ready(popup.buffers.len() - 1)
}

fn local_popup_frame(mut frame: OsrFrame) -> OsrFrame {
    frame.x = 0;
    frame.y = 0;
    frame
}

fn empty_popup_frame(size: (u32, u32)) -> OsrFrame {
    OsrFrame {
        surface: OsrSurface::Popup,
        width: size.0,
        height: size.1,
        x: 0,
        y: 0,
        bytes: vec![0; buffer_len(size.0, size.1)].into(),
    }
}
