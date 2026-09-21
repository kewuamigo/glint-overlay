use once_cell::sync::Lazy;
use scopeguard::defer;
use tracing::{debug, trace};
use windows::{
    Win32::{
        Graphics::{
            Direct3D10::{
                D3D10CreateStateBlock, D3D10StateBlockMaskEnableAll, D3D10_STATE_BLOCK_MASK,
                ID3D10Device, ID3D10StateBlock, ID3D10Texture2D,
            },
            Dxgi::IDXGISwapChain1,
        },
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{Interface, s},
};

use crate::{
    backend::{WindowBackend, render::Renderer},
    hook::dx::{
        dxgi::callback::register_swapchain_destruction_callback,
        render_gate::{DrawGate, adopt_renderer},
        renderer_map::with_map_entry,
    },
    renderer::dx10::Dx10Renderer,
    types::IntDashMap,
};

static RENDERERS: Lazy<IntDashMap<usize, RendererData>> = Lazy::new(IntDashMap::default);

struct RendererData {
    renderer: Dx10Renderer,
    state: ID3D10StateBlock,
    rtv: Option<windows::Win32::Graphics::Direct3D10::ID3D10RenderTargetView>,
}

fn d3d10_state_block_ready() -> bool {
    let Ok(module) = (unsafe { GetModuleHandleA(s!("d3d10.dll")) }) else {
        return false;
    };
    unsafe { GetProcAddress(module, s!("D3D10StateBlockMaskEnableAll")) }.is_some()
        && unsafe { GetProcAddress(module, s!("D3D10CreateStateBlock")) }.is_some()
}

fn capture_state(device: &ID3D10Device) -> anyhow::Result<ID3D10StateBlock> {
    let mut mask = D3D10_STATE_BLOCK_MASK::default();
    unsafe { D3D10StateBlockMaskEnableAll(&mut mask) }?;
    let block = unsafe { D3D10CreateStateBlock(device, &mask) }?;
    unsafe { block.Capture() }?;
    Ok(block)
}

#[inline]
fn with_or_init_renderer_data<R>(
    swapchain: &IDXGISwapChain1,
    f: impl FnOnce(&mut RendererData) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    let key = swapchain.as_raw() as usize;
    with_map_entry(
        &RENDERERS,
        key,
        || {
            debug!("initializing dx10 renderer");
            let device = unsafe { swapchain.GetDevice::<ID3D10Device>()? };
            Ok(RendererData {
                renderer: Dx10Renderer::new(&device)?,
                state: capture_state(&device)?,
                rtv: None,
            })
        },
        Some(|| register_swapchain_destruction_callback(swapchain, cleanup_swapchain)),
        f,
    )
}

pub fn draw_overlay(backend: &WindowBackend, device: &ID3D10Device, swapchain: &IDXGISwapChain1) {
    if !d3d10_state_block_ready() {
        return;
    }

    let mut render = backend.render.lock();
    if !matches!(
        adopt_renderer(&mut render.renderer, Renderer::Dx10, &[Renderer::Opengl]),
        DrawGate::Draw
    ) {
        return;
    }

    let layers = render.draw_layers();
    let screen = render.window_size;
    drop(render);

    if layers.is_empty() {
        return;
    }

    _ = with_or_init_renderer_data(swapchain, move |data| {
        trace!("using dx10 renderer");

        let state = data.state.clone();
        unsafe { state.Capture() }?;
        defer!(unsafe {
            _ = state.Apply();
        });

        let Ok(back_buffer) = (unsafe { swapchain.GetBuffer::<ID3D10Texture2D>(0) }) else {
            return Ok(());
        };
        let Some(rtv) = super::cached_rtv_get_or_insert(&mut data.rtv, || {
            let mut rtv = None;
            if unsafe { device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv)) }.is_err()
            {
                return None;
            }
            rtv
        }) else {
            return Ok(());
        };
        let rtv = rtv.clone();

        unsafe { device.OMSetRenderTargets(Some(&[Some(rtv)]), None) };
        defer!(unsafe { device.OMSetRenderTargets(None, None) });

        let color_space = crate::renderer::hdr::blit_color_space_for(swapchain);
        for (layer, update, position, size) in layers {
            if let Some(update) = update {
                data.renderer.update_texture(layer, update);
            }
            if size.0 == 0 || size.1 == 0 {
                continue;
            }
            let res = data
                .renderer
                .draw(device, layer, position, size, screen, color_space);
            trace!("dx10 render: {:?}", res);
            res?;
        }
        Ok(())
    });
}

pub fn resize_swapchain(swapchain: &IDXGISwapChain1) {
    let Some(mut data) = RENDERERS.get_mut(&(swapchain.as_raw() as _)) else {
        return;
    };
    super::cached_rtv_invalidate(&mut data.rtv);
}

pub(crate) fn teardown_swapchain(swapchain: usize) {
    cleanup_swapchain(swapchain);
}

#[tracing::instrument]
fn cleanup_swapchain(swapchain: usize) {
    crate::renderer::hdr::forget_swapchain_color_space(swapchain);
    if RENDERERS.remove(&swapchain).is_none() {
        return;
    };
    debug!("dx10 renderer cleanup");
}
