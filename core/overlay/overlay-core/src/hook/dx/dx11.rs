use once_cell::sync::Lazy;
use scopeguard::defer;
use tracing::{debug, trace};
use windows::{
    Win32::Graphics::{
        Direct3D::D3D_FEATURE_LEVEL_11_0,
        Direct3D11::{
            D3D11_1_CREATE_DEVICE_CONTEXT_STATE_SINGLETHREADED, D3D11_CREATE_DEVICE_SINGLETHREADED,
            D3D11_SDK_VERSION, ID3D11Device, ID3D11Device1, ID3D11Texture2D,
            ID3DDeviceContextState,
        },
        Dxgi::IDXGISwapChain1,
    },
    core::Interface,
};

use crate::{
    backend::{WindowBackend, render::Renderer},
    hook::dx::{
        dxgi::callback::register_swapchain_destruction_callback,
        render_gate::{adopt_renderer, DrawGate},
        renderer_map::with_map_entry,
    },
    renderer::dx11::Dx11Renderer,
    types::IntDashMap,
};

/// Mapping from [`IDXGISwapChain1`] to [`RendererData`].
static RENDERERS: Lazy<IntDashMap<usize, RendererData>> = Lazy::new(IntDashMap::default);

struct RendererData {
    renderer: Dx11Renderer,
    state: ID3DDeviceContextState,
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
            debug!("initializing dx11 renderer");
            let device = unsafe { swapchain.GetDevice::<ID3D11Device1>()? };

            let state = unsafe {
                let mut state = None;
                let flag = if device.GetCreationFlags() & D3D11_CREATE_DEVICE_SINGLETHREADED.0 != 0
                {
                    D3D11_1_CREATE_DEVICE_CONTEXT_STATE_SINGLETHREADED.0 as u32
                } else {
                    0
                };

                device
                    .CreateDeviceContextState(
                        flag,
                        &[D3D_FEATURE_LEVEL_11_0],
                        D3D11_SDK_VERSION,
                        &ID3D11Device::IID,
                        None,
                        Some(&mut state),
                    )
                    .expect("CreateDeviceContextState failed");

                state.unwrap()
            };

            Ok(RendererData {
                renderer: Dx11Renderer::new(&device)?,
                state,
            })
        },
        Some(|| register_swapchain_destruction_callback(swapchain, cleanup_swapchain)),
        f,
    )
}

pub fn draw_overlay(backend: &WindowBackend, device: &ID3D11Device1, swapchain: &IDXGISwapChain1) {
    let mut render = backend.render.lock();
    if !matches!(
        adopt_renderer(&mut render.renderer, Renderer::Dx11, &[Renderer::Opengl]),
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
        trace!("using dx11 renderer");

        let cx = unsafe { device.GetImmediateContext1().unwrap() };
        let mut prev_state = None;
        unsafe {
            cx.SwapDeviceContextState(&data.state, Some(&mut prev_state));
        }

        let prev_state = prev_state.unwrap();
        defer!(unsafe {
            cx.SwapDeviceContextState(&prev_state, None);
        });

        let back_buffer = unsafe { swapchain.GetBuffer::<ID3D11Texture2D>(0) }
            .expect("failed to get dx11 backbuffer");
        let mut rtv = None;
        unsafe { device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv)) }
            .expect("failed to create rtv");
        let rtv = rtv.unwrap();

        unsafe { cx.OMSetRenderTargets(Some(&[Some(rtv)]), None) };
        defer!(unsafe { cx.OMSetRenderTargets(None, None) });

        for (layer, update, position, size) in layers {
            if let Some(update) = update {
                data.renderer.update_texture(layer, update);
            }
            // Zero size = layer cleared this frame (texture cache already dropped).
            if size.0 == 0 || size.1 == 0 {
                continue;
            }
            let res = data.renderer.draw(device, &cx, layer, position, size, screen);
            trace!("dx11 render: {:?}", res);
            res?;
        }
        Ok(())
    });
}

#[tracing::instrument]
fn cleanup_swapchain(swapchain: usize) {
    if RENDERERS.remove(&swapchain).is_none() {
        return;
    };
    debug!("dx11 renderer cleanup");
}
