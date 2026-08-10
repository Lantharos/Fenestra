mod accel;
mod header;
mod paint;
mod regions;
mod shared_mem;

pub(crate) use shared_mem::SharedMapping;

use crate::osr::transport::IpcStream;
use std::io::{self, Read};

use crate::osr::protocol::{OsrFrame, OsrMessage, OsrSurface};

use accel::{KIND_GUEST_ACCEL, KIND_MAIN_ACCEL, KIND_POPUP_ACCEL, parse_accel_frame};
use header::{read_header, read_i32, read_u32};
use paint::{parse_paint_batch, split_guest_payload};
use regions::{parse_draggable_regions, parse_file_drag_request};

pub(super) const HEADER_LEN: usize = 28;
pub(super) const MAGIC: &[u8; 4] = b"SAB1";
const MAX_SURFACE_DIMENSION: u32 = 16_384;
pub(super) const MAX_PAINT_BYTES: usize = 256 * 1024 * 1024;
const MAX_CONTROL_BYTES: usize = 16 * 1024 * 1024;
pub(super) const KIND_MAIN_FRAME: u32 = 1;
pub(super) const KIND_POPUP_FRAME: u32 = 2;
pub(super) const KIND_POPUP_HIDDEN: u32 = 3;
pub(super) const KIND_CURSOR: u32 = 4;
pub(super) const KIND_CLOSE_REQUESTED: u32 = 5;
pub(super) const KIND_START_DRAG_REQUESTED: u32 = 6;
pub(super) const KIND_MINIMIZE_REQUESTED: u32 = 7;
pub(super) const KIND_TOGGLE_MAXIMIZE_REQUESTED: u32 = 8;
pub(super) const KIND_SHOW_REQUESTED: u32 = 9;
pub(super) const KIND_HIDE_REQUESTED: u32 = 10;
pub(super) const KIND_FOCUS_REQUESTED: u32 = 11;
pub(super) const KIND_MAIN_BATCH: u32 = 12;
pub(super) const KIND_POPUP_BATCH: u32 = 13;
pub(super) const KIND_MAIN_SHARED_BATCH: u32 = 14;
pub(super) const KIND_POPUP_SHARED_BATCH: u32 = 15;
pub(super) const KIND_FILE_DRAG_REQUESTED: u32 = 16;
pub(super) const KIND_GUEST_FRAME: u32 = 17;
pub(super) const KIND_GUEST_BATCH: u32 = 18;
pub(super) const KIND_GUEST_SHARED_BATCH: u32 = 19;
pub(super) const KIND_GUEST_HIDDEN: u32 = 20;
pub(super) const KIND_DRAGGABLE_REGIONS_CHANGED: u32 = 21;
pub(super) const KIND_GUEST_CAPTURE_REQUESTED: u32 = 22;
pub(super) const KIND_BRIDGE_REQUEST: u32 = 23;
pub(super) const KIND_FULLSCREEN_REQUESTED: u32 = 27;
pub(super) const KIND_EXIT_FULLSCREEN_REQUESTED: u32 = 28;
pub(super) const BATCH_ENTRY_LEN: usize = 28;

