use crate::osr::protocol::OsrAccelFrame;

use super::native::OsrNativeHost;

impl OsrNativeHost {
    pub(super) fn update_accel_frame(&mut self, frame: OsrAccelFrame) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(renderer) = self.renderer.as_mut() {
            if crate::osr::accel::try_import_dmabuf(renderer, &frame).is_ok() {
                self.accel_fallback.note_accel_ok();
                return true;
            }
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
}
