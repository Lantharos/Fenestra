use crate::osr::frame_buffer::FrameDamage;
use crate::osr::protocol::OsrFrame;
use crate::render::{GpuRenderer, RendererError};

/// Prefer sparse per-rect uploads when the AABB would waste significant bandwidth.
pub(super) fn prefer_sparse(frames: &[OsrFrame], damage: FrameDamage) -> bool {
    if frames.len() <= 1 {
        return false;
    }
    let union_area = u64::from(damage.width) * u64::from(damage.height);
    let sum_area = frames.iter().fold(0_u64, |acc, frame| {
        acc + u64::from(frame.width) * u64::from(frame.height)
    });
    union_area > sum_area.saturating_mul(2)
}

pub(super) fn upload_composed_damage(
    renderer: &mut GpuRenderer,
    texture_id: &str,
    buffer_width: u32,
    buffer_height: u32,
    buffer: &[u8],
    damage: FrameDamage,
    frames: &[OsrFrame],
) -> Result<(), RendererError> {
    if prefer_sparse(frames, damage) {
        return renderer.update_dynamic_bgra_image_rects(
            texture_id,
            buffer_width,
            buffer_height,
            frames.iter().filter_map(|frame| {
                if frame.x < 0 || frame.y < 0 {
                    return None;
                }
                Some((
                    frame.x as u32,
                    frame.y as u32,
                    frame.width,
                    frame.height,
                    frame.bytes.as_slice(),
                ))
            }),
        );
    }
    renderer.update_dynamic_bgra_image_region(
        texture_id,
        buffer_width,
        buffer_height,
        damage.x,
        damage.y,
        damage.width,
        damage.height,
        buffer,
    )
}
