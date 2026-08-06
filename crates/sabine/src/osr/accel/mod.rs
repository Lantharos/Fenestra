mod fallback;
#[cfg(target_os = "linux")]
mod import_linux;
mod mmap_fallback;

pub(crate) use fallback::{AccelFallbackPolicy, should_relaunch_software};
#[cfg(target_os = "linux")]
pub(crate) use import_linux::try_import_dmabuf;
pub(crate) use mmap_fallback::accel_to_paint_batch;
