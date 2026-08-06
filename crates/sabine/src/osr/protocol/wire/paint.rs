use std::io;
#[cfg(unix)]
use std::os::fd::FromRawFd;
use std::sync::Arc;

use super::header::{close_optional_fd, read_i32, read_u32, read_u64};
use super::shared_mem::SharedMapping;
use super::{
    BATCH_ENTRY_LEN, KIND_GUEST_BATCH, KIND_GUEST_SHARED_BATCH, KIND_MAIN_BATCH,
    KIND_MAIN_SHARED_BATCH, KIND_POPUP_SHARED_BATCH,
};
use crate::osr::protocol::{OsrFrame, OsrPaintBatch, OsrSurface};

enum PaintSource {
    Mapped(Arc<SharedMapping>),
    Inline(Vec<u8>),
}

impl PaintSource {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Mapped(mapping) => mapping.as_slice(),
            Self::Inline(bytes) => bytes.as_slice(),
        }
    }
}

pub(super) fn parse_paint_batch(
    kind: u32,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    payload: &[u8],
    fd: Option<i32>,
) -> io::Result<OsrPaintBatch> {
    let shared = matches!(
        kind,
        KIND_MAIN_SHARED_BATCH | KIND_POPUP_SHARED_BATCH | KIND_GUEST_SHARED_BATCH
    );
    let (surface, batch_payload) = if matches!(kind, KIND_GUEST_BATCH | KIND_GUEST_SHARED_BATCH) {
        let (guest_id, rest) = split_guest_payload(payload)?;
        (OsrSurface::Guest(guest_id), rest)
    } else if matches!(kind, KIND_MAIN_BATCH | KIND_MAIN_SHARED_BATCH) {
        (OsrSurface::Main, payload.to_vec())
    } else {
        (OsrSurface::Popup, payload.to_vec())
    };

    let count = super::regions::payload_count(&batch_payload)?;
    let entries_end = 4 + count * BATCH_ENTRY_LEN;
    let entries = batch_payload
        .get(4..entries_end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated OSR paint batch"))?
        .to_vec();

    let source = if shared {
        let fd = fd.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR paint batch missing file descriptor",
            )
        })?;
        PaintSource::Mapped(SharedMapping::map_fd(fd)?)
    } else {
        close_optional_fd(fd);
        let blob_start = 4 + count * BATCH_ENTRY_LEN;
        let bytes = batch_payload
            .get(blob_start..)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid OSR paint batch"))?
            .to_vec();
        PaintSource::Inline(bytes)
    };
    let source_bytes = source.as_slice();

    let mut frames = Vec::with_capacity(count);
    for entry in entries.chunks_exact(BATCH_ENTRY_LEN) {
        let rect_x = read_i32(&entry[0..4]);
        let rect_y = read_i32(&entry[4..8]);
        let rect_width = read_u32(&entry[8..12]);
        let rect_height = read_u32(&entry[12..16]);
        let offset = read_u64(&entry[16..24]) as usize;
        let len = read_u32(&entry[24..28]) as usize;
        let expected_len = rect_width as usize * rect_height as usize * 4;
        if len != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid OSR paint rect byte length",
            ));
        }
        let bytes = source_bytes
            .get(offset..offset + len)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "truncated OSR paint rect bytes")
            })?
            .to_vec();
        frames.push(OsrFrame {
            surface: surface.clone(),
            width: rect_width,
            height: rect_height,
            x: rect_x,
            y: rect_y,
            bytes,
        });
    }
    Ok(OsrPaintBatch {
        surface,
        width,
        height,
        x,
        y,
        frames,
    })
}

pub(super) fn split_guest_payload(payload: &[u8]) -> io::Result<(String, Vec<u8>)> {
    if payload.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "guest OSR payload missing id length",
        ));
    }
    let id_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    let id_end = 2 + id_len;
    if payload.len() < id_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "guest OSR payload missing id bytes",
        ));
    }
    let id = String::from_utf8_lossy(&payload[2..id_end]).into_owned();
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "guest OSR payload has empty id",
        ));
    }
    Ok((id, payload[id_end..].to_vec()))
}

#[cfg(unix)]
#[allow(dead_code)]
pub(super) fn read_shared_bytes(fd: i32) -> io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub(super) fn read_shared_bytes(_fd: i32) -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "shared OSR buffers are unavailable on this transport",
    ))
}
