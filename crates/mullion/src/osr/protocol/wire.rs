use crate::osr::transport::IpcStream;
use std::io::{self, Read};
#[cfg(unix)]
use std::{
    fs::File,
    io::{Seek, SeekFrom},
    os::fd::{AsRawFd, FromRawFd},
};

use mullion_platform::WindowRegionRect;
use serde_json::Value;

use super::{FileDragRequest, OsrFrame, OsrMessage, OsrPaintBatch, OsrSurface};

const HEADER_LEN: usize = 28;
const MAGIC: &[u8; 4] = b"MLON";
const KIND_MAIN_FRAME: u32 = 1;
const KIND_POPUP_FRAME: u32 = 2;
const KIND_POPUP_HIDDEN: u32 = 3;
const KIND_CURSOR: u32 = 4;
const KIND_CLOSE_REQUESTED: u32 = 5;
const KIND_START_DRAG_REQUESTED: u32 = 6;
const KIND_MINIMIZE_REQUESTED: u32 = 7;
const KIND_TOGGLE_MAXIMIZE_REQUESTED: u32 = 8;
const KIND_SHOW_REQUESTED: u32 = 9;
const KIND_HIDE_REQUESTED: u32 = 10;
const KIND_FOCUS_REQUESTED: u32 = 11;
const KIND_MAIN_BATCH: u32 = 12;
const KIND_POPUP_BATCH: u32 = 13;
const KIND_MAIN_SHARED_BATCH: u32 = 14;
const KIND_POPUP_SHARED_BATCH: u32 = 15;
const KIND_FILE_DRAG_REQUESTED: u32 = 16;
const KIND_GUEST_FRAME: u32 = 17;
const KIND_GUEST_BATCH: u32 = 18;
const KIND_GUEST_SHARED_BATCH: u32 = 19;
const KIND_GUEST_HIDDEN: u32 = 20;
const KIND_DRAGGABLE_REGIONS_CHANGED: u32 = 21;
const KIND_GUEST_CAPTURE_REQUESTED: u32 = 22;
const KIND_BRIDGE_REQUEST: u32 = 23;
const BATCH_ENTRY_LEN: usize = 28;