pub(crate) fn read_message(reader: &mut IpcStream) -> io::Result<Option<OsrMessage>> {
    let Some((header, mut fd)) = read_header(reader)? else {
        return Ok(None);
    };
    if &header[0..4] != MAGIC {
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
    if width > MAX_SURFACE_DIMENSION || height > MAX_SURFACE_DIMENSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "OSR surface dimensions exceed the protocol limit",
        ));
    }
    if is_paint_kind(kind)
        && (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .is_none_or(|bytes| bytes > MAX_PAINT_BYTES)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "OSR surface exceeds the protocol byte limit",
        ));
    }
    let payload_limit = if is_paint_kind(kind) {
        MAX_PAINT_BYTES
    } else {
        MAX_CONTROL_BYTES
    };
    if payload_len > payload_limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "OSR payload exceeds the protocol limit",
        ));
    }
    let mut payload = vec![0_u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload)?;
    }

    let message = match kind {
        KIND_MAIN_FRAME | KIND_POPUP_FRAME => OsrMessage::Frame(OsrFrame {
            surface: if kind == KIND_MAIN_FRAME {
                OsrSurface::Main
            } else {
                OsrSurface::Popup
            },
            width,
            height,
            x,
            y,
            bytes: payload.into(),
        }),
        KIND_GUEST_FRAME => {
            let (guest_id, bytes_start) = split_guest_payload(&payload)?;
            let bytes = payload.split_off(bytes_start);
            OsrMessage::Frame(OsrFrame {
                surface: OsrSurface::Guest(guest_id),
                width,
                height,
                x,
                y,
                bytes: bytes.into(),
            })
        }
        KIND_MAIN_BATCH
        | KIND_POPUP_BATCH
        | KIND_GUEST_BATCH
        | KIND_MAIN_SHARED_BATCH
        | KIND_POPUP_SHARED_BATCH
        | KIND_GUEST_SHARED_BATCH => OsrMessage::PaintBatch(parse_paint_batch(
            kind, width, height, x, y, payload, &mut fd,
        )?),
        KIND_POPUP_HIDDEN => OsrMessage::PopupHidden,
        KIND_GUEST_HIDDEN => {
            OsrMessage::GuestHidden(String::from_utf8(payload).unwrap_or_default())
        }
        KIND_GUEST_CAPTURE_REQUESTED => {
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
            let (drag, exclusion) = parse_draggable_regions(&payload)?;
            OsrMessage::DraggableRegionsChanged { drag, exclusion }
        }
        KIND_CURSOR => OsrMessage::Cursor(String::from_utf8(payload).unwrap_or_default()),
        KIND_CLOSE_REQUESTED => OsrMessage::CloseRequested,
        KIND_START_DRAG_REQUESTED => OsrMessage::StartDragRequested,
        KIND_MINIMIZE_REQUESTED => OsrMessage::MinimizeRequested,
        KIND_TOGGLE_MAXIMIZE_REQUESTED => OsrMessage::ToggleMaximizeRequested,
        KIND_FULLSCREEN_REQUESTED => OsrMessage::FullscreenRequested(true),
        KIND_EXIT_FULLSCREEN_REQUESTED => OsrMessage::FullscreenRequested(false),
        KIND_SHOW_REQUESTED => OsrMessage::ShowRequested,
        KIND_HIDE_REQUESTED => OsrMessage::HideRequested,
        KIND_FOCUS_REQUESTED => OsrMessage::FocusRequested(
            String::from_utf8(payload)
                .ok()
                .filter(|token| !token.trim().is_empty()),
        ),
        KIND_FILE_DRAG_REQUESTED => match parse_file_drag_request(&payload, x, y) {
            Some(request) => OsrMessage::FileDragRequested(request),
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid file drag request payload",
                ));
            }
        },
        KIND_BRIDGE_REQUEST => {
            OsrMessage::BridgeRequest(String::from_utf8(payload).unwrap_or_default())
        }
        KIND_MAIN_ACCEL | KIND_POPUP_ACCEL | KIND_GUEST_ACCEL => {
            OsrMessage::AccelFrame(parse_accel_frame(kind, width, height, x, y, &payload)?)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown OSR message kind",
            ));
        }
    };
    Ok(Some(message))
}

fn is_paint_kind(kind: u32) -> bool {
    matches!(
        kind,
        KIND_MAIN_FRAME
            | KIND_POPUP_FRAME
            | KIND_MAIN_BATCH
            | KIND_POPUP_BATCH
            | KIND_MAIN_SHARED_BATCH
            | KIND_POPUP_SHARED_BATCH
            | KIND_GUEST_FRAME
            | KIND_GUEST_BATCH
            | KIND_GUEST_SHARED_BATCH
            | KIND_MAIN_ACCEL
            | KIND_POPUP_ACCEL
            | KIND_GUEST_ACCEL
    )
}

mod tests {
    #[cfg(unix)]
    #[test]
    fn inline_paint_batch_parses_multiple_rects() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        use super::{HEADER_LEN, KIND_MAIN_BATCH, MAGIC, OsrMessage, OsrSurface, read_message};

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
        assert_eq!(batch.frames[1].bytes(), &[2, 2, 2, 255]);
    }

    #[cfg(unix)]
    #[test]
    fn oversized_surface_is_rejected_before_payload_allocation() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        use super::{HEADER_LEN, KIND_MAIN_FRAME, MAGIC, read_message};

        let (mut reader, mut writer) = UnixStream::pair().expect("socket pair");
        let mut header = [0_u8; HEADER_LEN];
        header[0..4].copy_from_slice(MAGIC);
        header[4..8].copy_from_slice(&KIND_MAIN_FRAME.to_le_bytes());
        header[8..12].copy_from_slice(&16_384_u32.to_le_bytes());
        header[12..16].copy_from_slice(&16_384_u32.to_le_bytes());
        writer.write_all(&header).expect("header");

        let error = read_message(&mut reader).expect_err("surface must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn draggable_regions_split_drag_and_exclusion_rects() {
        use sabine_platform::WindowRegionRect;

        use super::parse_draggable_regions;

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
}
