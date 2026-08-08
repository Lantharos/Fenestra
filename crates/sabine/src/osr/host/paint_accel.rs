use crate::osr::host::types::{overlay_id_for_surface, overlay_texture_id};
use crate::osr::protocol::{MAIN_TEXTURE_ID, OsrAccelFrame, OsrFrame, OsrSurface};

use super::native::OsrNativeHost;

impl OsrNativeHost {
    pub(super) fn update_accel_frame(&mut self, frame: OsrAccelFrame) -> bool {
        if self.try_install_accel_texture(&frame) {
            self.accel_fallback.note_accel_ok();
            self.note_accel_surface(&frame);
            return true;
        }
        match crate::osr::accel::accel_to_paint_batch(frame) {
            Ok(batch) => {
                self.accel_fallback.note_accel_ok();
                self.update_paint_batch(batch)
            }
            Err(_) => {
                self.accel_fallback.note_accel_fail();
                if crate::osr::accel::should_relaunch_software(&self.accel_fallback) {
                    self.relaunch_software_osr();
                }
                false
            }
        }
    }

    fn try_install_accel_texture(&mut self, frame: &OsrAccelFrame) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return false;
        };
        let texture_id = match &frame.surface {
            OsrSurface::Main => MAIN_TEXTURE_ID.to_string(),
            OsrSurface::Popup | OsrSurface::Guest(_) => {
                let Some(overlay_id) = overlay_id_for_surface(&frame.surface) else {
                    return false;
                };
                overlay_texture_id(&overlay_id)
            }
        };

        #[cfg(target_os = "linux")]
        let imported = crate::osr::accel::try_import_dmabuf(renderer, frame);
        #[cfg(windows)]
        let imported = crate::osr::accel::try_import_d3d11(renderer, frame);
        #[cfg(target_os = "macos")]
        let imported = crate::osr::accel::try_import_iosurface(renderer, frame);
        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        let imported: Result<wgpu::Texture, String> = Err("accel import unsupported".into());

        match imported {
            Ok(texture) => {
                let installed = crate::osr::accel::install_imported_texture(
                    renderer,
                    &texture_id,
                    frame,
                    texture,
                )
                .is_ok();
                #[cfg(windows)]
                crate::osr::accel::close_imported_handle(frame.native_handle);
                installed
            }
            Err(error) => {
                #[cfg(windows)]
                crate::osr::accel::close_imported_handle(frame.native_handle);
                eprintln!("Sabine OSR: accelerated texture import failed: {error}");
                false
            }
        }
    }

    fn note_accel_surface(&mut self, frame: &OsrAccelFrame) {
        let stub = OsrFrame {
            surface: frame.surface.clone(),
            width: frame.width,
            height: frame.height,
            x: frame.x,
            y: frame.y,
            bytes: Vec::new(),
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
}
