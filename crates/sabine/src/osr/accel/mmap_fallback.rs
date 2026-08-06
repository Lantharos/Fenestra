use std::io;

#[cfg(unix)]
use std::{os::fd::RawFd, ptr};

use crate::osr::protocol::{OsrAccelFrame, OsrFrame, OsrPaintBatch};

/// CEF `cef_color_type_t` values for packed 8-bit channels.
const CEF_COLOR_TYPE_RGBA_8888: u32 = 0;
const CEF_COLOR_TYPE_BGRA_8888: u32 = 1;

/// Map a DMA-BUF / memfd plane and convert dirty rects into a paint batch.
pub(crate) fn accel_to_paint_batch(frame: OsrAccelFrame) -> io::Result<OsrPaintBatch> {
    let mapped = map_plane(frame.fd, frame.size as usize, frame.offset)?;
    let bgra = unpack_plane(
        mapped.bytes(),
        frame.stride,
        frame.width,
        frame.height,
        frame.format,
    )?;
    drop(mapped);

    let mut frames = Vec::with_capacity(frame.dirty.len().max(1));
    if frame.dirty.is_empty() {
        frames.push(OsrFrame {
            surface: frame.surface.clone(),
            width: frame.width,
            height: frame.height,
            x: 0,
            y: 0,
            bytes: bgra,
        });
    } else {
        for rect in &frame.dirty {
            let Some(bytes) = copy_rect(&bgra, frame.width, frame.height, rect) else {
                continue;
            };
            frames.push(OsrFrame {
                surface: frame.surface.clone(),
                width: rect.width,
                height: rect.height,
                x: rect.x,
                y: rect.y,
                bytes,
            });
        }
    }

    Ok(OsrPaintBatch {
        surface: frame.surface,
        width: frame.width,
        height: frame.height,
        x: frame.x,
        y: frame.y,
        frames,
    })
}

struct MappedPlane {
    #[cfg(unix)]
    map_ptr: *mut libc::c_void,
    #[cfg(unix)]
    map_len: usize,
    #[cfg(unix)]
    data_offset: usize,
    #[cfg(unix)]
    data_len: usize,
    #[cfg(unix)]
    fd: RawFd,
    #[cfg(not(unix))]
    bytes: Vec<u8>,
}

impl MappedPlane {
    fn bytes(&self) -> &[u8] {
        #[cfg(unix)]
        {
            unsafe {
                std::slice::from_raw_parts(
                    (self.map_ptr as *const u8).add(self.data_offset),
                    self.data_len,
                )
            }
        }
        #[cfg(not(unix))]
        {
            &self.bytes
        }
    }
}

impl Drop for MappedPlane {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if !self.map_ptr.is_null() && self.map_len > 0 {
                unsafe {
                    libc::munmap(self.map_ptr, self.map_len);
                }
            }
            if self.fd >= 0 {
                unsafe {
                    libc::close(self.fd);
                }
            }
        }
    }
}

fn map_plane(fd: i32, size: usize, offset: u64) -> io::Result<MappedPlane> {
    #[cfg(unix)]
    {
        if fd < 0 || size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid accelerated plane",
            ));
        }
        let data_offset = offset as usize;
        let map_len = size.saturating_add(data_offset);
        let map_ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                map_len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map_ptr == libc::MAP_FAILED {
            let _ = unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }
        Ok(MappedPlane {
            map_ptr,
            map_len,
            data_offset,
            data_len: size,
            fd,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (fd, size, offset);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "accelerated mmap fallback is unavailable on this platform",
        ))
    }
}

fn unpack_plane(
    src: &[u8],
    stride: u32,
    width: u32,
    height: u32,
    format: u32,
) -> io::Result<Vec<u8>> {
    if width == 0 || height == 0 || stride == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid accelerated plane geometry",
        ));
    }
    let row_bytes = width as usize * 4;
    if (stride as usize) < row_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "accelerated plane stride too small",
        ));
    }
    let needed = stride as usize * height as usize;
    if src.len() < needed {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "accelerated plane truncated",
        ));
    }
    let src_is_rgba = match format {
        CEF_COLOR_TYPE_RGBA_8888 => true,
        CEF_COLOR_TYPE_BGRA_8888 => false,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported accelerated color format",
            ));
        }
    };
    let mut out = vec![0_u8; row_bytes * height as usize];
    for y in 0..height as usize {
        let src_row = &src[y * stride as usize..][..row_bytes];
        let dst_row = &mut out[y * row_bytes..][..row_bytes];
        if src_is_rgba {
            for x in 0..width as usize {
                let px = &src_row[x * 4..][..4];
                let out_px = &mut dst_row[x * 4..][..4];
                out_px[0] = px[2];
                out_px[1] = px[1];
                out_px[2] = px[0];
                out_px[3] = px[3];
            }
        } else {
            dst_row.copy_from_slice(src_row);
        }
    }
    Ok(out)
}

fn copy_rect(
    bgra: &[u8],
    frame_w: u32,
    frame_h: u32,
    rect: &crate::osr::protocol::OsrAccelRect,
) -> Option<Vec<u8>> {
    if rect.width == 0 || rect.height == 0 || rect.x < 0 || rect.y < 0 {
        return None;
    }
    let x = rect.x as u32;
    let y = rect.y as u32;
    if x >= frame_w || y >= frame_h {
        return None;
    }
    let w = rect.width.min(frame_w - x);
    let h = rect.height.min(frame_h - y);
    let mut out = vec![0_u8; (w * h * 4) as usize];
    let src_stride = (frame_w * 4) as usize;
    let dst_stride = (w * 4) as usize;
    for row in 0..h as usize {
        let src_off = (y as usize + row) * src_stride + x as usize * 4;
        let dst_off = row * dst_stride;
        out[dst_off..dst_off + dst_stride].copy_from_slice(&bgra[src_off..src_off + dst_stride]);
    }
    Some(out)
}
