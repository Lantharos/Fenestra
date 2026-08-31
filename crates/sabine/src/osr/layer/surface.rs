use std::{fs::File, os::fd::AsFd};

use layershellev::{RefreshRequest, WindowState, reexport::wl_shm};
use smithay_client_toolkit::shm::{Shm, slot::SlotPool};
use wayland_client::{Proxy, QueueHandle, protocol::wl_buffer::WlBuffer};

use crate::osr::frame_buffer::buffer_len;
use crate::osr::protocol::{OsrPaintBatch, OsrSurface};

use super::buffer::{
    DamageRect, compose_frames_buffer, copy_pixels_to_canvas, paint_buffer_file, pixel_stride,
};
use super::shell::{anchor_for_shell, keyboard_for_shell, layer_for_shell};
use super::types::OsrLayerHost;

const MAX_MAIN_BUFFERS: usize = 4;

impl OsrLayerHost {
    pub(super) fn install_shm(&mut self, shm: wl_shm::WlShm, qh: QueueHandle<WindowState<()>>) {
        self.reset_main_pool(&shm, buffer_len(self.buffer_size.0, self.buffer_size.1));
        self.shm = Some(shm);
        self.queue_handle = Some(qh);
    }

    pub(super) fn cache_hidden_main_frame(&mut self, frame: crate::osr::protocol::OsrFrame) {
        if frame.surface != OsrSurface::Main {
            return;
        }
        let frame_size = (frame.width, frame.height);
        if frame.x == 0 && frame.y == 0 && frame_size == self.surface_size {
            self.main_buffer.clear();
            self.main_frame_surface_size = Some(frame_size);
        } else if self.main_frame_surface_size != Some(self.surface_size) {
            return;
        }
        compose_frames_buffer(
            self.buffer_size.0,
            self.buffer_size.1,
            std::slice::from_ref(&frame),
            &mut self.main_buffer,
        );
        self.main_frame = Some(frame);
    }

    pub(super) fn cache_hidden_main_batch(&mut self, batch: OsrPaintBatch) {
        if batch.surface != OsrSurface::Main || (batch.width, batch.height) != self.surface_size {
            return;
        }
        compose_frames_buffer(
            batch.width,
            batch.height,
            &batch.frames,
            &mut self.main_buffer,
        );
        if let Some(frame) = batch.frames.last().cloned() {
            self.main_frame = Some(frame);
            self.main_frame_surface_size = Some((batch.width, batch.height));
        }
    }

    pub(super) fn install_wayland_buffer(
        &mut self,
        file: &mut File,
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<WindowState<()>>,
        width: u32,
        height: u32,
    ) -> WlBuffer {
        if self.buffer_size != (width, height) {
            self.main_buffer.clear();
            self.presentation_buffer.clear();
            self.scratch.clear();
        }
        self.buffer_size = (width, height);
        self.surface_mapped = false;
        let byte_len = buffer_len(width, height);
        if paint_buffer_file(
            file,
            width,
            height,
            self.main_frame.as_ref(),
            None,
            &mut self.main_buffer,
            &mut self.scratch,
        )
        .is_err()
        {
            let _ = file.set_len(byte_len as u64);
        }
        let pool = shm.create_pool(file.as_fd(), byte_len as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            pixel_stride(width) as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        self.reset_main_pool(shm, byte_len);
        buffer
    }

    pub(super) fn recreate_wayland_buffer(&mut self, width: u32, height: u32) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        self.clear_frames();
        let width = width.max(1);
        let height = height.max(1);
        self.buffer_size = (width, height);
        self.main_buffer.clear();
        self.presentation_buffer.clear();
        self.scratch.clear();
        self.reset_main_pool(&shm, buffer_len(width, height));
    }

    pub(super) fn refresh_surface(&mut self, state: &mut WindowState<()>) {
        if !self.visible || !self.main_frame_ready() {
            return;
        }
        let mut damage = compose_frames_buffer(
            self.buffer_size.0,
            self.buffer_size.1,
            self.main_frame.as_slice(),
            &mut self.main_buffer,
        );
        self.prepare_tooltip_buffer();
        if self.presentation_full_damage {
            damage = DamageRect::full(self.buffer_size.0, self.buffer_size.1);
        }
        self.schedule_surface(state, damage);
    }

