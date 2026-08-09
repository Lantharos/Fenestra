use crate::osr::transport::IpcStream;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;

use super::HEADER_LEN;

pub(super) fn read_header(
    reader: &mut IpcStream,
) -> io::Result<Option<([u8; HEADER_LEN], ReceivedFd)>> {
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
    Ok(Some((header, ReceivedFd(fd))))
}

pub(super) struct ReceivedFd(Option<i32>);

impl ReceivedFd {
    pub(super) fn take(&mut self) -> Option<i32> {
        self.0.take()
    }
}

impl Drop for ReceivedFd {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(fd) = self.0.take() {
            unsafe {
                libc::close(fd);
            }
        }
    }
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

pub(super) fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

pub(super) fn read_i32(bytes: &[u8]) -> i32 {
    i32::from_le_bytes(bytes.try_into().expect("slice length checked"))
}

pub(super) fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("slice length checked"))
}
