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
        popup.pool = SlotPool::new(byte_len.max(1), &Shm::from(shm.clone())).ok();
        popup.buffers.clear();
        popup.pending_refresh = false;
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
            buffers: Vec::new(),
            pending_refresh: false,
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
        self.commit_popup_surface(state, damage);
    }

    pub(super) fn commit_popup_surface(&mut self, state: &mut WindowState<()>, damage: DamageRect) {
        self.ensure_popup_effect(state);
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(unit) = state.get_unit_with_id(popup.id) else {
            return;
        };
        let Some(buffer_index) = prepare_popup_buffer(popup) else {
            if popup.pool.is_none() {
                unit.refresh();
            }
            popup.pending_refresh = true;
            return;
        };
        let damage = if popup.mapped && !popup.pending_refresh {
            damage
        } else {
            DamageRect::full(popup.size.0, popup.size.1)
        };
        let surface = unit.get_wlsurface();
        if popup.buffers[buffer_index].attach_to(surface).is_err() {
            popup.pending_refresh = true;
            return;
        }
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        surface.commit();
        if !super::surface::flush_surface(surface) {
            self.wayland_failed = true;
        }
        popup.pending_refresh = false;
        popup.mapped = true;
    }

    pub(super) fn commit_pending_popup_surface(&mut self, state: &mut WindowState<()>) {
        let Some(popup) = self.popup.as_ref() else {
            return;
        };
        if !popup.pending_refresh {
            return;
        }
        let damage = DamageRect::full(popup.size.0, popup.size.1);
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
            unit.set_blur_option(BlurOption::FullRegion);
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
        popup.pool = SlotPool::new(
            buffer_len(popup.size.0, popup.size.1).max(1),
            &Shm::from(shm.clone()),
        )
        .ok();
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
        self.commit_popup_surface(state, damage);
    }
}

fn prepare_popup_buffer(popup: &mut PopupSurface) -> Option<usize> {
    let pool = popup.pool.as_mut()?;
    let pixels = popup.buffer.as_slice();
    if pixels.len() != buffer_len(popup.size.0, popup.size.1) {
        return None;
    }
    let stride = pixel_stride(popup.size.0);

    for (index, buffer) in popup.buffers.iter().enumerate() {
        if let Some(canvas) = buffer.canvas(pool) {
            return copy_pixels_to_canvas(canvas, pixels, popup.size.0, popup.size.1, stride)
                .then_some(index);
        }
    }

    if popup.buffers.len() >= MAX_POPUP_BUFFERS {
        return None;
    }
    let (buffer, canvas) = pool
        .create_buffer(
            popup.size.0 as i32,
            popup.size.1 as i32,
            stride as i32,
            wayland_client::protocol::wl_shm::Format::Argb8888,
        )
        .ok()?;
    if !copy_pixels_to_canvas(canvas, pixels, popup.size.0, popup.size.1, stride) {
        return None;
    }
    popup.buffers.push(buffer);
    Some(popup.buffers.len() - 1)
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
