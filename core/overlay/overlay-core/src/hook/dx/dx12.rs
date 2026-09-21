mod rtv;
mod util;

pub use util::original_execute_command_lists;

use core::ffi::c_void;

use once_cell::sync::Lazy;
use tracing::{debug, trace};
use windows::{
    Win32::{
        Graphics::{
            Direct3D12::{D3D12_COMMAND_LIST_TYPE_DIRECT, ID3D12CommandQueue, ID3D12Device},
            Dxgi::{IDXGISwapChain1, IDXGISwapChain3},
        },
        System::LibraryLoader::GetModuleHandleA,
    },
    core::{IUnknown, Interface, s},
};

use crate::{
    backend::{WindowBackend, render::Renderer},
    hook::dx::{
        dx12::rtv::RtvDescriptors,
        dxgi::callback::register_swapchain_destruction_callback,
        render_gate::{DrawGate, adopt_renderer},
        renderer_map::with_map_entry,
    },
    renderer::dx12::Dx12Renderer,
    types::IntDashMap,
};

/// Mapping from [`IDXGISwapChain3`] to [`RendererData`].
static RENDERERS: Lazy<IntDashMap<usize, RendererData>> = Lazy::new(IntDashMap::default);

/// Per-swapchain `ResizeBuffers1` `ppPresentQueue` raw pointers (length = buffer count).
static PRESENT_QUEUE_MAP: Lazy<IntDashMap<usize, Vec<usize>>> = Lazy::new(IntDashMap::default);

/// Per-swapchain wrap `pDevice` when it QIs as `ID3D12CommandQueue` (Steam wrap a4).
static WRAP_QUEUE_MAP: Lazy<IntDashMap<usize, usize>> = Lazy::new(IntDashMap::default);

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
            register_swapchain_destruction_callback(swapchain, cleanup_swapchain);
        }),
        f,
    )
}

pub(crate) fn pick_dx12_overlay_queue(
    mapped: Option<&[usize]>,
    index: u32,
    wrap: Option<usize>,
) -> Option<usize> {
    mapped
        .and_then(|slots| slots.get(index as usize).copied().filter(|&ptr| ptr != 0))
        .or(wrap)
}

pub(crate) fn store_wrap_queue(swapchain: usize, queue: usize) {
    WRAP_QUEUE_MAP.insert(swapchain, queue);
}

pub(crate) fn get_wrap_queue(swapchain: usize) -> Option<usize> {
    WRAP_QUEUE_MAP.get(&swapchain).map(|e| *e)
}

pub(crate) fn clear_wrap_queue(swapchain: usize) {
    WRAP_QUEUE_MAP.remove(&swapchain);
}

pub(crate) fn store_present_queues(swapchain: usize, queues: &[usize]) {
    PRESENT_QUEUE_MAP.insert(swapchain, queues.to_vec());
}

pub(crate) fn get_present_queues(swapchain: usize) -> Option<Vec<usize>> {
    PRESENT_QUEUE_MAP.get(&swapchain).map(|e| e.clone())
}

pub(crate) fn clear_present_queues(swapchain: usize) {
    PRESENT_QUEUE_MAP.remove(&swapchain);
}

/// Copy `ppPresentQueue` (null clears; `buffer_count == 0` keeps previous).
pub(crate) fn record_resize_buffers1_present_queues(
    swapchain: usize,
    buffer_count: u32,
    present_queue: *const *mut c_void,
) {
    if present_queue.is_null() {
        clear_present_queues(swapchain);
        return;
    }
    if buffer_count == 0 {
        return;
    }
    let queues = unsafe {
        // SAFETY: DXGI `ppPresentQueue` has `buffer_count` entries when non-null.
        core::slice::from_raw_parts(present_queue, buffer_count as usize)
            .iter()
            .map(|&p| p as usize)
            .collect::<Vec<_>>()
    };
    store_present_queues(swapchain, &queues);
}

