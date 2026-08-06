use std::io;
#[cfg(unix)]
use std::os::fd::RawFd;

use super::header::{close_optional_fd, read_i32, read_u32, read_u64};
use super::paint::split_guest_payload;
use crate::osr::protocol::{OsrAccelFrame, OsrAccelRect, OsrSurface};

pub(super) const KIND_MAIN_ACCEL: u32 = 24;
pub(super) const KIND_POPUP_ACCEL: u32 = 25;
pub(super) const KIND_GUEST_ACCEL: u32 = 26;

pub(super) fn parse_accel_frame(
    kind: u32,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    payload: &[u8],
    fd: Option<i32>,
) -> io::Result<OsrAccelFrame> {
    let fd = fd.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "accelerated OSR frame missing file descriptor",
        )
    })?;

    let (surface, rest) = match kind {
        KIND_GUEST_ACCEL => {
            let (guest_id, rest) = split_guest_payload(payload)?;
            (OsrSurface::Guest(guest_id), rest)
        }
        KIND_MAIN_ACCEL => (OsrSurface::Main, payload.to_vec()),
        KIND_POPUP_ACCEL => (OsrSurface::Popup, payload.to_vec()),
        _ => {
            close_optional_fd(Some(fd));
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown accelerated OSR kind",
            ));
        }
    };

    if rest.len() < 4 + 8 + 4 + 8 + 8 + 4 {
        close_optional_fd(Some(fd));
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated accelerated OSR metadata",
        ));
    }

    let format = read_u32(&rest[0..4]);
    let modifier = read_u64(&rest[4..12]);
    let stride = read_u32(&rest[12..16]);
    let offset = read_u64(&rest[16..24]);
    let size = read_u64(&rest[24..32]);
    let count = read_u32(&rest[32..36]) as usize;
    let rects_end = 36 + count * 16;
    if rest.len() < rects_end {
        close_optional_fd(Some(fd));
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated accelerated OSR dirty rects",
        ));
    }
    let mut dirty = Vec::with_capacity(count);
    for entry in rest[36..rects_end].chunks_exact(16) {
        dirty.push(OsrAccelRect {
            x: read_i32(&entry[0..4]),
            y: read_i32(&entry[4..8]),
            width: read_u32(&entry[8..12]),
            height: read_u32(&entry[12..16]),
        });
    }

    Ok(OsrAccelFrame {
        surface,
        width,
        height,
        x,
        y,
        format,
        modifier,
        stride,
        offset,
        size,
        dirty,
        #[cfg(unix)]
        fd: fd as RawFd,
        #[cfg(not(unix))]
        fd: -1,
    })
}