pub(crate) fn read_message(reader: &mut IpcStream) -> io::Result<Option<OsrMessage>> {
    let Some((header, fd)) = read_header(reader)? else {
        return Ok(None);
    };
    if &header[0..4] != MAGIC {
        close_optional_fd(fd);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid OSR message magic",
        ));
    }

    let kind = read_u32(&header[4..8]);
    let width = read_u32(&header[8..12]);
    let height = read_u32(&header[12..16]);
    let x = read_i32(&header[16..20]);
    let y = read_i32(&header[20..24]);
    let payload_len = read_u32(&header[24..28]) as usize;
    let mut payload = vec![0_u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload)?;
    }

    let message = match kind {
        KIND_MAIN_FRAME | KIND_POPUP_FRAME => {
            close_optional_fd(fd);
            OsrMessage::Frame(OsrFrame {
                surface: if kind == KIND_MAIN_FRAME {
                    OsrSurface::Main
                } else {
                    OsrSurface::Popup
                },
                width,
                height,
                x,
                y,
                bytes: payload,
            })
        }
        KIND_GUEST_FRAME => {
            close_optional_fd(fd);
            let (guest_id, bytes) = split_guest_payload(&payload)?;
            OsrMessage::Frame(OsrFrame {
                surface: OsrSurface::Guest(guest_id),
                width,
                height,
                x,
                y,
                bytes,
            })
        }
        KIND_MAIN_BATCH | KIND_POPUP_BATCH => {
            close_optional_fd(fd);
            OsrMessage::PaintBatch(parse_paint_batch(
                kind, width, height, x, y, &payload, None,
            )?)
        }
        KIND_GUEST_BATCH => {
            close_optional_fd(fd);
            OsrMessage::PaintBatch(parse_paint_batch(
                kind, width, height, x, y, &payload, None,
            )?)
        }
        KIND_MAIN_SHARED_BATCH | KIND_POPUP_SHARED_BATCH | KIND_GUEST_SHARED_BATCH => {
            OsrMessage::PaintBatch(parse_paint_batch(kind, width, height, x, y, &payload, fd)?)
        }
        KIND_POPUP_HIDDEN => {
            close_optional_fd(fd);
            OsrMessage::PopupHidden
        }
        KIND_GUEST_HIDDEN => {
            close_optional_fd(fd);
            OsrMessage::GuestHidden(String::from_utf8(payload).unwrap_or_default())
        }
        KIND_GUEST_CAPTURE_REQUESTED => {
            close_optional_fd(fd);
            let mut parts = payload.splitn(3, |byte| *byte == 0);
            let browser_id =
                String::from_utf8(parts.next().unwrap_or_default().to_vec()).unwrap_or_default();
            let request_id =
                String::from_utf8(parts.next().unwrap_or_default().to_vec()).unwrap_or_default();
            let guest_id =
                String::from_utf8(parts.next().unwrap_or_default().to_vec()).unwrap_or_default();
            if browser_id.is_empty() || request_id.is_empty() || guest_id.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid guest capture request",
                ));
            }
            OsrMessage::GuestCaptureRequested {
                browser_id,
                request_id,
                guest_id,
            }
        }
        KIND_DRAGGABLE_REGIONS_CHANGED => {
            close_optional_fd(fd);
            let (drag, exclusion) = parse_draggable_regions(&payload)?;
            OsrMessage::DraggableRegionsChanged { drag, exclusion }
        }
        KIND_CURSOR => {
            close_optional_fd(fd);
            OsrMessage::Cursor(String::from_utf8(payload).unwrap_or_default())
        }
        KIND_CLOSE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::CloseRequested
        }
        KIND_START_DRAG_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::StartDragRequested
        }
        KIND_MINIMIZE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::MinimizeRequested
        }
        KIND_TOGGLE_MAXIMIZE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::ToggleMaximizeRequested
        }
        KIND_SHOW_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::ShowRequested
        }
        KIND_HIDE_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::HideRequested
        }
        KIND_FOCUS_REQUESTED => {
            close_optional_fd(fd);
            OsrMessage::FocusRequested
        }
        KIND_FILE_DRAG_REQUESTED => {
            close_optional_fd(fd);
            match parse_file_drag_request(&payload, x, y) {
                Some(request) => OsrMessage::FileDragRequested(request),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid file drag request payload",
                    ));
                }
            }
        }
        KIND_BRIDGE_REQUEST => {
            close_optional_fd(fd);
            OsrMessage::BridgeRequest(String::from_utf8(payload).unwrap_or_default())
        }
        _ => {
            close_optional_fd(fd);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown OSR message kind",
            ));
        }
    };
    Ok(Some(message))
}