    pub(super) fn refresh_batch_surface(
        &mut self,
        batch: OsrPaintBatch,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) -> Option<layershellev::ReturnData<()>> {
        if batch.surface == OsrSurface::Main && (batch.width, batch.height) != self.surface_size {
            self.main_frame = None;
            self.main_frame_surface_size = Some((batch.width, batch.height));
            self.close_popup(state);
            self.hide_surface(state);
            return None;
        }
        if matches!(batch.surface, OsrSurface::Popup | OsrSurface::Guest(_)) {
            return self.update_popup_batch(batch, state, id);
        }
        let mut damage = compose_frames_buffer(
            self.buffer_size.0,
            self.buffer_size.1,
            &batch.frames,
            &mut self.main_buffer,
        );
        match batch.surface {
            OsrSurface::Main => {
                self.main_frame = batch.frames.last().cloned();
                self.main_frame_surface_size = Some((batch.width, batch.height));
            }
            OsrSurface::Popup | OsrSurface::Guest(_) => {}
        }
        if self.main_frame_ready() && self.loading.is_some() {
            self.finish_loading(state);
        }
        if self.loading.is_some() && self.refresh_loading(state) {
            return None;
        }
        self.prepare_tooltip_buffer();
        if self.presentation_full_damage {
            damage = DamageRect::full(self.buffer_size.0, self.buffer_size.1);
        }
        if !self.main_frame_ready() {
            return None;
        }
        self.force_resume("first-paint");
        self.schedule_surface(state, damage);
        None
    }

    fn schedule_surface(&mut self, state: &mut WindowState<()>, damage: DamageRect) {
        self.pending_surface_damage = Some(
            self.pending_surface_damage
                .map_or(damage, |pending| pending.union(damage)),
        );
        self.pending_surface_refresh = true;
        let main_id = state.main_window().id();
        state.request_refresh(main_id, RefreshRequest::NextFrame);
    }

    pub(super) fn hide_surface(&mut self, state: &mut WindowState<()>) {
        if !self.surface_mapped {
            self.acknowledge_visibility(false);
            return;
        }
        let unit = state.main_window();
        unit.get_wlsurface().attach(None, 0, 0);
        unit.get_wlsurface().commit();
        self.next_remap_sync_token = self.next_remap_sync_token.wrapping_add(1);
        let sync_token = self.next_remap_sync_token;
        unit.request_sync(sync_token);
        if !flush_surface(unit.get_wlsurface()) {
            self.wayland_failed = true;
            return;
        }
        self.surface_mapped = false;
        self.remap_sync_token = Some(sync_token);
        self.remap_configure_generation = None;
        self.acknowledge_visibility(false);
    }

    pub(super) fn main_frame_ready(&self) -> bool {
        self.main_load_ready
            && self.main_frame.is_some()
            && self.main_frame_surface_size == Some(self.surface_size)
    }

    pub(super) fn retained_frame_ready(&self) -> bool {
        self.config.lifecycle.retain_hidden_frame
            && self.main_frame.is_some()
            && self.main_frame_surface_size == Some(self.surface_size)
            && self.main_buffer.len() == buffer_len(self.buffer_size.0, self.buffer_size.1)
    }

    pub(super) fn clear_frames(&mut self) {
        self.main_frame = None;
        self.main_frame_surface_size = None;
        self.popup = None;
    }

    pub(super) fn release_hidden_frame_memory(&mut self) {
        self.clear_frames();
        self.main_buffer = Vec::new();
        self.presentation_buffer = Vec::new();
        self.scratch = Vec::new();
    }

    pub(super) fn commit_surface(&mut self, state: &mut WindowState<()>, damage: DamageRect) {
        if !self.visible {
            return;
        }
        if self.remap_sync_token.is_some() || self.remap_configure_generation.is_some() {
            self.pending_surface_refresh = true;
            return;
        }
        if !self.surface_mapped {
            self.restore_layer_state(state);
        }
        let unit = state.main_window();
        let damage = if self.surface_mapped && !self.presentation_full_damage {
            damage
        } else {
            DamageRect::full(self.buffer_size.0, self.buffer_size.1)
        };
        let buffer_index = match self.prepare_main_buffer() {
            BufferPreparation::Ready(index) => index,
            BufferPreparation::Busy => {
                self.pending_surface_refresh = true;
                let surface = unit.get_wlsurface().clone();
                let unit_id = unit.id();
                state.request_next_present(unit_id);
                surface.commit();
                if !flush_surface(&surface) {
                    self.wayland_failed = true;
                    return;
                }
                state.request_refresh(unit_id, RefreshRequest::NextFrame);
                return;
            }
            BufferPreparation::Fatal(error) => {
                eprintln!("Sabine layer main buffer failed: {error}");
                self.wayland_failed = true;
                return;
            }
        };
        let unit_id = unit.id();
        let surface = unit.get_wlsurface().clone();
        if let Err(error) = self.main_buffers[buffer_index].attach_to(&surface) {
            eprintln!("Sabine layer main buffer attach failed: {error}");
            self.wayland_failed = true;
            return;
        }
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        state.request_next_present(unit_id);
        surface.commit();
        if !flush_surface(&surface) {
            self.wayland_failed = true;
            return;
        }
        self.pending_surface_refresh = false;
        self.pending_surface_damage = None;
        self.surface_mapped = true;
        self.presentation_full_damage = false;
        self.acknowledge_visibility(true);
    }

    pub(super) fn commit_current_layer_state(&mut self, state: &mut WindowState<()>) {
        self.restore_layer_state(state);
        self.presentation_full_damage = true;
        self.commit_surface(
            state,
            DamageRect::full(self.buffer_size.0, self.buffer_size.1),
        );
    }

