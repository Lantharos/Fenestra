use layershellev::{Ime, ImePurpose, WindowState, id};

use crate::osr::protocol::encode_component;

use super::types::OsrLayerHost;

impl OsrLayerHost {
    pub(super) fn forward_ime(&self, ime: &Ime) {
        match ime {
            Ime::Enabled => {}
            Ime::Disabled => self.send_control("ime_cancel\n"),
            Ime::Commit(text) => {
                self.send_control(&format!("ime_commit\t{}\n", encode_component(text)));
            }
            Ime::Preedit(text, _) if text.is_empty() => {
                self.send_control("ime_cancel\n");
            }
            Ime::Preedit(text, selection) => {
                let selection = selection
                    .and_then(|(start, end)| {
                        Some((utf16_offset(text, start)?, utf16_offset(text, end)?))
                    })
                    .map(|(start, end)| format!("\t{start}\t{end}"))
                    .unwrap_or_default();
                self.send_control(&format!(
                    "ime_composition\t{}{selection}\n",
                    encode_component(text)
                ));
            }
        }
    }

    pub(super) fn update_ime_state(&self, mode: u32, state: &mut WindowState<()>) {
        state.set_ime_purpose(ImePurpose::Normal);
        state.set_ime_allowed(mode != 1);
    }

    pub(super) fn update_ime_cursor_area(
        &self,
        state: &WindowState<()>,
        id: Option<id::Id>,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        let id = id
            .or_else(|| state.current_surface_id())
            .unwrap_or_else(|| state.main_window().id());
        state.set_ime_cursor_area(
            layershellev::dpi::LogicalPosition::new(x, y),
            layershellev::dpi::LogicalSize::new(width.max(1), height.max(1)),
            id,
        );
    }
}

fn utf16_offset(text: &str, byte_offset: usize) -> Option<usize> {
    text.get(..byte_offset)
        .map(|prefix| prefix.encode_utf16().count())
}
