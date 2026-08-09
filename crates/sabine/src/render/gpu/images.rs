use image::GenericImageView;
use std::ops::Range;

use crate::render::rect_pipeline::{ImageVertex, push_image_quad};
use crate::render::{DisplayCommand, DisplayList};

use super::{CachedTexture, GpuRenderer, RendererError};

pub(super) struct ImageDraw {
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) vertices: Range<u32>,
}

impl GpuRenderer {
    pub fn set_dynamic_bgra_image(
        &mut self,
        id: impl Into<String>,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) -> Result<(), RendererError> {
        let id = id.into();
        if width == 0 || height == 0 {
            return Err(RendererError::Texture(
                "dynamic image has empty size".to_string(),
            ));
        }
        let expected_len = width as usize * height as usize * 4;
        if bytes.len() != expected_len {
            return Err(RendererError::Texture(format!(
                "dynamic image expected {expected_len} bytes, got {}",
                bytes.len()
            )));
        }

        let recreate = self
            .texture_cache
            .get(&id)
            .is_none_or(|entry| entry.width != width || entry.height != height);
        if recreate {
            self.create_dynamic_bgra_image(id.clone(), width, height);
        }

        let Some(entry) = self.texture_cache.get(&id) else {
            return Err(RendererError::Texture(
                "dynamic image was not cached".to_string(),
            ));
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &entry.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub fn update_dynamic_bgra_image_region(
        &mut self,
        id: impl Into<String>,
        image_size: (u32, u32),
        origin: (u32, u32),
        region_size: (u32, u32),
        bytes: &[u8],
    ) -> Result<(), RendererError> {
        let id = id.into();
        let (width, height) = image_size;
        let (x, y) = origin;
        let (region_width, region_height) = region_size;
        if width == 0 || height == 0 {
            return Err(RendererError::Texture(
                "dynamic image has empty size".to_string(),
            ));
        }
        if region_width == 0 || region_height == 0 {
            return Ok(());
        }
        if x >= width
            || y >= height
            || x.saturating_add(region_width) > width
            || y.saturating_add(region_height) > height
        {
            return Err(RendererError::Texture(format!(
                "dynamic image region {x},{y} {region_width}x{region_height} exceeds {width}x{height}"
            )));
        }
        let expected_len = width as usize * height as usize * 4;
        if bytes.len() != expected_len {
            return Err(RendererError::Texture(format!(
                "dynamic image expected {expected_len} bytes, got {}",
                bytes.len()
            )));
        }

        let recreate = self
            .texture_cache
            .get(&id)
            .is_none_or(|entry| entry.width != width || entry.height != height);
        if recreate {
            self.create_dynamic_bgra_image(id.clone(), width, height);
            return self.set_dynamic_bgra_image(id, width, height, bytes);
        }

        let Some(entry) = self.texture_cache.get(&id) else {
            return Err(RendererError::Texture(
                "dynamic image was not cached".to_string(),
            ));
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &entry.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: (u64::from(y) * u64::from(width) + u64::from(x)) * 4,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width: region_width,
                height: region_height,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    pub(super) fn image_draws(
        &mut self,
        display_list: &DisplayList,
    ) -> (Vec<ImageDraw>, Vec<ImageVertex>) {
        let mut draws = Vec::new();
        let mut vertices = Vec::new();
        for command in &display_list.commands {
            let DisplayCommand::Image(image) = command else {
                continue;
            };
            let cached = self.texture_cache.get(&image.id).cloned();
            let bind_group = match cached {
                Some(entry) => entry.bind_group,
                None => match self.load_image_texture(&image.id) {
                    Ok(bind_group) => bind_group,
                    Err(_) => continue,
                },
            };
            let vertex_start = vertices.len() as u32;
            push_image_quad(
                &mut vertices,
                image.x,
                image.y,
                image.width,
                image.height,
                self.scale_factor,
            );
            draws.push(ImageDraw {
                bind_group,
                vertices: vertex_start..vertices.len() as u32,
            });
        }
        (draws, vertices)
    }

    fn load_image_texture(&mut self, path: &str) -> Result<wgpu::BindGroup, String> {
        let img = image::open(path).map_err(|e| format!("failed to load image {path}: {e}"))?;
        let dimensions = img.dimensions();
        let rgba = img.to_rgba8();
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(path),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * dimensions.0),
                rows_per_image: Some(dimensions.1),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(path),
            layout: &self.image_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.texture_cache.insert(
            path.to_string(),
            CachedTexture {
                texture,
                bind_group: bind_group.clone(),
                width: dimensions.0,
                height: dimensions.1,
            },
        );
        Ok(bind_group)
    }

    #[cfg(windows)]
    pub fn copy_external_bgra_texture(
        &mut self,
        id: impl Into<String>,
        source: wgpu::Texture,
        source_origin: (u32, u32),
        size: (u32, u32),
        completed: impl FnOnce() + Send + 'static,
    ) -> Result<(), RendererError> {
        let id = id.into();
        let (width, height) = size;
        if width == 0 || height == 0 {
            return Err(RendererError::Texture(
                "external image has empty size".to_string(),
            ));
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&id),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sabine-osr-d3d11-copy"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: source_origin.0,
                    y: source_origin.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            texture.as_image_copy(),
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture: &source,
                selector: None,
                state: wgpu::TextureUses::empty(),
            }),
        );
        self.queue.submit([encoder.finish()]);
        self.queue.on_submitted_work_done(completed);

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&id),
            layout: &self.image_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.texture_cache.insert(
            id,
            CachedTexture {
                texture,
                bind_group,
                width,
                height,
            },
        );
        Ok(())
    }

    #[cfg(windows)]
    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(super) fn create_dynamic_bgra_image(&mut self, id: String, width: u32, height: u32) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&id),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&id),
            layout: &self.image_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.texture_cache.insert(
            id,
            CachedTexture {
                texture,
                bind_group,
                width,
                height,
            },
        );
    }
}
