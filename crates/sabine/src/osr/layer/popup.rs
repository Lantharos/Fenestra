use std::fs::File;

use layershellev::reexport::xdg_positioner::{Anchor, ConstraintAdjustment, Gravity};
use layershellev::{NewPopUpSettings, PopupPlacement, ReturnData, WindowState, id};
use sabine_platform::{WindowBackgroundEffect, WindowOptions, WindowRegion, WindowRegions};
use wayland_client::{QueueHandle, protocol::wl_buffer::WlBuffer};

use crate::osr::frame_buffer::buffer_len;
use crate::osr::protocol::{OsrFrame, OsrPaintBatch, OsrSurface};

use super::buffer::{DamageRect, paint_buffer_file, paint_frames_buffer_file};
use super::surface::create_buffer;
use super::types::{OsrLayerHost, PopupSurface};

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
        if let Ok(clone) = file.try_clone() {
            popup.buffer_file = Some(clone);
        }
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
        popup.wayland_buffer = Some(buffer.clone());
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
            buffer_file: None,
            wayland_buffer: None,
            buffer: Vec::new(),
            scratch: Vec::new(),
            mapped: false,
            effect: None,
        });
        ReturnData::NewPopUp((
            NewPopUpSettings {
                size,
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
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        let Some(file) = &popup.buffer_file else {
            return;
        };
        let Ok(mut file) = file.try_clone() else {
            return;
        };
        let damage = match paint_buffer_file(
            &mut file,
            popup.size.0,
            popup.size.1,
            popup.frame.as_ref(),
            None,
            &mut popup.buffer,
            &mut popup.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return,
        };
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
        let Some(buffer) = popup.wayland_buffer.as_ref() else {
            unit.refresh();
            return;
        };
        let damage = if popup.mapped {
            damage
        } else {
            DamageRect::full(popup.size.0, popup.size.1)
        };
        let surface = unit.get_wlsurface();
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        surface.commit();
        popup.mapped = true;
    }

    pub(super) fn ensure_popup_effect(&mut self, state: &WindowState<()>) {
        let Some(popup) = self.popup.as_mut() else {
            return;
        };
        if popup.effect.is_some() {
            return;
        }
        let Some(unit) = state.get_unit_with_id(popup.id) else {
            return;
        };
        let options = popup_effect_options(popup.size);
        popup.effect = sabine_platform::request_surface_effect(
            unit,
            &options,
            popup.size.0 as i32,
            popup.size.1 as i32,
        );
    }

    pub(super) fn close_popup(&mut self, state: &mut WindowState<()>) {
        if let Some(popup) = self.popup.take() {
            state.request_close(popup.id);
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
        let Some(file) = &popup.buffer_file else {
            if let Some(frame) = batch.frames.last().cloned() {
                popup.frame = Some(frame);
            }
            return;
        };
        let Ok(mut file) = file.try_clone() else {
            return;
        };
        let local_frames = batch
            .frames
            .iter()
            .cloned()
            .map(local_popup_frame)
            .collect::<Vec<_>>();
        let damage = match paint_frames_buffer_file(
            &mut file,
            popup.size.0,
            popup.size.1,
            &local_frames,
            &[],
            &mut popup.buffer,
            &mut popup.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return,
        };
        popup.frame = local_frames.last().cloned();
        self.commit_popup_surface(state, damage);
    }
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

fn popup_effect_options(size: (u32, u32)) -> WindowOptions {
    WindowOptions {
        width: size.0,
        height: size.1,
        transparent: true,
        background_effect: WindowBackgroundEffect::Blur,
        regions: WindowRegions::new().blur(WindowRegion::adaptive_full()),
        ..WindowOptions::default()
    }
}
