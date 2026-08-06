use crate::osr::protocol::OsrAccelFrame;
use crate::render::GpuRenderer;

/// Attempt zero-copy DMA-BUF → wgpu import.
///
/// Returns `Err` to fall through to [`super::mmap_fallback`]. True Vulkan
/// external-memory import is wired here once the wgpu HAL path is stable on
/// Wayland; until then mmap keeps accelerated CEF frames flowing.
pub(crate) fn try_import_dmabuf(
    _renderer: &mut GpuRenderer,
    _frame: &OsrAccelFrame,
) -> Result<(), String> {
    Err("dma-buf wgpu import not available; using mmap fallback".into())
}
