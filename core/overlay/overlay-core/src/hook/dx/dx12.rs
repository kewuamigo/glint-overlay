mod rtv;
mod util;

pub use util::original_execute_command_lists;

use core::ffi::c_void;

use anyhow::Context;
use glint_overlay_hook::DetourHook;
use once_cell::sync::{Lazy, OnceCell};
use tracing::{debug, trace};
use windows::{
    Win32::Graphics::{
        Direct3D::D3D_FEATURE_LEVEL_11_0,
        Direct3D12::{
            D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
            D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device,
        },
        Dxgi::{IDXGISwapChain1, IDXGISwapChain3},
    },
    core::Interface,
};

use crate::{
    backend::{WindowBackend, render::Renderer},
    hook::dx::{
        dx12::rtv::RtvDescriptors, dxgi::callback::register_swapchain_destruction_callback,
        render_gate::{adopt_renderer, DrawGate},
        renderer_map::with_map_entry,
    },
    renderer::dx12::Dx12Renderer,
    types::IntDashMap,
};

struct WeakID3D12CommandQueue(*mut c_void);
unsafe impl Send for WeakID3D12CommandQueue {}
unsafe impl Sync for WeakID3D12CommandQueue {}

static QUEUE_MAP: Lazy<IntDashMap<usize, WeakID3D12CommandQueue>> = Lazy::new(IntDashMap::default);

/// Mapping from [`IDXGISwapChain3`] to [`RendererData`].
static RENDERERS: Lazy<IntDashMap<usize, RendererData>> = Lazy::new(IntDashMap::default);

struct RendererData {
    renderer: Dx12Renderer,
    rtv: RtvDescriptors,
}

#[inline]
fn with_or_init_renderer_data<R>(
    swapchain: &IDXGISwapChain3,
    f: impl FnOnce(&mut RendererData) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    let key = swapchain.as_raw() as usize;
    let device_raw = unsafe { swapchain.GetDevice::<ID3D12Device>()?.as_raw() as usize };
    with_map_entry(
        &RENDERERS,
        key,
        || {
            debug!("initializing dx12 renderer");
            let device = unsafe { swapchain.GetDevice::<ID3D12Device>()? };
            Ok(RendererData {
                renderer: Dx12Renderer::new(&device, swapchain)?,
                rtv: RtvDescriptors::new(&device)?,
            })
        },
        Some(|| {
            register_swapchain_destruction_callback(swapchain, move |this| {
                cleanup_swapchain(this, device_raw)
            });
        }),
        f,
    )
}

#[tracing::instrument]
fn get_queue_for(device: &ID3D12Device) -> Option<ID3D12CommandQueue> {
    Some(unsafe {
        ID3D12CommandQueue::from_raw_borrowed(&QUEUE_MAP.remove(&(device.as_raw() as _))?.1.0)
            .unwrap()
            .clone()
    })
}

pub fn draw_overlay(backend: &WindowBackend, device: &ID3D12Device, swapchain: &IDXGISwapChain3) {
    let Some(queue) = get_queue_for(device) else {
        return;
    };

    let mut render = backend.render.lock();
    if !matches!(
        adopt_renderer(
            &mut render.renderer,
            Renderer::Dx12,
            &[Renderer::Opengl, Renderer::Vulkan],
        ),
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
        trace!("using dx12 renderer");

        let backbuffer_index = unsafe { swapchain.GetCurrentBackBufferIndex() };
        data.rtv
            .with_next_swapchain(device, swapchain, backbuffer_index as _, |desc| {
                for (layer, update, position, size) in &layers {
                    if let Some(update) = update {
                        data.renderer.update_texture(*layer, update.clone());
                    }
                    if size.0 == 0 || size.1 == 0 {
                        continue;
                    }
                    let res = data.renderer.draw(
                        device,
                        swapchain,
                        backbuffer_index,
                        desc,
                        &queue,
                        *layer,
                        *position,
                        *size,
                        screen,
                    );
                    trace!("dx12 render: {:?}", res);
                    res?;
                }
                Ok(())
            })
    });
}

pub fn resize_swapchain(swapchain: &IDXGISwapChain1) {
    let Some(mut data) = RENDERERS.get_mut(&(swapchain.as_raw() as _)) else {
        return;
    };

    // invalidate old rtv descriptors
    data.rtv.reset();
}

#[tracing::instrument]
fn cleanup_swapchain(swapchain: usize, device: usize) {
    if RENDERERS.remove(&swapchain).is_none() {
        return;
    };
    debug!("dx12 renderer cleanup");

    QUEUE_MAP.remove(&device);
}

#[tracing::instrument]
extern "system" fn hooked_execute_command_lists(
    this: *mut c_void,
    num_command_lists: u32,
    pp_commmand_lists: *const *mut c_void,
) {
    trace!("ExecuteCommandLists called");

    unsafe {
        let queue = ID3D12CommandQueue::from_raw_borrowed(&this).unwrap();

        if queue.GetDesc().Type == D3D12_COMMAND_LIST_TYPE_DIRECT {
            let mut device = None;
            queue.GetDevice::<ID3D12Device>(&mut device).unwrap();
            let device = device.unwrap();

            trace!(
                "found DIRECT command queue {:?} for device {:?}",
                queue, device
            );
            QUEUE_MAP.insert(device.as_raw() as _, WeakID3D12CommandQueue(queue.as_raw()));
        }

        HOOK.execute_command_lists.wait().original_fn()(this, num_command_lists, pp_commmand_lists)
    }
}

type ExecuteCommandListsFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);

struct Hook {
    execute_command_lists: OnceCell<DetourHook<ExecuteCommandListsFn>>,
}

static HOOK: Hook = Hook {
    execute_command_lists: OnceCell::new(),
};

pub fn hook() -> anyhow::Result<()> {
    let execute_command_lists =
        get_execute_command_lists_addr().context("failed to load dx12 addrs")?;
    HOOK.execute_command_lists.get_or_try_init(|| unsafe {
        debug!("hooking ID3D12CommandQueue::ExecuteCommandLists");
        DetourHook::attach(execute_command_lists, hooked_execute_command_lists as _)
    })?;

    Ok(())
}

/// Get pointer to ID3D12CommandQueue::ExecuteCommandLists of D3D12_COMMAND_LIST_TYPE_DIRECT type by creating dummy device
#[tracing::instrument]
fn get_execute_command_lists_addr() -> anyhow::Result<ExecuteCommandListsFn> {
    unsafe {
        let mut device = None;
        D3D12CreateDevice::<_, ID3D12Device>(None, D3D_FEATURE_LEVEL_11_0, &mut device)?;
        let device = device.context("cannot create IDirect3DDevice12")?;

        let queue = device.CreateCommandQueue::<ID3D12CommandQueue>(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            ..Default::default()
        })?;
        let addr = Interface::vtable(&queue).ExecuteCommandLists;
        debug!("ID3D12CommandQueue::ExecuteCommandLists found: {:p}", addr);

        Ok(addr)
    }
}
