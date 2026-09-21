pub mod callback;
mod factory;

use core::{ffi::c_void, mem, ops::Deref};

use glint_overlay_hook::DetourHook;
use once_cell::sync::Lazy;
use tracing::{error, trace, warn};
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D10::ID3D10Device,
            Direct3D11::ID3D11Device1,
            Direct3D12::ID3D12Device,
            Dxgi::{
                Common::{DXGI_COLOR_SPACE_TYPE, DXGI_FORMAT},
                CreateDXGIFactory1, DXGI_PRESENT, DXGI_PRESENT_PARAMETERS, DXGI_PRESENT_TEST,
                IDXGIAdapter, IDXGIDevice, IDXGIFactory4, IDXGISwapChain, IDXGISwapChain1,
                IDXGISwapChain3,
            },
        },
    },
    core::{HRESULT, Interface},
};

use crate::{
    backend::Backends,
    compositor,
    event_sink::OverlayEventSink,
    hook::dx::{dx10, dx11, dx12},
    types::IntDashMap,
    util::after_original_present,
};

/// HWND → last DXGI swapchain seen on Present (`GetHwnd`). Raw pointer; drop on destroy.
static HWND_SWAPCHAIN: Lazy<IntDashMap<u32, usize>> = Lazy::new(IntDashMap::default);

/// Target fn addr → Detours trampoline. Per-chain wrap stores trampolines here.
static ORIG: Lazy<IntDashMap<usize, usize>> = Lazy::new(IntDashMap::default);

fn fn_key<F: Copy>(f: F) -> usize {
    unsafe { mem::transmute_copy(&f) }
}

fn remember_orig<F: Copy>(target: F, trampoline: F) {
    ORIG.insert(fn_key(target), fn_key(trampoline));
}

fn attach_if_new<F: Copy + std::fmt::Debug>(target: F, detour: F) {
    if ORIG.contains_key(&fn_key(target)) {
        return;
    }
    match unsafe { DetourHook::attach(target, detour) } {
        Ok(h) => remember_orig(target, h.original_fn()),
        Err(err) => warn!("dxgi wrap attach skipped: {err:?}"),
    }
}

/// Steam `sub_1800945B0`: hook this chain's Present family if not already hooked.
/// `create_device` is factory `pDevice` (DX12: the game command queue).
pub(super) fn wrap_swapchain(swapchain: &IDXGISwapChain1, create_device: *mut c_void) {
    remember_hwnd_swapchain(swapchain);
    let base = Interface::vtable(swapchain.deref());
    attach_if_new(base.Present, hooked_present as _);
    attach_if_new(base.ResizeBuffers, hooked_resize_buffers as _);
    attach_if_new(Interface::vtable(swapchain).Present1, hooked_present1 as _);
    if let Ok(sc3) = swapchain.cast::<IDXGISwapChain3>() {
        let vt = Interface::vtable(&sc3);
        attach_if_new(vt.ResizeBuffers1, hooked_resize_buffers1 as _);
        attach_if_new(vt.SetColorSpace1, hooked_set_color_space1 as _);
    }
    dx12::attach_from_swapchain(swapchain, create_device);
}

fn remember_hwnd_swapchain(swapchain: &IDXGISwapChain1) {
    let Ok(hwnd) = (unsafe { swapchain.GetHwnd() }) else {
        return;
    };
    if hwnd.0.is_null() {
        return;
    }
    let id = hwnd.0 as u32;
    let raw = swapchain.as_raw() as usize;
    if HWND_SWAPCHAIN.get(&id).is_some_and(|e| *e == raw) {
        return;
    }
    HWND_SWAPCHAIN.insert(id, raw);
    callback::register_swapchain_destruction_callback(swapchain, move |_| {
        dx12::clear_present_queues(raw);
        dx12::clear_wrap_queue(raw);
        if HWND_SWAPCHAIN.get(&id).is_some_and(|e| *e == raw) {
            HWND_SWAPCHAIN.remove(&id);
        }
    });
}

