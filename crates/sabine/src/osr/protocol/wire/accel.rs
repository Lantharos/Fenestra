use super::header::{read_u32, read_u64};
use super::paint::split_guest_payload;
use crate::osr::protocol::{OsrAccelFrame, OsrSurface};
use std::io;

pub(super) const KIND_MAIN_ACCEL: u32 = 24;
pub(super) const KIND_POPUP_ACCEL: u32 = 25;
pub(super) const KIND_GUEST_ACCEL: u32 = 26;

const META_LEN: usize = 4 + 8 + 8;

pub(super) fn parse_accel_frame(
    kind: u32,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    payload: &[u8],
) -> io::Result<OsrAccelFrame> {
    let (surface, rest_start) = match kind {
        KIND_GUEST_ACCEL => {
            let (guest_id, rest) = split_guest_payload(payload)?;
            (OsrSurface::Guest(guest_id), rest)
        }
        KIND_MAIN_ACCEL => (OsrSurface::Main, 0),
        KIND_POPUP_ACCEL => (OsrSurface::Popup, 0),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown accelerated OSR kind",
            ));
        }
    };

    let rest = &payload[rest_start..];
    if rest.len() != META_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid accelerated OSR metadata length",
        ));
    }

    let format = read_u32(&rest[0..4]);
    let native_handle = read_u64(&rest[4..12]);
    let slot_token = read_u64(&rest[12..20]);

    Ok(OsrAccelFrame {
        surface,
        width,
        height,
        x,
        y,
        format,
        native_handle,
        slot_token,
    })
}
