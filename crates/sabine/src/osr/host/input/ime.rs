use winit::{
    dpi::{LogicalPosition, LogicalSize},
    event::Ime,
    window::{
        ImeCapabilities, ImeEnableRequest, ImeHint, ImePurpose, ImeRequest, ImeRequestData,
        ImeSurroundingText,
    },
};

use crate::osr::host::native::OsrNativeHost;
use crate::osr::host::types::{ImePreedit, ImeSurrounding};
use crate::osr::protocol::encode_component;

impl OsrNativeHost {
    pub(in crate::osr::host) fn forward_ime(&mut self, ime: Ime) {
        match ime {
            Ime::Enabled => {}
            Ime::Disabled => {
                self.ime_preedit = None;
                self.send_control("ime_cancel\n");
            }
            Ime::Commit(text) => {
                self.ime_preedit = None;
                let encoded = encode_component(&text);
                self.send_control(&format!("ime_commit\t{encoded}\n"));
            }
            Ime::Preedit(text, selection) => {
                if text.is_empty() {
                    self.ime_preedit = None;
                    self.send_control("ime_cancel\n");
                } else {
                    self.ime_preedit = Some(ImePreedit { text, selection });
                    self.send_preedit();
                }
            }
            Ime::DeleteSurrounding {
                before_bytes,
                after_bytes,
            } => self.delete_ime_surrounding(before_bytes, after_bytes),
            _ => {}
        }
    }

    fn send_preedit(&self) {
        let Some(preedit) = &self.ime_preedit else {
            return;
        };
        let encoded = encode_component(&preedit.text);
        let selection = preedit
            .selection
            .and_then(|(start, end)| {
                Some((
                    utf16_offset(&preedit.text, start)?,
                    utf16_offset(&preedit.text, end)?,
                ))
            })
            .map(|(start, end)| format!("\t{start}\t{end}"))
            .unwrap_or_default();
        self.send_control(&format!("ime_composition\t{encoded}{selection}\n"));
    }

    fn delete_ime_surrounding(&self, before_bytes: usize, after_bytes: usize) {
        let surrounding = &self.ime_surrounding;
        let selection_start = surrounding.cursor.min(surrounding.anchor);
        let selection_end = surrounding.cursor.max(surrounding.anchor);
        let start = selection_start.saturating_sub(before_bytes);
        let end = selection_end
            .saturating_add(after_bytes)
            .min(surrounding.text.len());
        let Some(start_utf16) = utf16_offset(&surrounding.text, start) else {
            return;
        };
        let Some(end_utf16) = utf16_offset(&surrounding.text, end) else {
            return;
        };
        self.send_control("ime_cancel\n");
        self.send_control(&format!(
            "ime_delete\t{}\t{}\n",
            surrounding.base_utf16 + start_utf16,
            surrounding.base_utf16 + end_utf16
        ));
        self.send_preedit();
    }

    pub(in crate::osr::host) fn update_ime_surrounding(
        &mut self,
        text: String,
        cursor_utf16: usize,
        anchor_utf16: usize,
        base_utf16: usize,
    ) {
        self.ime_surrounding =
            prepare_surrounding(text, cursor_utf16, anchor_utf16, base_utf16).unwrap_or_default();
        let Some(purpose) = ime_purpose(self.ime_mode) else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if let Err(error) =
            window.request_ime_update(ImeRequest::Update(self.ime_request_data(purpose)))
        {
            eprintln!("failed to update Sabine IME surrounding text: {error}");
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
            .with_cursor_area()
            .with_surrounding_text();
        let request_data = self.ime_request_data(purpose);
        let enable = ImeEnableRequest::new(capabilities, request_data)
            .expect("Sabine IME capabilities and initial data must match");
        if let Err(error) = window.request_ime_update(ImeRequest::Enable(enable)) {
            eprintln!("failed to enable Sabine IME: {error}");
        }
    }

    fn ime_request_data(&self, purpose: ImePurpose) -> ImeRequestData {
        let (x, y, width, height) = self.ime_cursor_area;
        let surrounding = ImeSurroundingText::new(
            self.ime_surrounding.text.clone(),
            self.ime_surrounding.cursor,
            self.ime_surrounding.anchor,
        )
        .expect("Sabine IME surrounding text is normalized before platform updates");
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
            .with_surrounding_text(surrounding)
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

fn byte_offset(text: &str, utf16_offset: usize) -> Option<usize> {
    let mut utf16 = 0;
    for (byte, character) in text.char_indices() {
        if utf16 == utf16_offset {
            return Some(byte);
        }
        utf16 += character.len_utf16();
        if utf16 > utf16_offset {
            return None;
        }
    }
    (utf16 == utf16_offset).then_some(text.len())
}

fn prepare_surrounding(
    text: String,
    cursor_utf16: usize,
    anchor_utf16: usize,
    mut base_utf16: usize,
) -> Option<ImeSurrounding> {
    let cursor = byte_offset(&text, cursor_utf16)?;
    let anchor = byte_offset(&text, anchor_utf16)?;
    if text.len() < 4000 {
        return Some(ImeSurrounding {
            text,
            cursor,
            anchor,
            base_utf16,
        });
    }
    let low = cursor.min(anchor);
    let high = cursor.max(anchor);
    if high - low >= 3999 {
        return Some(ImeSurrounding {
            base_utf16: base_utf16 + cursor_utf16,
            ..ImeSurrounding::default()
        });
    }
    let margin = (3999 - (high - low)) / 2;
    let mut start = low.saturating_sub(margin);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + 3999).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < high {
        end = high;
        start = end.saturating_sub(3999);
        while !text.is_char_boundary(start) {
            start += 1;
        }
    }
    base_utf16 += text[..start].encode_utf16().count();
    Some(ImeSurrounding {
        text: text[start..end].to_string(),
        cursor: cursor - start,
        anchor: anchor - start,
        base_utf16,
    })
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

    #[test]
    fn surrounding_text_offsets_round_trip_across_utf8_and_utf16() {
        let text = "aあ🦆z";
        for byte in [0, 1, 4, 8, text.len()] {
            let utf16 = utf16_offset(text, byte).unwrap();
            assert_eq!(byte_offset(text, utf16), Some(byte));
        }
        assert_eq!(byte_offset(text, 3), None);
    }

    #[test]
    fn long_surrounding_text_is_cropped_on_character_boundaries() {
        let text = format!("{}🦆{}", "a".repeat(2500), "b".repeat(2500));
        let cursor_utf16 = text[..2504].encode_utf16().count();
        let surrounding = prepare_surrounding(text, cursor_utf16, cursor_utf16, 7).unwrap();
        assert!(surrounding.text.len() < 4000);
        assert!(surrounding.text.is_char_boundary(surrounding.cursor));
        assert_eq!(surrounding.cursor, surrounding.anchor);
        assert!(surrounding.base_utf16 > 7);
    }
}