    pub(super) fn commit_pending_surface(&mut self, state: &mut WindowState<()>) {
        if !self.pending_surface_refresh || !self.visible || !self.current_buffer_ready() {
            return;
        }
        let damage = self
            .pending_surface_damage
            .unwrap_or_else(|| DamageRect::full(self.buffer_size.0, self.buffer_size.1));
        self.commit_surface(state, damage);
    }

    fn current_buffer_ready(&self) -> bool {
        let expected = buffer_len(self.buffer_size.0, self.buffer_size.1);
        self.presentation_buffer.len() == expected || self.main_buffer.len() == expected
    }

    fn reset_main_pool(&mut self, shm: &wl_shm::WlShm, byte_len: usize) {
        self.main_buffers.clear();
        self.pending_surface_damage = None;
        match SlotPool::new(byte_len.max(1), &Shm::from(shm.clone())) {
            Ok(pool) => {
                self.main_pool = Some(pool);
                self.main_pool_error = None;
            }
            Err(error) => {
                self.main_pool = None;
                self.main_pool_error = Some(error.to_string());
            }
        }
        self.pending_surface_refresh = false;
        self.pending_surface_damage = None;
    }

    fn prepare_main_buffer(&mut self) -> BufferPreparation {
        let Some(pool) = self.main_pool.as_mut() else {
            return BufferPreparation::Fatal(
                self.main_pool_error
                    .clone()
                    .unwrap_or_else(|| "SHM pool is unavailable".to_string()),
            );
        };
        let pixels = if self.presentation_buffer.is_empty() {
            self.main_buffer.as_slice()
        } else {
            self.presentation_buffer.as_slice()
        };
        if pixels.len() != buffer_len(self.buffer_size.0, self.buffer_size.1) {
            return BufferPreparation::Fatal("main pixel buffer has an invalid size".to_string());
        }
        let stride = pixel_stride(self.buffer_size.0);

        for (index, buffer) in self.main_buffers.iter().enumerate() {
            if let Some(canvas) = buffer.canvas(pool) {
                return if copy_pixels_to_canvas(
                    canvas,
                    pixels,
                    self.buffer_size.0,
                    self.buffer_size.1,
                    stride,
                ) {
                    BufferPreparation::Ready(index)
                } else {
                    BufferPreparation::Fatal("main SHM canvas has an invalid layout".to_string())
                };
            }
        }

        if self.main_buffers.len() >= MAX_MAIN_BUFFERS {
            return BufferPreparation::Busy;
        }
        let (buffer, canvas) = match pool.create_buffer(
            self.buffer_size.0 as i32,
            self.buffer_size.1 as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
        ) {
            Ok(buffer) => buffer,
            Err(error) => return BufferPreparation::Fatal(error.to_string()),
        };
        if !copy_pixels_to_canvas(
            canvas,
            pixels,
            self.buffer_size.0,
            self.buffer_size.1,
            stride,
        ) {
            return BufferPreparation::Fatal("main SHM canvas has an invalid layout".to_string());
        }
        self.main_buffers.push(buffer);
        BufferPreparation::Ready(self.main_buffers.len() - 1)
    }

    pub(super) fn restore_layer_state(&mut self, state: &mut WindowState<()>) {
        let Some(shell_surface) = self.config.shell_surface.clone() else {
            return;
        };
        let main_id = state.main_window().id();
        let (width, height) = self.layer_commit_size();
        let unit = state
            .get_mut_unit_with_id(main_id)
            .expect("main layer surface must exist");
        unit.set_layout(
            anchor_for_shell(shell_surface.anchor),
            super::layer_size_for_shell((width, height)),
        );
        unit.set_margin((
            shell_surface.margin.top,
            shell_surface.margin.right,
            shell_surface.margin.bottom,
            shell_surface.margin.left,
        ));
        unit.set_layer(layer_for_shell(shell_surface.layer));
        unit.set_exclusive_zone(shell_surface.exclusive_zone.unwrap_or_default());
        unit.set_keyboard_interactivity(keyboard_for_shell(shell_surface.keyboard_interactivity));
        if self.alpha_modifier.is_none() {
            self.alpha_modifier = super::alpha::LayerAlphaModifier::bind(state);
        }
        if let Some(modifier) = &self.alpha_modifier {
            let _ = modifier.set_alpha(self.surface_alpha);
        }
        self.update_main_effect(state);
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

enum BufferPreparation {
    Ready(usize),
    Busy,
    Fatal(String),
}

pub(super) fn flush_surface(surface: &wayland_client::protocol::wl_surface::WlSurface) -> bool {
    if let Some(backend) = surface.backend().upgrade() {
        return backend.flush().is_ok();
    }
    false
}

pub(super) fn create_buffer(
    file: &mut File,
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<WindowState<()>>,
    width: u32,
    height: u32,
) -> WlBuffer {
    let byte_len = buffer_len(width, height);
    let pool = shm.create_pool(file.as_fd(), byte_len as i32, qh, ());
    pool.create_buffer(
        0,
        width as i32,
        height as i32,
        pixel_stride(width) as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    )
}