fn read_header(reader: &mut IpcStream) -> io::Result<Option<([u8; HEADER_LEN], Option<i32>)>> {
    let mut header = [0_u8; HEADER_LEN];
    let mut filled = 0;
    let mut fd = None;
    while filled < HEADER_LEN {
        if filled == 0 {
            match recv_header_start(reader, &mut header)? {
                Some((read, received_fd)) => {
                    filled = read.min(HEADER_LEN);
                    fd = received_fd;
                }
                None => return Ok(None),
            }
        } else {
            match reader.read_exact(&mut header[filled..]) {
                Ok(()) => filled = HEADER_LEN,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(Some((header, fd)))
}

#[cfg(unix)]
fn recv_header_start(
    reader: &IpcStream,
    header: &mut [u8; HEADER_LEN],
) -> io::Result<Option<(usize, Option<i32>)>> {
    let mut iov = libc::iovec {
        iov_base: header.as_mut_ptr().cast(),
        iov_len: HEADER_LEN,
    };
    let mut control = [0_u8; 64];
    let mut message = libc::msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len() as _,
        msg_flags: 0,
    };
    let result = unsafe { libc::recvmsg(reader.as_raw_fd(), &mut message, 0) };
    if result == 0 {
        return Ok(None);
    }
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { received_fd(&message) };
    Ok(Some((result as usize, fd)))
}

#[cfg(not(unix))]
fn recv_header_start(
    reader: &IpcStream,
    header: &mut [u8; HEADER_LEN],
) -> io::Result<Option<(usize, Option<i32>)>> {
    let mut reader = reader;
    match reader.read(header) {
        Ok(0) => Ok(None),
        Ok(read) => Ok(Some((read, None))),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
unsafe fn received_fd(message: &libc::msghdr) -> Option<i32> {
    let mut control = unsafe { libc::CMSG_FIRSTHDR(message) };
    while !control.is_null() {
        let header = unsafe { &*control };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            return Some(unsafe { *(libc::CMSG_DATA(control).cast::<i32>()) });
        }
        control = unsafe { libc::CMSG_NXTHDR(message, control) };
    }
    None
}

fn parse_paint_batch(
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
    let source_bytes;
    let source = if shared {
        let fd = fd.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR paint batch missing file descriptor",
            )
        })?;
        source_bytes = read_shared_bytes(fd)?;
        source_bytes.as_slice()
    } else {
        close_optional_fd(fd);
        let count = payload_count(&batch_payload)?;
        let blob_start = 4 + count * BATCH_ENTRY_LEN;
        batch_payload
            .get(blob_start..)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid OSR paint batch"))?
    };
    let count = payload_count(&batch_payload)?;
    let entries_end = 4 + count * BATCH_ENTRY_LEN;
    let entries = batch_payload
        .get(4..entries_end)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated OSR paint batch"))?;
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
        let bytes = source
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

fn split_guest_payload(payload: &[u8]) -> io::Result<(String, Vec<u8>)> {
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

fn parse_draggable_regions(
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

fn parse_file_drag_request(payload: &[u8], x: i32, y: i32) -> Option<FileDragRequest> {
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

fn payload_count(payload: &[u8]) -> io::Result<usize> {
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

#[cfg(unix)]
fn read_shared_bytes(fd: i32) -> io::Result<Vec<u8>> {
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_shared_bytes(_fd: i32) -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "shared OSR buffers are unavailable on this transport",
    ))
}

fn close_optional_fd(fd: Option<i32>) {
    #[cfg(unix)]
    if let Some(fd) = fd {
        unsafe {
            libc::close(fd);
        }
    }
    #[cfg(not(unix))]
    let _ = fd;
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

fn read_i32(bytes: &[u8]) -> i32 {
    i32::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    #[cfg(unix)]
    #[test]
    fn inline_paint_batch_parses_multiple_rects() {
        let (mut reader, mut writer) = UnixStream::pair().expect("socket pair");
        let mut payload = Vec::new();
        payload.extend_from_slice(&2_u32.to_le_bytes());
        push_entry(&mut payload, 0, 0, 1, 1, 0, 4);
        push_entry(&mut payload, 2, 1, 1, 1, 4, 4);
        payload.extend_from_slice(&[1, 1, 1, 255, 2, 2, 2, 255]);
        let mut header = vec![0_u8; HEADER_LEN];
        header[0..4].copy_from_slice(MAGIC);
        header[4..8].copy_from_slice(&KIND_MAIN_BATCH.to_le_bytes());
        header[8..12].copy_from_slice(&3_u32.to_le_bytes());
        header[12..16].copy_from_slice(&2_u32.to_le_bytes());
        header[24..28].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        writer.write_all(&header).expect("header");
        writer.write_all(&payload).expect("payload");

        let message = read_message(&mut reader).expect("read").expect("message");
        let OsrMessage::PaintBatch(batch) = message else {
            panic!("expected paint batch");
        };
        assert_eq!(batch.surface, OsrSurface::Main);
        assert_eq!((batch.width, batch.height), (3, 2));
        assert_eq!(batch.frames.len(), 2);
        assert_eq!((batch.frames[1].x, batch.frames[1].y), (2, 1));
        assert_eq!(batch.frames[1].bytes, vec![2, 2, 2, 255]);
    }

    #[test]
    fn draggable_regions_split_drag_and_exclusion_rects() {
        let mut payload = 2_u32.to_le_bytes().to_vec();
        for (x, y, width, height, draggable) in
            [(0_i32, 0_i32, 600_i32, 38_i32, 1_u32), (520, 0, 80, 38, 0)]
        {
            payload.extend_from_slice(&x.to_le_bytes());
            payload.extend_from_slice(&y.to_le_bytes());
            payload.extend_from_slice(&width.to_le_bytes());
            payload.extend_from_slice(&height.to_le_bytes());
            payload.extend_from_slice(&draggable.to_le_bytes());
        }

        let (drag, exclusion) = parse_draggable_regions(&payload).expect("regions");
        assert_eq!(drag, vec![WindowRegionRect::new(0, 0, 600, 38)]);
        assert_eq!(exclusion, vec![WindowRegionRect::new(520, 0, 80, 38)]);
    }

    fn push_entry(
        payload: &mut Vec<u8>,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        offset: u64,
        len: u32,
    ) {
        payload.extend_from_slice(&x.to_le_bytes());
        payload.extend_from_slice(&y.to_le_bytes());
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&height.to_le_bytes());
        payload.extend_from_slice(&offset.to_le_bytes());
        payload.extend_from_slice(&len.to_le_bytes());
    }
}
