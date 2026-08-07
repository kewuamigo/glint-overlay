mod present;
mod shared;

use core::ptr;

use anyhow::{Context, bail};
use scopeguard::defer;
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE,
                IDXGIKeyedMutex, IDXGIResource1,
            },
        },
    },
    core::Interface,
};

pub use present::{
    resolve_dxgi_present_address, resolve_dxgi_present_addresses, DxgiPresent1Fn, DxgiPresentFn,
};
pub use shared::{find_dxgi_adapter, open_shared_texture2d, texture_dimensions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuLuid {
    pub low: u32,
    pub high: i32,
}

pub struct SharedTextureUploader {
    device: ID3D11Device,
    cx: ID3D11DeviceContext,
    buffers: [Option<SharedBuffer>; 2],
    index: usize,
    width: u32,
    height: u32,
}

struct SharedBuffer {
    texture: ID3D11Texture2D,
    mutex: IDXGIKeyedMutex,
    nt_handle: u32,
}

impl SharedTextureUploader {
    pub fn new(luid: GpuLuid) -> anyhow::Result<Self> {
        let adapter = find_dxgi_adapter(luid)?;
        let mut device = None;
        let mut cx = None;
        unsafe {
            D3D11CreateDevice(
                adapter.as_ref(),
                if adapter.is_none() {
                    D3D_DRIVER_TYPE_HARDWARE
                } else {
                    D3D_DRIVER_TYPE_UNKNOWN
                },
                HMODULE(ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut cx),
            )?;
        }

        Ok(Self {
            device: device.context("failed to create D3D11 device")?,
            cx: cx.context("failed to create D3D11 context")?,
            buffers: [const { None }; 2],
            index: 0,
            width: 0,
            height: 0,
        })
    }

    pub fn upload(&mut self, width: u32, height: u32, data: &[u8]) -> anyhow::Result<u32> {
        if width == 0 || height == 0 {
            bail!("width and height must be non-zero");
        }

        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            bail!(
                "BGRA byte length mismatch: expected {expected}, got {}",
                data.len()
            );
        }

        let size_changed = self.width != width || self.height != height;
        if size_changed {
            self.width = width;
            self.height = height;
            self.index = (self.index + 1) % 2;
            self.buffers[self.index] = None;
        } else {
            self.index = (self.index + 1) % 2;
            if self.buffers[self.index].is_some() {
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe {
                    self.buffers[self.index]
                        .as_ref()
                        .unwrap()
                        .texture
                        .GetDesc(&mut desc);
                }
                if desc.Width != width || desc.Height != height {
                    self.buffers[self.index] = None;
                }
            }
        }

        if self.buffers[self.index].is_none() {
            self.buffers[self.index] = Some(create_shared_buffer(
                &self.device,
                width,
                height,
                Some(data),
            )?);
        } else {
            let buffer = self.buffers[self.index].as_ref().unwrap();
            unsafe {
                buffer.mutex.AcquireSync(0, u32::MAX)?;
                defer!({
                    let _ = buffer.mutex.ReleaseSync(0);
                });
                self.cx.UpdateSubresource(
                    &buffer.texture,
                    0,
                    None,
                    data.as_ptr().cast(),
                    width * 4,
                    0,
                );
            }
        }

        Ok(self.buffers[self.index].as_ref().unwrap().nt_handle)
    }
}

fn create_shared_buffer(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    initial: Option<&[u8]>,
) -> anyhow::Result<SharedBuffer> {
    let initial_data = initial.map(|data| D3D11_SUBRESOURCE_DATA {
        pSysMem: data.as_ptr().cast(),
        SysMemPitch: width * 4,
        SysMemSlicePitch: 0,
    });

    let mut texture = None;
    unsafe {
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: (D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX
                    | D3D11_RESOURCE_MISC_SHARED_NTHANDLE)
                    .0 as u32,
            },
            initial_data.map(|r| &r as *const D3D11_SUBRESOURCE_DATA),
            Some(&mut texture),
        )?;
    }

    let texture = texture.context("failed to create shared texture")?;
    let mutex = texture.cast::<IDXGIKeyedMutex>()?;
    let resource = texture.cast::<IDXGIResource1>()?;
    let access = (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0;
    let nt_handle = unsafe {
        resource
            .CreateSharedHandle(None, access, windows::core::PCWSTR::null())?
            .0 as usize as u32
    };

    Ok(SharedBuffer {
        texture,
        mutex,
        nt_handle,
    })
}

pub fn read_u32_le(bytes: &[u8]) -> anyhow::Result<u32> {
    let array: [u8; 4] = bytes
        .get(..4)
        .context("missing u32")?
        .try_into()
        .unwrap();
    Ok(u32::from_le_bytes(array))
}

pub fn read_i32_le(bytes: &[u8]) -> anyhow::Result<i32> {
    let array: [u8; 4] = bytes
        .get(..4)
        .context("missing i32")?
        .try_into()
        .unwrap();
    Ok(i32::from_le_bytes(array))
}

pub fn read_u32_le_at(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    read_u32_le(bytes.get(offset..).context("offset out of range")?)
}

pub fn read_i32_le_at(bytes: &[u8], offset: usize) -> anyhow::Result<i32> {
    read_i32_le(bytes.get(offset..).context("offset out of range")?)
}

pub const CMD_INIT: u8 = 1;
pub const CMD_UPLOAD: u8 = 2;
pub const CMD_SHUTDOWN: u8 = 3;

pub const STATUS_OK: u8 = 0;
pub const STATUS_ERR: u8 = 1;
