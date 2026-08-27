use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event::Ime,
    window::{ImeCapabilities, ImeEnableRequest, ImeHint, ImePurpose, ImeRequest, ImeRequestData},
};

use crate::osr::host::native::OsrNativeHost;
use crate::osr::protocol::encode_component;

impl OsrNativeHost {
    pub(in crate::osr::host) fn forward_ime(&self, ime: Ime) {
        match ime {
            Ime::Enabled => {}
            Ime::Disabled => self.send_control("ime_cancel\n"),
            Ime::Commit(text) => {
                let encoded = encode_component(&text);
                self.send_control(&format!("ime_commit\t{encoded}\n"));
            }
            Ime::Preedit(text, selection) => {
                if text.is_empty() {
                    self.send_control("ime_cancel\n");
                } else {
                    let encoded = encode_component(&text);
                    let selection = selection
                        .and_then(|(start, end)| {
                            Some((utf16_offset(&text, start)?, utf16_offset(&text, end)?))
                        })
                        .map(|(start, end)| format!("\t{start}\t{end}"))
                        .unwrap_or_default();
                    self.send_control(&format!("ime_composition\t{encoded}{selection}\n"));
                }
            }
            Ime::DeleteSurrounding {
                before_bytes,
                after_bytes,
            } => eprintln!(
                "Sabine OSR host received an unexpected IME surrounding-text deletion ({before_bytes} before, {after_bytes} after)"
            ),
            _ => {}
        }
    }

    pub(in crate::osr::host) fn update_ime_state(&mut self, mode: u32) {
        if self.ime_mode == mode {
            return;
        }
        let was_enabled = ime_purpose(self.ime_mode).is_some();
        self.ime_mode = mode;
        let Some(window) = self.window.as_ref() else {
            return;
        };
        match (was_enabled, ime_purpose(mode)) {
            (_, None) if was_enabled => {
                if let Err(error) = window.request_ime_update(ImeRequest::Disable) {
                    eprintln!("failed to disable Sabine IME: {error}");
                }
            }
            (true, Some(purpose)) => {
                let update = self.ime_request_data(purpose);
                if let Err(error) = window.request_ime_update(ImeRequest::Update(update)) {
                    eprintln!("failed to update Sabine IME: {error}");
                }
            }
            (false, Some(_)) => self.restore_ime_state(),
            _ => {}
        }
    }

    pub(in crate::osr::host) fn update_ime_cursor_area(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        self.ime_cursor_area = (x, y, width.max(1), height.max(1));
        let Some(purpose) = ime_purpose(self.ime_mode) else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let update = self.ime_request_data(purpose);
        if let Err(error) = window.request_ime_update(ImeRequest::Update(update)) {
            eprintln!("failed to position Sabine IME: {error}");
        }
    }

    pub(in crate::osr::host) fn restore_ime_state(&self) {
        let Some(purpose) = ime_purpose(self.ime_mode) else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let capabilities = ImeCapabilities::new()
            .with_hint_and_purpose()
            .with_cursor_area();
        let request_data = self.ime_request_data(purpose);
        let enable = ImeEnableRequest::new(capabilities, request_data)
            .expect("Sabine IME capabilities and initial data must match");
        if let Err(error) = window.request_ime_update(ImeRequest::Enable(enable)) {
            eprintln!("failed to enable Sabine IME: {error}");
        }
    }

    fn ime_request_data(&self, purpose: ImePurpose) -> ImeRequestData {
        let (x, y, width, height) = self.ime_cursor_area;
        ImeRequestData::default()
            .with_hint_and_purpose(ime_hint(self.ime_mode), purpose)
            .with_cursor_area(
                LogicalPosition::new(
                    f64::from(x),
                    f64::from(y) + f64::from(self.titlebar_height()),
                )
                .into(),
                LogicalSize::new(f64::from(width), f64::from(height)).into(),
            )
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

fn ime_purpose(mode: u32) -> Option<ImePurpose> {
    match mode {
        0 | 2 | 8 => Some(ImePurpose::Normal),
        3 => Some(ImePurpose::Phone),
        4 => Some(ImePurpose::Url),
        5 => Some(ImePurpose::Email),
        6 | 7 => Some(ImePurpose::Number),
        _ => None,
    }
}

fn ime_hint(mode: u32) -> ImeHint {
    match mode {
        2 => ImeHint::COMPLETION | ImeHint::SPELLCHECK,
        8 => ImeHint::COMPLETION,
        _ => ImeHint::NONE,
    }
}

fn utf16_offset(text: &str, byte_offset: usize) -> Option<usize> {
    text.get(..byte_offset)
        .map(|prefix| prefix.encode_utf16().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cef_input_modes_map_to_platform_purposes() {
        assert_eq!(ime_purpose(1), None);
        assert_eq!(ime_purpose(3), Some(ImePurpose::Phone));
        assert_eq!(ime_purpose(4), Some(ImePurpose::Url));
        assert_eq!(ime_purpose(5), Some(ImePurpose::Email));
        assert_eq!(ime_purpose(7), Some(ImePurpose::Number));
        assert_eq!(ime_purpose(99), None);
    }

    #[test]
    fn utf8_preedit_offsets_convert_to_cef_utf16_offsets() {
        let text = "aあ🦆";
        assert_eq!(utf16_offset(text, 0), Some(0));
        assert_eq!(utf16_offset(text, 1), Some(1));
        assert_eq!(utf16_offset(text, 4), Some(2));
        assert_eq!(utf16_offset(text, text.len()), Some(4));
        assert_eq!(utf16_offset(text, 2), None);
    }
}
