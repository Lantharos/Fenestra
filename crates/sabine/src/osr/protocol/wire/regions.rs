use std::io;

use sabine_platform::WindowRegionRect;
use serde_json::Value;

use super::BATCH_ENTRY_LEN;
use super::header::{read_i32, read_u32};
use crate::osr::protocol::FileDragRequest;

pub(super) fn parse_draggable_regions(
    payload: &[u8],
) -> io::Result<(Vec<WindowRegionRect>, Vec<WindowRegionRect>)> {
    const ENTRY_LEN: usize = 20;
    let count =
        payload.get(0..4).map(read_u32).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "missing drag region count")
        })? as usize;
    let entries_end = count
        .checked_mul(ENTRY_LEN)
        .and_then(|len| len.checked_add(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid drag region count"))?;
    let entries = payload
        .get(4..entries_end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated drag regions"))?;
    let mut drag = Vec::new();
    let mut exclusion = Vec::new();
    for entry in entries.chunks_exact(ENTRY_LEN) {
        let rect = WindowRegionRect::new(
            read_i32(&entry[0..4]),
            read_i32(&entry[4..8]),
            read_i32(&entry[8..12]),
            read_i32(&entry[12..16]),
        );
        if rect.is_empty() {
            continue;
        }
        if read_u32(&entry[16..20]) != 0 {
            drag.push(rect);
        } else {
            exclusion.push(rect);
        }
    }
    Ok((drag, exclusion))
}

pub(super) fn parse_file_drag_request(payload: &[u8], x: i32, y: i32) -> Option<FileDragRequest> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    let paths = value
        .get("paths")?
        .as_array()?
        .iter()
        .filter_map(|item| item.as_str().map(String::from))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return None;
    }
    Some(FileDragRequest { paths, x, y })
}

pub(super) fn payload_count(payload: &[u8]) -> io::Result<usize> {
    let Some(count) = payload.get(0..4).map(read_u32) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated OSR paint batch header",
        ));
    };
    let count = count as usize;
    let entries_end = 4 + count * BATCH_ENTRY_LEN;
    if payload.len() < entries_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated OSR paint batch entries",
        ));
    }
    Ok(count)
}
