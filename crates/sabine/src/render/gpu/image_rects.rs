use super::{GpuRenderer, RendererError};

impl GpuRenderer {
    /// Upload packed BGRA rects (tight `bytes_per_row = width * 4`) into a
    /// dynamic image without requiring the full framebuffer.
    pub fn update_dynamic_bgra_image_rects<'a>(
        &mut self,
        id: impl Into<String>,
        width: u32,
        height: u32,
        rects: impl IntoIterator<Item = (u32, u32, u32, u32, &'a [u8])>,
    ) -> Result<(), RendererError> {
        let id = id.into();
        if width == 0 || height == 0 {
            return Err(RendererError::Texture(
                "dynamic image dimensions must be non-zero".to_string(),
            ));
        }
        let recreate = self
            .texture_cache
            .get(&id)
            .is_none_or(|entry| entry.external || entry.width != width || entry.height != height);
        if recreate {
            self.create_dynamic_bgra_image(id.clone(), width, height);
        }
        let Some(entry) = self.texture_cache.get(&id) else {
            return Err(RendererError::Texture(
                "dynamic image was not cached".to_string(),
            ));
        };
        for (x, y, region_width, region_height, bytes) in rects {
            if region_width == 0 || region_height == 0 {
                continue;
            }
            if x.saturating_add(region_width) > width || y.saturating_add(region_height) > height {
                return Err(RendererError::Texture(format!(
                    "dynamic image region ({x},{y},{region_width}x{region_height}) exceeds {width}x{height}"
                )));
            }
            let expected = region_width as usize * region_height as usize * 4;
            if bytes.len() != expected {
                return Err(RendererError::Texture(format!(
                    "dynamic image rect expected {expected} bytes, got {}",
                    bytes.len()
                )));
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &entry.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * region_width),
                    rows_per_image: Some(region_height),
                },
                wgpu::Extent3d {
                    width: region_width,
                    height: region_height,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(())
    }
}