pub fn draw_overlay(backend: &WindowBackend, device: &ID3D12Device, swapchain: &IDXGISwapChain3) {
    if dx12_hooks_disabled() {
        return;
    }
    let swapchain_key = swapchain.as_raw() as usize;
    let mapped = get_present_queues(swapchain_key);
    let wrap = get_wrap_queue(swapchain_key);
    let backbuffer_index = unsafe { swapchain.GetCurrentBackBufferIndex() };
    let Some(queue_ptr) = pick_dx12_overlay_queue(mapped.as_deref(), backbuffer_index, wrap) else {
        return;
    };
    let queue_raw = queue_ptr as *mut c_void;
    let Some(queue) = (unsafe { ID3D12CommandQueue::from_raw_borrowed(&queue_raw).cloned() })
    else {
        return;
    };
    if unsafe { queue.GetDesc() }.Type != D3D12_COMMAND_LIST_TYPE_DIRECT {
        return;
    }

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
    let interop = render.interop.device.clone();
    drop(render);

    if layers.is_empty() {
        return;
    }

    _ = with_or_init_renderer_data(swapchain, move |data| {
        trace!("using dx12 renderer");

        let color_space = crate::renderer::hdr::blit_color_space_for3(swapchain);
        data.rtv
            .with_next_swapchain(device, swapchain, backbuffer_index as _, |desc| {
                let draw_res = (|| {
                    for (layer, update, position, size) in &layers {
                        if let Some(update) = update {
                            data.renderer.update_texture(*layer, update.clone());
                        }
                        if size.0 == 0 || size.1 == 0 {
                            continue;
                        }
                        let res = data.renderer.draw(
                            device,
                            &interop,
                            swapchain,
                            backbuffer_index,
                            desc,
                            *layer,
                            *position,
                            *size,
                            screen,
                            color_space,
                        );
                        trace!("dx12 render: {:?}", res);
                        res?;
                    }
                    Ok(())
                })();
                let finish_res = data.renderer.finish(&queue);
                draw_res.and(finish_res)
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

pub(crate) fn teardown_swapchain(swapchain: usize) {
    crate::renderer::hdr::forget_swapchain_color_space(swapchain);
    clear_present_queues(swapchain);
    clear_wrap_queue(swapchain);
    if RENDERERS.remove(&swapchain).is_some() {
        debug!("dx12 renderer cleanup");
    }
}

#[tracing::instrument]
fn cleanup_swapchain(swapchain: usize) {
    teardown_swapchain(swapchain);
}

/// Steam `SteamOverlayDisableDX12` — skip when the value starts with `'1'`.
pub(crate) fn steam_overlay_disable_dx12(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.starts_with('1'))
}

fn dx12_hooks_disabled() -> bool {
    steam_overlay_disable_dx12(std::env::var("SteamOverlayDisableDX12").ok().as_deref())
}

/// `IDXGISwapChain::GetDevice(IID_ID3D12Device)` — not QI, not `D3D12CreateDevice`.
pub(crate) fn attach_from_swapchain(swapchain: &IDXGISwapChain1, create_device: *mut c_void) {
    if dx12_hooks_disabled() {
        return;
    }
    if unsafe { swapchain.GetDevice::<ID3D12Device>() }.is_err() {
        return;
    }
    if let Some(queue) = queue_from_create_device(create_device) {
        store_wrap_queue(swapchain.as_raw() as usize, queue.as_raw() as usize);
    }
}

fn queue_from_create_device(ptr: *mut c_void) -> Option<ID3D12CommandQueue> {
    if ptr.is_null() {
        return None;
    }
    let unk = unsafe { IUnknown::from_raw_borrowed(&ptr)? };
    unk.cast().ok()
}

/// GetModuleHandle only. No `LoadLibrary`, no dummy device.
pub fn hook() {
    let Ok(_) = (unsafe { GetModuleHandleA(s!("d3d12.dll")) }) else {
        return;
    };
}

#[cfg(test)]
mod tests {
    use core::{ffi::c_void, ptr};

    use super::{
        clear_present_queues, clear_wrap_queue, get_present_queues, get_wrap_queue,
        pick_dx12_overlay_queue, record_resize_buffers1_present_queues,
        steam_overlay_disable_dx12, store_present_queues, store_wrap_queue,
    };

    #[test]
    fn steam_overlay_disable_dx12_starts_with_one() {
        assert!(steam_overlay_disable_dx12(Some("1")));
        assert!(steam_overlay_disable_dx12(Some("1foo")));
        assert!(!steam_overlay_disable_dx12(Some("0")));
        assert!(!steam_overlay_disable_dx12(Some("")));
        assert!(!steam_overlay_disable_dx12(Some("true")));
        assert!(!steam_overlay_disable_dx12(None));
    }

    #[test]
    fn pick_dx12_overlay_queue_mapped_hit_uses_slot_not_wrap() {
        assert_eq!(
            pick_dx12_overlay_queue(Some(&[0x10, 0x20]), 1, Some(0x99)),
            Some(0x20)
        );
    }

    #[test]
    fn pick_dx12_overlay_queue_wrap_used_when_map_none() {
        assert_eq!(pick_dx12_overlay_queue(None, 0, Some(0xAA)), Some(0xAA));
    }

    #[test]
    fn pick_dx12_overlay_queue_mapped_beats_wrap() {
        assert_eq!(
            pick_dx12_overlay_queue(Some(&[0x10, 0x20]), 0, Some(0xAA)),
            Some(0x10)
        );
    }

    #[test]
    fn pick_dx12_overlay_queue_mapped_zero_slot_uses_wrap_not_last_direct() {
        assert_eq!(
            pick_dx12_overlay_queue(Some(&[0x10, 0]), 1, Some(0xAA)),
            Some(0xAA)
        );
    }

    #[test]
    fn pick_dx12_overlay_queue_no_wrap_no_map_is_none() {
        assert_eq!(pick_dx12_overlay_queue(None, 0, None), None);
    }

    #[test]
    fn pick_dx12_overlay_queue_mapped_oob_uses_wrap() {
        assert_eq!(
            pick_dx12_overlay_queue(Some(&[0x10, 0x20]), 5, Some(0xAA)),
            Some(0xAA)
        );
    }

    #[test]
    fn pick_dx12_overlay_queue_empty_map_uses_wrap() {
        assert_eq!(pick_dx12_overlay_queue(Some(&[]), 0, Some(0xAA)), Some(0xAA));
    }

    #[test]
    fn store_wrap_queue_roundtrip_with_fake_key() {
        let key = 0xD12_3101;
        store_wrap_queue(key, 0xAA);
        assert_eq!(get_wrap_queue(key), Some(0xAA));
        clear_wrap_queue(key);
    }

    #[test]
    fn get_wrap_queue_missing_key_is_none() {
        assert_eq!(get_wrap_queue(0xD12_3102), None);
    }

    #[test]
    fn clear_wrap_queue_removes_entry() {
        let key = 0xD12_3103;
        store_wrap_queue(key, 0xAA);
        clear_wrap_queue(key);
        assert_eq!(get_wrap_queue(key), None);
    }

    #[test]
    fn clear_present_queues_does_not_clear_wrap() {
        let key = 0xD12_3104;
        store_wrap_queue(key, 0xAA);
        store_present_queues(key, &[0x10]);
        clear_present_queues(key);
        assert_eq!(get_wrap_queue(key), Some(0xAA));
        clear_wrap_queue(key);
    }

    #[test]
    fn record_resize_buffers1_null_keeps_wrap() {
        let key = 0xD12_3105;
        store_wrap_queue(key, 0xAA);
        store_present_queues(key, &[0x10, 0x20]);
        record_resize_buffers1_present_queues(key, 2, ptr::null());
        assert_eq!(get_present_queues(key), None);
        assert_eq!(get_wrap_queue(key), Some(0xAA));
        clear_wrap_queue(key);
    }

    #[test]
    fn store_present_queues_roundtrip_with_fake_key() {
        let key = 0xD12_2101;
        store_present_queues(key, &[0x10, 0x20]);
        assert_eq!(get_present_queues(key).as_deref(), Some(&[0x10, 0x20][..]));
        clear_present_queues(key);
    }

    #[test]
    fn get_present_queues_missing_key_is_none() {
        assert_eq!(get_present_queues(0xD12_2102), None);
    }

    #[test]
    fn store_present_queues_replaces_previous() {
        let key = 0xD12_2103;
        store_present_queues(key, &[0x10]);
        store_present_queues(key, &[0x30, 0]);
        assert_eq!(get_present_queues(key).as_deref(), Some(&[0x30, 0][..]));
        clear_present_queues(key);
    }

    #[test]
    fn clear_present_queues_removes_entry() {
        let key = 0xD12_2104;
        store_present_queues(key, &[0x10]);
        clear_present_queues(key);
        assert_eq!(get_present_queues(key), None);
    }

    #[test]
    fn record_resize_buffers1_copies_non_null_queues() {
        let key = 0xD12_2105;
        let queues: [*mut c_void; 2] = [0x10 as *mut c_void, 0x20 as *mut c_void];
        record_resize_buffers1_present_queues(key, 2, queues.as_ptr());
        assert_eq!(get_present_queues(key).as_deref(), Some(&[0x10, 0x20][..]));
        clear_present_queues(key);
    }

    #[test]
    fn record_resize_buffers1_null_does_not_abort_and_clears() {
        let key = 0xD12_2106;
        store_present_queues(key, &[0x10, 0x20]);
        record_resize_buffers1_present_queues(key, 2, ptr::null());
        assert_eq!(get_present_queues(key), None);
    }

    #[test]
    fn record_resize_buffers1_zero_count_keeps_previous() {
        let key = 0xD12_2107;
        let queues: [*mut c_void; 2] = [0x10 as *mut c_void, 0x20 as *mut c_void];
        record_resize_buffers1_present_queues(key, 2, queues.as_ptr());
        record_resize_buffers1_present_queues(key, 0, queues.as_ptr());
        assert_eq!(get_present_queues(key).as_deref(), Some(&[0x10, 0x20][..]));
        clear_present_queues(key);
    }
}
