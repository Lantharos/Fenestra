use std::{
    fs::{File, OpenOptions},
    os::fd::AsFd,
    time::{SystemTime, UNIX_EPOCH},
};

use layershellev::{WindowState, reexport::wl_shm};
use sabine_platform::ShellSurfaceKeyboardInteractivity;
use smithay_client_toolkit::shm::{Shm, slot::SlotPool};
use wayland_client::{QueueHandle, protocol::wl_buffer::WlBuffer};

use crate::osr::frame_buffer::buffer_len;
use crate::osr::protocol::{OsrPaintBatch, OsrSurface};

use super::buffer::{DamageRect, paint_buffer_file, paint_frames_buffer_file};
use super::shell::keyboard_for_shell;
use super::types::OsrLayerHost;

const MAX_MAIN_BUFFERS: usize = 4;

impl OsrLayerHost {
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
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        self.reset_main_pool(shm, byte_len);
        self.buffer_file = temporary_buffer_file().ok();
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
        self.surface_mapped = false;
        self.main_buffer.clear();
        self.scratch.clear();
        self.reset_main_pool(&shm, buffer_len(width, height));
        self.buffer_file = temporary_buffer_file().ok();
    }

    pub(super) fn refresh_surface(
        &mut self,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) {
        if !self.visible || !self.main_frame_ready() {
            return;
        }
        let Some(file) = &self.buffer_file else {
            return;
        };
        let Ok(mut file) = file.try_clone() else {
            return;
        };
        let damage = match paint_buffer_file(
            &mut file,
            self.buffer_size.0,
            self.buffer_size.1,
            self.main_frame.as_ref(),
            None,
            &mut self.main_buffer,
            &mut self.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return,
        };
        if let Some(id) = id
            && let Some(unit) = state.get_unit_with_id(id)
        {
            self.commit_surface(unit, damage);
            return;
        }
        self.commit_surface(state.main_window(), damage);
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
        let Some(file) = &self.buffer_file else {
            return None;
        };
        let Ok(mut file) = file.try_clone() else {
            return None;
        };
        let damage = match paint_frames_buffer_file(
            &mut file,
            self.buffer_size.0,
            self.buffer_size.1,
            &batch.frames,
            &[],
            &mut self.main_buffer,
            &mut self.scratch,
        ) {
            Ok(damage) => damage,
            Err(_) => return None,
        };
        match batch.surface {
            OsrSurface::Main => {
                self.main_frame = batch.frames.last().cloned();
                self.main_frame_surface_size = Some((batch.width, batch.height));
            }
            OsrSurface::Popup | OsrSurface::Guest(_) => {}
        }
        if !self.main_frame_ready() {
            return None;
        }
        self.restore_keyboard(state);
        self.force_resume("first-paint");
        if let Some(id) = id
            && let Some(unit) = state.get_unit_with_id(id)
        {
            self.commit_surface(unit, damage);
            return None;
        }
        self.commit_surface(state.main_window(), damage);
        None
    }

    pub(super) fn hide_surface(&mut self, state: &mut WindowState<()>) {
        if !self.surface_mapped {
            return;
        }
        let unit = state.main_window();
        self.ensure_layer_unit_size(unit);
        unit.set_keyboard_interactivity(keyboard_for_shell(
            ShellSurfaceKeyboardInteractivity::None,
        ));
        unit.get_wlsurface().attach(None, 0, 0);
        unit.get_wlsurface().commit();
        self.surface_mapped = false;
    }

    pub(super) fn main_frame_ready(&self) -> bool {
        self.main_frame.is_some() && self.main_frame_surface_size == Some(self.surface_size)
    }

    pub(super) fn clear_frames(&mut self) {
        self.main_frame = None;
        self.main_frame_surface_size = None;
        self.popup = None;
    }

    pub(super) fn release_hidden_frame_memory(&mut self) {
        self.clear_frames();
        self.main_buffer = Vec::new();
        self.scratch = Vec::new();
    }

    pub(super) fn commit_surface(
        &mut self,
        unit: &layershellev::WindowStateUnit<()>,
        damage: DamageRect,
    ) {
        self.ensure_layer_unit_size(unit);
        let damage = if self.surface_mapped && !self.pending_surface_refresh {
            damage
        } else {
            DamageRect::full(self.buffer_size.0, self.buffer_size.1)
        };
        let Some(buffer_index) = self.prepare_main_buffer() else {
            self.pending_surface_refresh = true;
            return;
        };
        let surface = unit.get_wlsurface();
        if self.main_buffers[buffer_index].attach_to(surface).is_err() {
            self.pending_surface_refresh = true;
            return;
        }
        surface.damage_buffer(
            damage.x as i32,
            damage.y as i32,
            damage.width as i32,
            damage.height as i32,
        );
        surface.commit();
        self.pending_surface_refresh = false;
        self.surface_mapped = true;
    }

    pub(super) fn commit_pending_surface(
        &mut self,
        state: &mut WindowState<()>,
        id: Option<layershellev::id::Id>,
    ) {
        if !self.pending_surface_refresh || !self.visible || !self.main_frame_ready() {
            return;
        }
        let damage = DamageRect::full(self.buffer_size.0, self.buffer_size.1);
        if let Some(id) = id
            && let Some(unit) = state.get_unit_with_id(id)
        {
            self.commit_surface(unit, damage);
            return;
        }
        self.commit_surface(state.main_window(), damage);
    }

    fn reset_main_pool(&mut self, shm: &wl_shm::WlShm, byte_len: usize) {
        self.main_buffers.clear();
        self.main_pool = SlotPool::new(byte_len.max(1), &Shm::from(shm.clone())).ok();
        self.pending_surface_refresh = false;
    }

    fn prepare_main_buffer(&mut self) -> Option<usize> {
        let pool = self.main_pool.as_mut()?;
        let pixels = self.main_buffer.as_slice();
        if pixels.len() != buffer_len(self.buffer_size.0, self.buffer_size.1) {
            return None;
        }

        for (index, buffer) in self.main_buffers.iter().enumerate() {
            if let Some(canvas) = buffer.canvas(pool) {
                canvas.copy_from_slice(pixels);
                return Some(index);
            }
        }

        if self.main_buffers.len() >= MAX_MAIN_BUFFERS {
            return None;
        }
        let (buffer, canvas) = pool
            .create_buffer(
                self.buffer_size.0 as i32,
                self.buffer_size.1 as i32,
                (self.buffer_size.0 * 4) as i32,
                wl_shm::Format::Argb8888,
            )
            .ok()?;
        canvas.copy_from_slice(pixels);
        self.main_buffers.push(buffer);
        Some(self.main_buffers.len() - 1)
    }

    fn ensure_layer_unit_size(&self, unit: &layershellev::WindowStateUnit<()>) {
        let (width, height) = self.layer_commit_size();
        unit.set_size((width, height));
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
        (width * 4) as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    )
}

pub(super) fn temporary_buffer_file() -> std::io::Result<File> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "sabine-layer-buffer-{}-{nanos}.shm",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let _ = std::fs::remove_file(path);
    Ok(file)
}
