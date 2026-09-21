//! Steam HDR blit pick (`sub_180078E50` / `CD3D11HDRtoSDR`).
//!
//! IDA MCP `user-ida-pro-mcp` 2026-08-15, IDB `GameOverlayRenderer64.dll`:
//! - `"Using SCRGB shader for HDR\n"` @ `0x18011A768` — xref `sub_180078E50` @ `0x180079312`
//! - `"Using PQ shader for HDR\n"` @ `0x18011A788` — xref `sub_180078E50` @ `0x180079334`
//! - `"CD3D11HDRtoSDR failed to create tonemap shader: 0x%x\n"` @ `0x1801118F8`
//!   — xref `sub_1800584F0` @ `0x180058521` (`CreatePixelShader` of 8212-byte blob; not copied)
//!
//! `v22` is `*(DWORD*)(a1+384)` written by `SetColorSpace1` hook `sub_180094260` →
//! `sub_180076DA0`. DXGI has no `GetColorSpace1`; we cache that argument like Steam.
//! `1` = `DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709`; `12` =
//! `DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020`. Else SDR mailbox quad.

use once_cell::sync::Lazy;
use windows::{
    Win32::Graphics::Dxgi::{
        Common::{
            DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
            DXGI_COLOR_SPACE_TYPE,
        },
        IDXGISwapChain1, IDXGISwapChain3,
    },
    core::Interface,
};

use crate::types::IntDashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayBlitColorSpace {
    /// Existing `texture.hlsl` `ps_main` mailbox quad.
    Sdr,
    /// Linear-out PS (sRGB mailbox → linear).
    Scrgb,
    /// PQ-out PS (sRGB mailbox → Rec.2020 ST.2084).
    Pq,
}

pub fn overlay_blit_color_space(v22: DXGI_COLOR_SPACE_TYPE) -> OverlayBlitColorSpace {
    match v22 {
        DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709 => OverlayBlitColorSpace::Scrgb,
        DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 => OverlayBlitColorSpace::Pq,
        _ => OverlayBlitColorSpace::Sdr,
    }
}

static LAST: Lazy<IntDashMap<usize, i32>> = Lazy::new(IntDashMap::default);

pub fn record_swapchain_color_space(swapchain: usize, v22: DXGI_COLOR_SPACE_TYPE) {
    LAST.insert(swapchain, v22.0);
}

pub fn forget_swapchain_color_space(swapchain: usize) {
    LAST.remove(&swapchain);
}

pub fn blit_color_space_for(swapchain: &IDXGISwapChain1) -> OverlayBlitColorSpace {
    let key = swapchain
        .cast::<IDXGISwapChain3>()
        .map(|s3| s3.as_raw() as usize)
        .unwrap_or(swapchain.as_raw() as usize);
    overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(
        LAST.get(&key).map(|v| *v).unwrap_or(0),
    ))
}

pub fn blit_color_space_for3(swapchain: &IDXGISwapChain3) -> OverlayBlitColorSpace {
    overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(
        LAST.get(&(swapchain.as_raw() as usize))
            .map(|v| *v)
            .unwrap_or(0),
    ))
}

#[cfg(test)]
mod tests {
    use super::{OverlayBlitColorSpace, overlay_blit_color_space};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_TYPE;

    #[test]
    fn v22_1_is_scrgb() {
        assert_eq!(
            overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(1)),
            OverlayBlitColorSpace::Scrgb
        );
    }

    #[test]
    fn v22_12_is_pq() {
        assert_eq!(
            overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(12)),
            OverlayBlitColorSpace::Pq
        );
    }

    #[test]
    fn v22_other_is_sdr() {
        assert_eq!(
            overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(0)),
            OverlayBlitColorSpace::Sdr
        );
        assert_eq!(
            overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(2)),
            OverlayBlitColorSpace::Sdr
        );
        assert_eq!(
            overlay_blit_color_space(DXGI_COLOR_SPACE_TYPE(17)),
            OverlayBlitColorSpace::Sdr
        );
    }
}
