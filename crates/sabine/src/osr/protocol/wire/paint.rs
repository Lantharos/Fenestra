use std::io;
use std::sync::Arc;

use super::header::{ReceivedFd, read_i32, read_u32, read_u64};
use super::shared_mem::SharedMapping;
use super::{
    BATCH_ENTRY_LEN, KIND_GUEST_BATCH, KIND_GUEST_SHARED_BATCH, KIND_MAIN_BATCH,
    KIND_MAIN_SHARED_BATCH, KIND_POPUP_SHARED_BATCH, MAX_PAINT_BYTES,
};
use crate::osr::protocol::{FrameBytes, OsrFrame, OsrPaintBatch, OsrSurface};

pub(super) fn parse_paint_batch(
    kind: u32,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    payload: Vec<u8>,
    received_fd: &mut ReceivedFd,
) -> io::Result<OsrPaintBatch> {
    let shared = matches!(
        kind,
        KIND_MAIN_SHARED_BATCH | KIND_POPUP_SHARED_BATCH | KIND_GUEST_SHARED_BATCH
    );
    let (surface, metadata_start) = if matches!(kind, KIND_GUEST_BATCH | KIND_GUEST_SHARED_BATCH) {
        let (guest_id, rest_start) = split_guest_payload(&payload)?;
        (OsrSurface::Guest(guest_id), rest_start)
    } else if matches!(kind, KIND_MAIN_BATCH | KIND_MAIN_SHARED_BATCH) {
        (OsrSurface::Main, 0)
    } else {
        (OsrSurface::Popup, 0)
    };

    let batch_payload = payload.get(metadata_start..).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid OSR paint batch metadata",
        )
    })?;
    let count = super::regions::payload_count(batch_payload)?;
    let entries_len = count.checked_mul(BATCH_ENTRY_LEN).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "OSR paint batch entry count overflow",
        )
    })?;
    let entries_end = 4_usize.checked_add(entries_len).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "OSR paint batch metadata overflow",
        )
    })?;
    let entries = batch_payload
        .get(4..entries_end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated OSR paint batch"))?
        .to_vec();

    let shared_source = if shared {
        let fd = received_fd.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR paint batch missing file descriptor",
            )
        })?;
        Some(SharedMapping::map_fd(fd, MAX_PAINT_BYTES)?)
    } else {
        None
    };
    let inline_source: Option<Arc<[u8]>> = (!shared).then(|| payload.into());
    let inline_blob_start = metadata_start + entries_end;
    let source_len = shared_source.as_ref().map_or_else(
        || inline_source.as_ref().map_or(0, |source| source.len()),
        |source| source.as_slice().len(),
    );

    let mut frames = Vec::with_capacity(count);
    for entry in entries.chunks_exact(BATCH_ENTRY_LEN) {
        let rect_x = read_i32(&entry[0..4]);
        let rect_y = read_i32(&entry[4..8]);
        let rect_width = read_u32(&entry[8..12]);
        let rect_height = read_u32(&entry[12..16]);
        let offset = read_u64(&entry[16..24]) as usize;
        let len = read_u32(&entry[24..28]) as usize;
        let expected_len = (rect_width as usize)
            .checked_mul(rect_height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "OSR paint rect size overflow")
            })?;
        if len != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid OSR paint rect byte length",
            ));
        }
        let source_offset = if shared {
            offset
        } else {
            inline_blob_start.checked_add(offset).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "OSR paint rect offset overflow")
            })?
        };
        let source_end = source_offset.checked_add(len).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "OSR paint rect range overflow")
        })?;
        if source_end > source_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated OSR paint rect bytes",
            ));
        }
        let bytes = if let Some(source) = &shared_source {
            FrameBytes::Shared {
                source: Arc::clone(source),
                range: source_offset..source_end,
            }
        } else if let Some(source) = &inline_source {
            FrameBytes::Inline {
                source: Arc::clone(source),
                range: source_offset..source_end,
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "OSR paint batch has no byte source",
            ));
        };
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

pub(super) fn split_guest_payload(payload: &[u8]) -> io::Result<(String, usize)> {
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
    Ok((id, id_end))
}
