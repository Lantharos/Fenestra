use wgpu::hal::api::Vulkan;

use crate::osr::protocol::OsrAccelFrame;
use crate::render::GpuRenderer;

const CEF_COLOR_TYPE_RGBA_8888: u32 = 0;
const CEF_COLOR_TYPE_BGRA_8888: u32 = 1;

/// Import a duplicated D3D11 shared `HANDLE` into a wgpu Vulkan texture.
pub(crate) fn try_import_d3d11(
    renderer: &mut GpuRenderer,
    frame: &OsrAccelFrame,
) -> Result<wgpu::Texture, String> {
    if frame.native_handle == 0 || frame.width == 0 || frame.height == 0 {
        return Err("invalid d3d11 shared handle frame".into());
    }
    // CEF reports channel order; the shared DXGI texture is BGRA8888 either way.
    if frame.format != CEF_COLOR_TYPE_BGRA_8888 && frame.format != CEF_COLOR_TYPE_RGBA_8888 {
        close_imported_handle(frame.native_handle);
        return Err(format!("unsupported d3d11 color format {}", frame.format));
    }
    if !renderer.supports_feature(wgpu::Features::VULKAN_EXTERNAL_MEMORY_WIN32) {
        close_imported_handle(frame.native_handle);
        return Err("adapter lacks VULKAN_EXTERNAL_MEMORY_WIN32".into());
    }

    let desc = wgpu::TextureDescriptor {
        label: Some("sabine-osr-d3d11"),
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

    let handle = windows::Win32::Foundation::HANDLE(frame.native_handle as *mut std::ffi::c_void);
    let hal_desc = wgpu::hal::TextureDescriptor {
        label: desc.label,
        size: desc.size,
        mip_level_count: desc.mip_level_count,
        sample_count: desc.sample_count,
        dimension: desc.dimension,
        format: desc.format,
        usage: wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COPY_SRC,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: vec![],
    };

    let hal_texture = {
        let Some(hal_device) = (unsafe { renderer.device().as_hal::<Vulkan>() }) else {
            close_imported_handle(frame.native_handle);
            return Err("wgpu device is not Vulkan".into());
        };
        match unsafe { hal_device.texture_from_d3d11_shared_handle(handle, &hal_desc) } {
            Ok(texture) => texture,
            Err(error) => {
                close_imported_handle(frame.native_handle);
                return Err(format!("texture_from_d3d11_shared_handle: {error:?}"));
            }
        }
    };

    Ok(unsafe {
        renderer.device().create_texture_from_hal::<Vulkan>(
            hal_texture,
            &desc,
            wgpu::TextureUses::RESOURCE,
        )
    })
}

pub(crate) fn close_imported_handle(raw: u64) {
    if raw == 0 {
        return;
    }
    let handle = windows::Win32::Foundation::HANDLE(raw as *mut std::ffi::c_void);
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
}
