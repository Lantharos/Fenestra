use winit::event::Ime;

use crate::osr::host::native::OsrNativeHost;
use crate::osr::protocol::encode_component;

impl OsrNativeHost {
    pub(in crate::osr::host) fn forward_ime(&self, ime: Ime) {
        match ime {
            Ime::Enabled | Ime::Disabled => {}
            Ime::Commit(text) => {
                let encoded = encode_component(&text);
                self.send_control(&format!("ime_commit\t{encoded}\n"));
            }
            Ime::Preedit(text, _) => {
                if text.is_empty() {
                    self.send_control("ime_cancel\n");
                } else {
                    let encoded = encode_component(&text);
                    self.send_control(&format!("ime_composition\t{encoded}\n"));
                }
            }
            _ => {}
        }
    }

    pub(in crate::osr::host) fn send_screen_origin(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Ok(position) = window.outer_position() else {
            return;
        };
        let titlebar = self.titlebar_height();
        let scale = window.scale_factor().max(1.0);
        let content_x = position.x;
        let content_y = position.y + (titlebar as f64 * scale).round() as i32;
        self.send_control(&format!("screen_origin\t{content_x}\t{content_y}\n"));
    }
}
