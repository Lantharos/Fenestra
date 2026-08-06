use objc2::runtime::ProtocolObject;
use objc2_io_surface::{IOSurfaceID, IOSurfaceLookup};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};
use wgpu::hal::api::Metal;

use crate::osr::protocol::OsrAccelFrame;
use crate::render::GpuRenderer;

const CEF_COLOR_TYPE_BGRA_8888: u32 = 1;

/// Zero-copy IOSurface → Metal texture → wgpu.
pub(crate) fn try_import_iosurface(
    renderer: &mut GpuRenderer,
    frame: &OsrAccelFrame,
) -> Result<wgpu::Texture, String> {
    if frame.native_handle == 0 || frame.width == 0 || frame.height == 0 {
        return Err("invalid iosurface frame".into());
    }
    if frame.format != CEF_COLOR_TYPE_BGRA_8888 {
        return Err("iosurface import requires BGRA8888".into());
    }

    let surface_id = frame.native_handle as IOSurfaceID;
    let surface =
        IOSurfaceLookup(surface_id).ok_or_else(|| "IOSurfaceLookup failed".to_string())?;

    let desc = wgpu::TextureDescriptor {
        label: Some("sabine-osr-iosurface"),
        size: wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    };

    let mtl_texture = {
        let Some(hal_device) = (unsafe { renderer.device().as_hal::<Metal>() }) else {
            return Err("wgpu device is not Metal".into());
        };
        let device = hal_device.raw_device();
        let tex_desc = unsafe { MTLTextureDescriptor::new() };
        unsafe {
            tex_desc.setTextureType(MTLTextureType::Type2D);
            tex_desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            tex_desc.setWidth(frame.width as _);
            tex_desc.setHeight(frame.height as _);
            tex_desc.setDepth(1);
            tex_desc.setMipmapLevelCount(1);
            tex_desc.setSampleCount(1);
            tex_desc.setArrayLength(1);
            tex_desc.setUsage(MTLTextureUsage::ShaderRead);
            tex_desc.setStorageMode(MTLStorageMode::Shared);
        }
        let texture: Option<objc2::rc::Retained<ProtocolObject<dyn MTLTexture>>> =
            unsafe { device.newTextureWithDescriptor_iosurface_plane(&tex_desc, &surface, 0) };
        texture.ok_or_else(|| "newTextureWithDescriptor:iosurface:plane: failed".to_string())?
    };

    let copy_size = wgpu::hal::CopyExtent {
        width: frame.width,
        height: frame.height,
        depth: 1,
    };
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            mtl_texture,
            desc.format,
            MTLTextureType::Type2D,
            1,
            1,
            copy_size,
            None,
        )
    };

    Ok(unsafe {
        renderer.device().create_texture_from_hal::<Metal>(
            hal_texture,
            &desc,
            wgpu::TextureUses::RESOURCE,
        )
    })
}
