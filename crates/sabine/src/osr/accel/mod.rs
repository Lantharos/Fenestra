mod fallback;
#[cfg(target_os = "linux")]
mod import_linux;
#[cfg(target_os = "macos")]
mod import_mac;
#[cfg(windows)]
mod import_win;
mod mmap_fallback;

pub(crate) use fallback::{AccelFallbackPolicy, should_relaunch_software};
#[cfg(target_os = "linux")]
pub(crate) use import_linux::try_import_dmabuf;
#[cfg(target_os = "macos")]
pub(crate) use import_mac::try_import_iosurface;
#[cfg(windows)]
pub(crate) use import_win::try_import_d3d11;
pub(crate) use mmap_fallback::accel_to_paint_batch;

use crate::osr::protocol::OsrAccelFrame;
use crate::render::GpuRenderer;

pub(crate) fn install_imported_texture(
    renderer: &mut GpuRenderer,
    texture_id: &str,
    frame: &OsrAccelFrame,
    texture: wgpu::Texture,
) -> Result<(), String> {
    renderer
        .install_external_bgra_texture(texture_id, texture, frame.width, frame.height)
        .map_err(|error| error.to_string())
}
