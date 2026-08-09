#[cfg(unix)]
use std::{
    fs::File,
    os::fd::{FromRawFd, IntoRawFd, RawFd},
    ptr,
};
use std::{io, sync::Arc};

/// Owns a mapped shared paint buffer for the lifetime of a batch decode.
#[derive(Debug)]
pub(crate) struct SharedMapping {
    #[cfg(unix)]
    ptr: *mut u8,
    #[cfg(unix)]
    len: usize,
    #[cfg(unix)]
    fd: RawFd,
    #[cfg(not(unix))]
    _private: (),
}

// Safety: mapping is read-only after construction and not shared across threads
// without an Arc; the decode path is single-threaded.
unsafe impl Send for SharedMapping {}
unsafe impl Sync for SharedMapping {}

impl SharedMapping {
    #[cfg(unix)]
    pub(super) fn map_fd(fd: i32, max_len: usize) -> io::Result<Arc<Self>> {
        let file = unsafe { File::from_raw_fd(fd) };
        let len = usize::try_from(file.metadata()?.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR buffer does not fit this platform",
            )
        })?;
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR buffer is empty",
            ));
        }
        if len > max_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared OSR paint buffer exceeds the protocol limit",
            ));
        }
        let raw = file.into_raw_fd();
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                raw,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            unsafe {
                let _ = File::from_raw_fd(raw);
            }
            return Err(io::Error::last_os_error());
        }
        Ok(Arc::new(Self {
            ptr: ptr.cast(),
            len,
            fd: raw,
        }))
    }

    #[cfg(not(unix))]
    pub(super) fn map_fd(_fd: i32, _max_len: usize) -> io::Result<Arc<Self>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared OSR buffers are unavailable on this transport",
        ))
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        #[cfg(unix)]
        {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
        #[cfg(not(unix))]
        {
            &[]
        }
    }
}

impl Drop for SharedMapping {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            if !self.ptr.is_null() && self.len > 0 {
                unsafe {
                    libc::munmap(self.ptr.cast(), self.len);
                }
            }
            if self.fd >= 0 {
                unsafe {
                    let _ = File::from_raw_fd(self.fd);
                }
            }
        }
    }
}
