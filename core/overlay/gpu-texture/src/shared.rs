use core::ptr;

use anyhow::Context;
use windows::{
    Win32::{
        Foundation::{HANDLE, LUID},
        Graphics::{
            Direct3D11::{ID3D11Device, ID3D11Device1, ID3D11Texture2D},
            Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1, IDXGIResource1},
        },
    },
    core::Interface,
};

use crate::GpuLuid;

/// Find a DXGI adapter matching the given LUID.
pub fn find_dxgi_adapter(luid: GpuLuid) -> anyhow::Result<Option<IDXGIAdapter>> {
    let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>()? };
    let target = LUID {
        LowPart: luid.low,
        HighPart: luid.high,
    };

    let mut index = 0;
    loop {
        let adapter = match unsafe { factory.EnumAdapters(index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        index += 1;

        let Ok(desc) = (unsafe { adapter.GetDesc() }) else {
            continue;
        };
        if desc.AdapterLuid == target {
            return Ok(Some(adapter));
        }
    }

    Ok(None)
}

/// Open an NT-handle shared D3D11 texture on the given device.
pub fn open_shared_texture2d(
    device: &ID3D11Device,
    nt_handle: u32,
) -> anyhow::Result<ID3D11Texture2D> {
    let handle = HANDLE(ptr::with_exposed_provenance_mut(nt_handle as usize));

    if let Ok(device1) = device.cast::<ID3D11Device1>() {
        if let Ok(resource) =
            unsafe { device1.OpenSharedResource1::<IDXGIResource1>(handle) }
        {
            return resource.cast().context("shared resource is not a 2D texture");
        }
    }

    let mut texture = None;
    unsafe {
        device.OpenSharedResource(handle, &mut texture)
            .context("OpenSharedResource failed")?;
    }
    texture.context("shared resource is not a 2D texture")
}

/// Read width and height from a D3D11 2D texture.
pub fn texture_dimensions(texture: &ID3D11Texture2D) -> (u32, u32) {
    let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    (desc.Width, desc.Height)
}
