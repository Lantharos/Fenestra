#[cfg(windows)]
mod import_win;
use crate::osr::protocol::OsrAccelFrame;
#[cfg(windows)]
use crate::render::GpuRenderer;
#[cfg(windows)]
pub(crate) use import_win::{close_imported_handle, try_import_d3d12};

#[cfg(windows)]
pub(crate) fn install_imported_texture(
    renderer: &mut GpuRenderer,
    texture_id: &str,
    frame: &OsrAccelFrame,
    texture: wgpu::Texture,
    completed: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    renderer
        .set_external_bgra_texture(
            texture_id,
            texture,
            (frame.visible_x, frame.visible_y),
            (frame.visible_width, frame.visible_height),
            completed,
        )
        .map_err(|error| error.to_string())
}

#[cfg(not(windows))]
pub(crate) fn discard_frame(_frame: OsrAccelFrame) {}
