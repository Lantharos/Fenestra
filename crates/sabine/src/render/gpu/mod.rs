use std::{collections::HashMap, sync::Arc};

use glyphon::{
    Buffer, Cache, FontSystem, Resolution, SwashCache, TextAtlas, TextRenderer, Viewport,
};
use thiserror::Error;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::render::rect_pipeline::{
    Globals, RectVertex, create_image_pipeline, create_rounded_rect_pipeline, push_rect_command,
    push_rounded_rect_command, to_wgpu_color,
};
use crate::render::{DisplayCommand, DisplayList, TextCommand};

mod image_rects;
mod images;
mod text;

use text::text_areas;

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("failed to create GPU surface: {0}")]
    Surface(String),
    #[error("failed to request GPU adapter: {0}")]
    Adapter(String),
    #[error("failed to request GPU device: {0}")]
    Device(String),
    #[error("text renderer failed: {0}")]
    Text(String),
    #[error("surface validation failed")]
    SurfaceValidation,
    #[error("texture update failed: {0}")]
    Texture(String),
}

pub struct GpuRenderer {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    image_sampler: wgpu::Sampler,
    image_bind_group_layout: wgpu::BindGroupLayout,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffers: Vec<TextBufferEntry>,
    texture_cache: HashMap<String, CachedTexture>,
    scale_factor: f32,
    window: Arc<dyn Window>,
}

#[derive(Clone)]
pub(super) struct CachedTexture {
    pub(super) texture: wgpu::Texture,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) struct TextBufferEntry {
    pub(super) buffer: Buffer,
    pub(super) command: TextCommand,
}

fn preferred_backends() -> wgpu::Backends {
    #[cfg(target_os = "windows")]
    {
        // CEF D3D11 shared-handle import is Vulkan-only in wgpu-hal
        // (`texture_from_d3d11_shared_handle`). DX12 cannot import those handles.
        wgpu::Backends::VULKAN
    }
    #[cfg(target_os = "linux")]
    {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    }
    #[cfg(target_os = "macos")]
    {
        wgpu::Backends::METAL
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        wgpu::Backends::all()
    }
}

fn external_memory_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let supported = adapter.features();
    let mut features = wgpu::Features::empty();
    #[cfg(target_os = "linux")]
    {
        let dma = wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF;
        if supported.contains(dma) {
            features |= dma;
        }
    }
    #[cfg(target_os = "windows")]
    {
        let win32 = wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32;
        if supported.contains(win32) {
            features |= win32;
        }
    }
    let _ = supported;
    features
}

