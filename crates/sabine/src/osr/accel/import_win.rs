use wgpu::hal::api::Dx12;
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;

use crate::osr::protocol::OsrAccelFrame;
use crate::render::GpuRenderer;

const CEF_COLOR_TYPE_BGRA_8888: u32 = 1;

/// Open the Sabine-owned D3D11 shared texture on wgpu's D3D12 device.
pub(crate) fn try_import_d3d12(
    renderer: &mut GpuRenderer,
    frame: &OsrAccelFrame,
) -> Result<wgpu::Texture, String> {
    if frame.native_handle == 0 || frame.width == 0 || frame.height == 0 {
        return Err("invalid d3d11 shared handle frame".into());
    }
    if frame.format != CEF_COLOR_TYPE_BGRA_8888 {
        return Err(format!("unsupported d3d11 color format {}", frame.format));
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
    let hal_texture = {
        let Some(hal_device) = (unsafe { renderer.device().as_hal::<Dx12>() }) else {
            return Err("wgpu device is not D3D12".into());
        };
        let mut resource = None::<ID3D12Resource>;
        unsafe {
            hal_device
                .raw_device()
                .OpenSharedHandle(handle, &mut resource)
        }
        .map_err(|error| format!("ID3D12Device::OpenSharedHandle: {error}"))?;
        let resource = resource.ok_or_else(|| "D3D12 shared resource was null".to_string())?;
        unsafe {
            wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                desc.format,
                desc.dimension,
                desc.size,
                desc.mip_level_count,
                desc.sample_count,
            )
        }
    };

    Ok(unsafe {
        renderer.device().create_texture_from_hal::<Dx12>(
            hal_texture,
            &desc,
            wgpu::TextureUses::empty(),
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