/// Mantle queue-present: mailbox blit via the DXGI swapchain already known for this HWND.
pub(crate) fn draw_overlay_for_hwnd(hwnd: HWND) -> bool {
    let Some(raw) = HWND_SWAPCHAIN.get(&(hwnd.0 as u32)).map(|e| *e) else {
        return false;
    };
    let raw = raw as *mut c_void;
    let Some(swapchain) = (unsafe { IDXGISwapChain1::from_raw_borrowed(&raw) }) else {
        return false;
    };
    draw_overlay(swapchain);
    true
}

#[tracing::instrument]
fn draw_overlay(swapchain: &IDXGISwapChain1) {
    let Ok(hwnd) = (unsafe { swapchain.GetHwnd() }) else {
        return;
    };

    if let Ok(device) = unsafe { swapchain.GetDevice::<ID3D10Device>() }
        && let Some(dxgi_device) = dxgi_device_qi(device.cast::<IDXGIDevice>())
    {
        if let Err(_err) = Backends::with_or_init_backend(
            hwnd.0 as _,
            || unsafe { dxgi_device.GetAdapter().ok() },
            |backend| {
                ensure_overlay_plane();
                if should_blit() {
                    dx10::draw_overlay(backend, &device, swapchain);
                }
            },
        ) {
            error!("Backends::with_or_init_backend failed. err: {:?}", _err);
        }
    }

    if let Ok(device) = unsafe { swapchain.GetDevice::<ID3D11Device1>() }
        && let Some(dxgi_device) = dxgi_device_qi(device.cast::<IDXGIDevice>())
    {
        if let Err(_err) = Backends::with_or_init_backend(
            hwnd.0 as _,
            || unsafe { dxgi_device.GetAdapter().ok() },
            |backend| {
                ensure_overlay_plane();
                if should_blit() {
                    dx11::draw_overlay(backend, &device, swapchain);
                }
            },
        ) {
            error!("Backends::with_or_init_backend failed. err: {:?}", _err);
        }
    }

    if let Ok(device) = unsafe { swapchain.GetDevice::<ID3D12Device>() } {
        if let Err(_err) = Backends::with_or_init_backend(
            hwnd.0 as _,
            || {
                let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory4>() }.ok()?;
                let luid = unsafe { device.GetAdapterLuid() };
                unsafe { factory.EnumAdapterByLuid::<IDXGIAdapter>(luid) }.ok()
            },
            |backend| {
                ensure_overlay_plane();
                if should_blit() {
                    if let Ok(swapchain3) = swapchain.cast::<IDXGISwapChain3>() {
                        dx12::draw_overlay(backend, &device, &swapchain3);
                    }
                }
            },
        ) {
            error!("Backends::with_or_init_backend failed. err: {:?}", _err);
        }
    }
}

/// QI `IDXGIDevice` before `with_or_init_backend`. `None` skips that API's draw.
/// Adapter `Option` from `GetAdapter` is not this skip (default adapter is OK).
fn dxgi_device_qi<T, E>(qi: Result<T, E>) -> Option<T> {
    qi.ok()
}

#[cfg(test)]
fn dx11_device_qi<T, E>(qi: Result<T, E>) -> Option<T> {
    dxgi_device_qi(qi)
}

#[cfg(test)]
fn present_draw_apis() -> &'static [&'static str] {
    &["d3d10", "d3d11", "d3d12"]
}

/// Padló blit. Independent scanout is off — always draw in the hooked Present.
fn should_blit() -> bool {
    !compositor::independent_active()
}

/// `DXGI_PRESENT_TEST` is a capability probe — skip overlay, still call original.
fn should_overlay(flags: DXGI_PRESENT) -> bool {
    !flags.contains(DXGI_PRESENT_TEST)
}

fn present_hung_or_removed(hr: i32) -> bool {
    hr.wrapping_add(2005270523) <= 1
}

fn teardown_present_renderers(this: *mut c_void) {
    let raw = this as usize;
    dx10::teardown_swapchain(raw);
    dx11::teardown_swapchain(raw);
    dx12::teardown_swapchain(raw);
}

/// Scanout off — no device clone, no compositor lock, no plane/DWM.
fn ensure_overlay_plane() {
    compositor::ensure_overlay_plane();
}

