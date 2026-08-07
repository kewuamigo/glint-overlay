//! Resolve DXGI swapchain Present vtable entry via a transient D3D11 device.

use core::ffi::c_void;

use anyhow::Context;
use windows::{
    Win32::{
        Foundation::{HWND, HMODULE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{D3D11CreateDeviceAndSwapChain, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION},
            Dxgi::{
                Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_MODE_DESC, DXGI_RATIONAL, DXGI_SAMPLE_DESC},
                DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD,
                DXGI_USAGE_RENDER_TARGET_OUTPUT,
            },
        },
    },
    core::{Interface, HRESULT},
};

/// `IDXGISwapChain::Present` function pointer (slot 8 in the DXGI vtable).
pub type DxgiPresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT;

/// `IDXGISwapChain1::Present1` function pointer (slot 22 in the DXGI vtable).
/// DX12 titles (and Streamline/FSR3 frame-gen swapchains) present through this.
pub type DxgiPresent1Fn =
    unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> HRESULT;

/// Create a dummy swapchain and return its `Present` entry point.
pub fn resolve_dxgi_present_address() -> anyhow::Result<DxgiPresentFn> {
    let (present, _) = resolve_dxgi_present_addresses()?;
    Ok(present)
}

/// Resolve both `Present` (slot 8) and `Present1` (slot 22) entry points.
/// `Present1` is `None` when the swapchain does not implement `IDXGISwapChain1`.
pub fn resolve_dxgi_present_addresses(
) -> anyhow::Result<(DxgiPresentFn, Option<DxgiPresent1Fn>)> {
    unsafe {
        let mut swap_chain = None;
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: 2,
                Height: 2,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                RefreshRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                ScanlineOrdering: Default::default(),
                Scaling: Default::default(),
            },
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 1,
            OutputWindow: HWND(std::ptr::null_mut()),
            Windowed: true.into(),
            SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
            ..Default::default()
        };

        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&desc),
            Some(&mut swap_chain),
            None,
            None,
            None,
        )
        .context("D3D11CreateDeviceAndSwapChain failed")?;

        let swap_chain = swap_chain.context("swap chain missing after creation")?;
        let vtable = *(Interface::as_raw(&swap_chain) as *const *const usize);
        let present_ptr = *vtable.add(8);
        let present: DxgiPresentFn = std::mem::transmute::<usize, DxgiPresentFn>(present_ptr);

        // Present1 exists only if the object implements IDXGISwapChain1
        // (true on Win8+ / flip-model capable systems).
        let present1 = swap_chain
            .cast::<windows::Win32::Graphics::Dxgi::IDXGISwapChain1>()
            .ok()
            .map(|sc1| {
                let vtable1 = *(Interface::as_raw(&sc1) as *const *const usize);
                std::mem::transmute::<usize, DxgiPresent1Fn>(*vtable1.add(22))
            });

        Ok((present, present1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dxgi_present_address_is_non_null() {
        let Ok(present) = resolve_dxgi_present_address() else {
            eprintln!("skip: DXGI swapchain probe unavailable on this machine");
            return;
        };
        let ptr = present as usize;
        assert!(ptr > 0x1000, "Present vtable slot should be a valid code pointer");
    }
}