impl GpuRenderer {
    pub async fn new(window: Arc<dyn Window>) -> Result<Self, RendererError> {
        let size = window.surface_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: preferred_backends(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| RendererError::Surface(error.to_string()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|error| RendererError::Adapter(error.to_string()))?;
        let required_features = external_memory_features(&adapter);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("sabine-gpu"),
                required_features,
                ..Default::default()
            })
            .await
            .map_err(|error| RendererError::Device(error.to_string()))?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let present_mode = capabilities
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::Fifo)
            .unwrap_or(capabilities.present_modes[0]);
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::PreMultiplied)
            .or_else(|| {
                capabilities
                    .alpha_modes
                    .iter()
                    .copied()
                    .find(|mode| *mode == wgpu::CompositeAlphaMode::PostMultiplied)
            })
            .unwrap_or(capabilities.alpha_modes[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &surface_config);

        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sabine globals"),
            contents: bytemuck::cast_slice(&[Globals {
                viewport: [surface_config.width as f32, surface_config.height as f32],
                _padding: [0.0, 0.0],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let globals_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sabine globals bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sabine globals bind group"),
            layout: &globals_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });
        let pipeline = create_rounded_rect_pipeline(&device, format, &globals_bind_group_layout);
        let image_pipeline = create_image_pipeline(&device, format, &globals_bind_group_layout);
        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sabine image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let image_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sabine image per-texture bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        let font_system = FontSystem::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        Ok(Self {
            instance,
            device,
            queue,
            surface,
            surface_config,
            pipeline,
            image_pipeline,
            image_sampler,
            image_bind_group_layout,
            globals_buffer,
            globals_bind_group,
            font_system,
            swash_cache: SwashCache::new(),
            viewport,
            atlas,
            text_renderer,
            text_buffers: Vec::new(),
            texture_cache: HashMap::new(),
            scale_factor: window.scale_factor() as f32,
            window,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f32) {
        if width == 0 || height == 0 {
            return;
        }
        let scale_factor = scale_factor.max(0.25);
        // Wayland often emits a configure after interactive move with the same
        // size. Reconfiguring the swapchain there flashes a blank frame.
        if self.surface_config.width == width
            && self.surface_config.height == height
            && (self.scale_factor - scale_factor).abs() < f32::EPSILON
        {
            return;
        }

        self.surface_config.width = width;
        self.surface_config.height = height;
        self.scale_factor = scale_factor;
        self.surface.configure(&self.device, &self.surface_config);
    }

    pub fn render(&mut self, display_list: &DisplayList) -> Result<(), RendererError> {
        self.queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::cast_slice(&[Globals {
                viewport: [
                    self.surface_config.width as f32,
                    self.surface_config.height as f32,
                ],
                _padding: [0.0, 0.0],
            }]),
        );
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.surface_config.width,
                height: self.surface_config.height,
            },
        );

        let rect_vertices = self.rect_vertices(display_list);
        let image_draws = self.image_draws(display_list);
        self.rebuild_text_buffers(display_list);
        let text_areas = text_areas(&self.text_buffers, self.scale_factor);
        if !text_areas.is_empty() {
            self.text_renderer
                .prepare(
                    &self.device,
                    &self.queue,
                    &mut self.font_system,
                    &mut self.atlas,
                    &self.viewport,
                    text_areas,
                    &mut self.swash_cache,
                )
                .map_err(|error| RendererError::Text(error.to_string()))?;
        }

        let Some(frame) = self.acquire_surface_texture()? else {
            return Ok(());
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("sabine render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sabine render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(to_wgpu_color(display_list.background)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if !rect_vertices.is_empty() {
                let vertex_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("sabine rounded rect vertices"),
                            contents: bytemuck::cast_slice(&rect_vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..rect_vertices.len() as u32, 0..1);
            }

            for draw in &image_draws {
                pass.set_pipeline(&self.image_pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_bind_group(1, &draw.bind_group, &[]);

                let vertex_buffer =
                    self.device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("sabine image vertices"),
                            contents: bytemuck::cast_slice(&draw.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..draw.vertices.len() as u32, 0..1);
            }

            if !self.text_buffers.is_empty() {
                self.text_renderer
                    .render(&self.atlas, &self.viewport, &mut pass)
                    .map_err(|error| RendererError::Text(error.to_string()))?;
            }
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.atlas.trim();
        Ok(())
    }

    fn acquire_surface_texture(&mut self) -> Result<Option<wgpu::SurfaceTexture>, RendererError> {
        // After interactive move Wayland often returns Outdated. Reconfigure and
        // retry in-place — returning without a present flashes transparent glass.
        for attempt in 0..3 {
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame) => return Ok(Some(frame)),
                wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                    // Still present this buffer; refresh config for the next frame.
                    self.surface.configure(&self.device, &self.surface_config);
                    return Ok(Some(frame));
                }
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    self.window.request_redraw();
                    return Ok(None);
                }
                wgpu::CurrentSurfaceTexture::Outdated => {
                    self.surface.configure(&self.device, &self.surface_config);
                    if attempt + 1 == 3 {
                        self.window.request_redraw();
                        return Ok(None);
                    }
                }
                wgpu::CurrentSurfaceTexture::Lost => {
                    self.surface = self
                        .instance
                        .create_surface(self.window.clone())
                        .map_err(|error| RendererError::Surface(error.to_string()))?;
                    self.surface.configure(&self.device, &self.surface_config);
                    if attempt + 1 == 3 {
                        self.window.request_redraw();
                        return Ok(None);
                    }
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    return Err(RendererError::SurfaceValidation);
                }
            }
        }
        Ok(None)
    }

    fn rect_vertices(&self, display_list: &DisplayList) -> Vec<RectVertex> {
        let mut vertices = Vec::new();
        for command in &display_list.commands {
            match command {
                DisplayCommand::Rect(command) => {
                    push_rect_command(&mut vertices, command, self.scale_factor)
                }
                DisplayCommand::RoundedRect(command) => {
                    push_rounded_rect_command(&mut vertices, command, self.scale_factor)
                }
                DisplayCommand::Text(_) | DisplayCommand::Image(_) => {}
            }
        }
        vertices
    }
}