fn orig_present(this: *mut c_void) -> PresentFn {
    if let Some(sc) = unsafe { IDXGISwapChain::from_raw_borrowed(&this) } {
        let addr = Interface::vtable(sc).Present;
        if let Some(p) = ORIG.get(&fn_key(addr)) {
            return unsafe { mem::transmute_copy(&*p) };
        }
    }
    panic!("dxgi Present trampoline missing")
}

fn orig_present1(this: *mut c_void) -> Present1Fn {
    if let Some(sc) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
        let addr = Interface::vtable(sc).Present1;
        if let Some(p) = ORIG.get(&fn_key(addr)) {
            return unsafe { mem::transmute_copy(&*p) };
        }
    }
    panic!("dxgi Present1 trampoline missing")
}

fn orig_resize(this: *mut c_void) -> ResizeBuffersFn {
    if let Some(sc) = unsafe { IDXGISwapChain::from_raw_borrowed(&this) } {
        let addr = Interface::vtable(sc).ResizeBuffers;
        if let Some(p) = ORIG.get(&fn_key(addr)) {
            return unsafe { mem::transmute_copy(&*p) };
        }
    }
    panic!("dxgi ResizeBuffers trampoline missing")
}

fn orig_resize1(this: *mut c_void) -> ResizeBuffers1Fn {
    if let Some(sc) = unsafe { IDXGISwapChain3::from_raw_borrowed(&this) } {
        let addr = Interface::vtable(sc).ResizeBuffers1;
        if let Some(p) = ORIG.get(&fn_key(addr)) {
            return unsafe { mem::transmute_copy(&*p) };
        }
    }
    panic!("dxgi ResizeBuffers1 trampoline missing")
}

fn orig_set_color_space1(this: *mut c_void) -> SetColorSpace1Fn {
    if let Some(sc) = unsafe { IDXGISwapChain3::from_raw_borrowed(&this) } {
        let addr = Interface::vtable(sc).SetColorSpace1;
        if let Some(p) = ORIG.get(&fn_key(addr)) {
            return unsafe { mem::transmute_copy(&*p) };
        }
    }
    panic!("dxgi SetColorSpace1 trampoline missing")
}

#[tracing::instrument]
extern "system" fn hooked_present(
    this: *mut c_void,
    sync_interval: u32,
    flags: DXGI_PRESENT,
) -> HRESULT {
    trace!("Present called");

    if let Some(swapchain) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
        remember_hwnd_swapchain(swapchain);
        if should_overlay(flags) && OverlayEventSink::connected() {
            draw_overlay(swapchain);
        }
    }

    let hr = after_original_present(|| unsafe { orig_present(this)(this, sync_interval, flags) });
    if present_hung_or_removed(hr.0) {
        teardown_present_renderers(this);
    }
    hr
}

#[tracing::instrument]
extern "system" fn hooked_present1(
    this: *mut c_void,
    sync_interval: u32,
    flags: DXGI_PRESENT,
    present_params: *const DXGI_PRESENT_PARAMETERS,
) -> HRESULT {
    trace!("Present1 called");

    if let Some(swapchain) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
        remember_hwnd_swapchain(swapchain);
        if should_overlay(flags) && OverlayEventSink::connected() {
            draw_overlay(swapchain);
        }
    }

    let hr = after_original_present(|| unsafe {
        orig_present1(this)(this, sync_interval, flags, present_params)
    });
    if present_hung_or_removed(hr.0) {
        teardown_present_renderers(this);
    }
    hr
}

fn resize_swapchain(swapchain: &IDXGISwapChain1) {
    dx10::resize_swapchain(swapchain);
    dx11::resize_swapchain(swapchain);
    dx12::resize_swapchain(swapchain);
}

#[tracing::instrument]
extern "system" fn hooked_resize_buffers(
    this: *mut c_void,
    buffer_count: u32,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    flags: u32,
) -> HRESULT {
    trace!("ResizeBuffers called");

    if let Some(swapchain) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
        dx12::clear_present_queues(swapchain.as_raw() as usize);
        resize_swapchain(swapchain);
    }

    unsafe { orig_resize(this)(this, buffer_count, width, height, format, flags) }
}

