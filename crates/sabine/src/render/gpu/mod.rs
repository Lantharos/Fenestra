use std::{collections::HashMap, sync::Arc};

use thiserror::Error;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::render::rect_pipeline::{
    Globals, RectVertex, create_image_pipeline, create_rounded_rect_pipeline, push_rect_command,
    push_rounded_rect_command, to_wgpu_color,
};
use crate::render::{DisplayCommand, DisplayList};

mod image_rects;
mod images;
mod text;
mod vertex_buffer;

use text::TextRendererState;
use vertex_buffer::DynamicVertexBuffer;

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
    rect_vertex_buffer: DynamicVertexBuffer,
    image_vertex_buffer: DynamicVertexBuffer,
    text: Option<TextRendererState>,
    texture_cache: HashMap<String, CachedTexture>,
    scale_factor: f32,
    surface_alpha_is_opaque: bool,
    window: Arc<dyn Window>,
}

#[derive(Clone)]
pub(super) struct CachedTexture {
    pub(super) texture: wgpu::Texture,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) width: u32,
    pub(super) height: u32,
}

fn select_surface_alpha_mode(
    modes: &[wgpu::CompositeAlphaMode],
    transparent: bool,
) -> wgpu::CompositeAlphaMode {
    if transparent {
        for preferred in [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ] {
            if modes.contains(&preferred) {
                return preferred;
            }
        }
    }
    modes
        .iter()
        .copied()
        .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
        .unwrap_or(modes[0])
}

fn preferred_backends() -> wgpu::Backends {
    #[cfg(target_os = "windows")]
    {
        wgpu::Backends::DX12
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

impl GpuRenderer {
    pub async fn new(window: Arc<dyn Window>, transparent: bool) -> Result<Self, RendererError> {
        let size = window.surface_size();
        let instance_descriptor = wgpu::InstanceDescriptor {
            backends: preferred_backends(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        };
        #[cfg(target_os = "windows")]
        let instance_descriptor = {
            let mut descriptor = instance_descriptor;
            descriptor.backend_options.dx12.presentation_system =
                wgpu::Dx12SwapchainKind::DxgiFromVisual;
            descriptor
        };
        let instance = wgpu::Instance::new(instance_descriptor);
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| RendererError::Surface(error.to_string()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                power_preference: wgpu::PowerPreference::LowPower,
                ..Default::default()
            })
            .await
            .map_err(|error| RendererError::Adapter(error.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("sabine-gpu"),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
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
        let alpha_mode = select_surface_alpha_mode(&capabilities.alpha_modes, transparent);
        let surface_alpha_is_opaque = matches!(
            alpha_mode,
            wgpu::CompositeAlphaMode::Opaque | wgpu::CompositeAlphaMode::Auto
        );
        if std::env::var_os("SABINE_TRACE").is_some() {
            #[cfg(target_os = "windows")]
            let presentation = "DirectComposition";
            #[cfg(not(target_os = "windows"))]
            let presentation = "native";
            let adapter_info = adapter.get_info();
            eprintln!(
                "Sabine GPU: adapter={:?} type={:?} backend={:?} driver={:?} presentation={presentation} surface alpha={alpha_mode:?} transparent={transparent}",
                adapter_info.name,
                adapter_info.device_type,
                adapter_info.backend,
                adapter_info.driver
            );
        }
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
            rect_vertex_buffer: DynamicVertexBuffer::default(),
            image_vertex_buffer: DynamicVertexBuffer::default(),
            text: None,
            texture_cache: HashMap::new(),
            scale_factor: window.scale_factor() as f32,
            surface_alpha_is_opaque,
            window,
        })
    }

    pub(crate) fn surface_alpha_is_opaque(&self) -> bool {
        self.surface_alpha_is_opaque
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
        let rect_vertices = self.rect_vertices(display_list);
        let (image_draws, image_vertices) = self.image_draws(display_list);
        let has_text = display_list
            .commands
            .iter()
            .any(|command| matches!(command, DisplayCommand::Text(_)));
        if has_text {
            let text = self.text.get_or_insert_with(|| {
                TextRendererState::new(&self.device, &self.queue, self.surface_config.format)
            });
            text.prepare(
                &self.device,
                &self.queue,
                display_list,
                self.scale_factor,
                self.surface_config.width,
                self.surface_config.height,
            )?;
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
        let rect_vertex_buffer = self.rect_vertex_buffer.upload(
            &self.device,
            &self.queue,
            "sabine rounded rect vertices",
            &rect_vertices,
        );
        let image_vertex_buffer = self.image_vertex_buffer.upload(
            &self.device,
            &self.queue,
            "sabine image vertices",
            &image_vertices,
        );

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

            if let Some(vertex_buffer) = &rect_vertex_buffer {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..rect_vertices.len() as u32, 0..1);
            }

            for draw in &image_draws {
                pass.set_pipeline(&self.image_pipeline);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_bind_group(1, &draw.bind_group, &[]);

                if let Some(vertex_buffer) = &image_vertex_buffer {
                    pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    pass.draw(draw.vertices.clone(), 0..1);
                }
            }

            if has_text && let Some(text) = &self.text {
                text.render(&mut pass)?;
            }
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        if let Some(text) = &mut self.text {
            text.trim();
        }
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
