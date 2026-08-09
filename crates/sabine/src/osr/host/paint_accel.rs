#[cfg(windows)]
use crate::osr::host::types::{overlay_id_for_surface, overlay_texture_id};
use crate::osr::protocol::OsrAccelFrame;
#[cfg(windows)]
use crate::osr::protocol::{MAIN_TEXTURE_ID, OsrFrame, OsrSurface};
#[cfg(windows)]
use std::io::Write;

use super::native::OsrNativeHost;

impl OsrNativeHost {
    pub(super) fn update_accel_frame(&mut self, frame: OsrAccelFrame) -> bool {
        #[cfg(windows)]
        {
            if self.try_install_accel_texture(&frame) {
                self.note_accel_surface(&frame);
                return true;
            }
            false
        }
        #[cfg(not(windows))]
        {
            crate::osr::accel::discard_frame(frame);
            false
        }
    }

    #[cfg(windows)]
    fn try_install_accel_texture(&mut self, frame: &OsrAccelFrame) -> bool {
        let release_socket = self.socket.clone();
        let slot_token = frame.slot_token;
        let release_slot = move || {
            let Some(socket) = release_socket else {
                return;
            };
            if let Ok(mut socket) = socket.lock() {
                let _ = writeln!(socket, "accel_release\t{slot_token}");
                let _ = socket.flush();
            }
        };
        if frame.surface == OsrSurface::Main {
            let frame_size = self.accel_frame_size(frame);
            let target = self.content_surface_size();
            if !self.should_accept_main_frame_size(frame_size, target) {
                crate::osr::accel::close_imported_handle(frame.native_handle);
                release_slot();
                self.retry_resize_paint();
                return false;
            }
        }
        let Some(renderer) = self.renderer.as_mut() else {
            crate::osr::accel::close_imported_handle(frame.native_handle);
            release_slot();
            return false;
        };
        let texture_id = match &frame.surface {
            OsrSurface::Main => MAIN_TEXTURE_ID.to_string(),
            OsrSurface::Popup | OsrSurface::Guest(_) => {
                let Some(overlay_id) = overlay_id_for_surface(&frame.surface) else {
                    crate::osr::accel::close_imported_handle(frame.native_handle);
                    release_slot();
                    return false;
                };
                overlay_texture_id(&overlay_id)
            }
        };

        let imported = crate::osr::accel::try_import_d3d12(renderer, frame);
        let installed = match imported {
            Ok(texture) => crate::osr::accel::copy_imported_texture(
                renderer,
                &texture_id,
                frame,
                texture,
                release_slot,
            )
            .is_ok(),
            Err(error) => {
                eprintln!("Sabine OSR: D3D12 texture import failed: {error}");
                release_slot();
                false
            }
        };
        crate::osr::accel::close_imported_handle(frame.native_handle);
        installed
    }

    #[cfg(windows)]
    fn note_accel_surface(&mut self, frame: &OsrAccelFrame) {
        let (width, height) = self.accel_frame_size(frame);
        let stub = OsrFrame {
            surface: frame.surface.clone(),
            width,
            height,
            x: frame.x,
            y: frame.y,
            bytes: Vec::new().into(),
        };
        match &frame.surface {
            OsrSurface::Main => {
                self.main_frame = Some(stub);
                self.clear_pending_resize_paint();
            }
            OsrSurface::Popup | OsrSurface::Guest(_) => {
                let Some(overlay_id) = overlay_id_for_surface(&frame.surface) else {
                    return;
                };
                let entry =
                    self.overlays
                        .entry(overlay_id)
                        .or_insert_with(|| super::types::OverlayLayer {
                            frame: stub.clone(),
                            buffer: crate::osr::frame_buffer::FrameBuffer::new(),
                        });
                entry.frame = stub;
            }
        }
    }

    #[cfg(windows)]
    fn accel_frame_size(&self, frame: &OsrAccelFrame) -> (u32, u32) {
        self.frame_size_for_view((frame.visible_width, frame.visible_height))
    }
}