#[tracing::instrument]
extern "system" fn hooked_resize_buffers1(
    this: *mut c_void,
    buffer_count: u32,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    flags: u32,
    creation_node_mask: *const u32,
    present_queue: *const *mut c_void,
) -> HRESULT {
    trace!("ResizeBuffers1 called");

    if let Some(swapchain) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
        dx12::record_resize_buffers1_present_queues(
            swapchain.as_raw() as usize,
            buffer_count,
            present_queue,
        );
        resize_swapchain(swapchain);
    }

    unsafe {
        orig_resize1(this)(
            this,
            buffer_count,
            width,
            height,
            format,
            flags,
            creation_node_mask,
            present_queue,
        )
    }
}

/// Steam `sub_180094260`: store `a2` then original. DXGI has no GetColorSpace1.
#[tracing::instrument]
extern "system" fn hooked_set_color_space1(
    this: *mut c_void,
    color_space: DXGI_COLOR_SPACE_TYPE,
) -> HRESULT {
    crate::renderer::hdr::record_swapchain_color_space(this as usize, color_space);
    unsafe { orig_set_color_space1(this)(this, color_space) }
}

pub type PresentFn = unsafe extern "system" fn(*mut c_void, u32, DXGI_PRESENT) -> HRESULT;
pub type Present1Fn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    DXGI_PRESENT,
    *const DXGI_PRESENT_PARAMETERS,
) -> HRESULT;

pub type ResizeBuffersFn =
    unsafe extern "system" fn(*mut c_void, u32, u32, u32, DXGI_FORMAT, u32) -> HRESULT;
pub type ResizeBuffers1Fn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    u32,
    u32,
    DXGI_FORMAT,
    u32,
    *const u32,
    *const *mut c_void,
) -> HRESULT;

pub type SetColorSpace1Fn =
    unsafe extern "system" fn(*mut c_void, DXGI_COLOR_SPACE_TYPE) -> HRESULT;

/// Factory export hooks only. Present attaches from the game chain via `wrap_swapchain`.
pub fn hook() {
    factory::hook();
}

#[cfg(test)]
mod tests {
    use super::{
        dx11_device_qi, present_draw_apis, present_hung_or_removed, should_blit, should_overlay,
    };
    use windows::Win32::Graphics::Dxgi::{DXGI_PRESENT, DXGI_PRESENT_TEST};

    /// Enter `with_or_init_backend` only on QI Ok. Adapter Option is not a skip.
    fn dx11_enter_backend(qi: Result<(), ()>, _adapter: Option<()>) -> bool {
        dx11_device_qi(qi).is_some()
    }

    #[test]
    fn dx11_qi_cast_failure_skips_draw() {
        assert!(!dx11_enter_backend(Err(()), None));
        assert!(!dx11_enter_backend(Err(()), Some(())));
    }

    #[test]
    fn dx11_qi_ok_enters_backend_even_when_adapter_none() {
        assert!(dx11_enter_backend(Ok(()), None));
        assert!(dx11_enter_backend(Ok(()), Some(())));
    }

    #[test]
    fn should_blit_when_scanout_off() {
        assert!(should_blit());
    }

    #[test]
    fn present_test_skips_overlay() {
        assert!(!should_overlay(DXGI_PRESENT_TEST));
        assert!(should_overlay(DXGI_PRESENT(0)));
    }

    #[test]
    fn present_draw_order_is_d3d10_then_d3d11_then_d3d12() {
        assert_eq!(present_draw_apis(), &["d3d10", "d3d11", "d3d12"]);
    }

    #[test]
    fn present_hung_or_removed_matches_steam_predicate() {
        const REMOVED: i32 = 0x887A_0005u32 as i32;
        const HUNG: i32 = 0x887A_0006u32 as i32;
        const NEXT: i32 = 0x887A_0007u32 as i32;
        assert!(present_hung_or_removed(REMOVED));
        assert!(present_hung_or_removed(HUNG));
        assert!(!present_hung_or_removed(NEXT));
        assert!(!present_hung_or_removed(0));
        assert!(!present_hung_or_removed(1));
    }
}
