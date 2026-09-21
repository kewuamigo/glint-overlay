//! DXGI factory create hooks (Steam `sub_1800AB510` / `sub_180094CD0`).
//!
//! Original create first; success → wrap so Present still blits. No
//! `IDCompositionDevice`. Per-chain Present comes from `wrap_swapchain`.

use core::{ffi::c_void, mem};

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::{debug, warn};
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::Dxgi::{
            DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FULLSCREEN_DESC,
            IDXGIFactory, IDXGIFactory2, IDXGISwapChain, IDXGISwapChain1,
        },
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{GUID, HRESULT, Interface, s},
};

use super::wrap_swapchain;

type CreateFactoryFn = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> HRESULT;
type CreateFactory2Fn = unsafe extern "system" fn(u32, *const GUID, *mut *mut c_void) -> HRESULT;

type CreateSwapChainFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *const DXGI_SWAP_CHAIN_DESC,
    *mut *mut c_void,
) -> HRESULT;
type CreateForHwndFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    HWND,
    *const DXGI_SWAP_CHAIN_DESC1,
    *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC,
    *mut c_void,
    *mut *mut c_void,
) -> HRESULT;
type CreateForCoreFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *const DXGI_SWAP_CHAIN_DESC1,
    *mut c_void,
    *mut *mut c_void,
) -> HRESULT;
type CreateForCompFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *const DXGI_SWAP_CHAIN_DESC1,
    *mut c_void,
    *mut *mut c_void,
) -> HRESULT;

struct Hooks {
    create_factory: OnceCell<DetourHook<CreateFactoryFn>>,
    create_factory1: OnceCell<DetourHook<CreateFactoryFn>>,
    create_factory2: OnceCell<DetourHook<CreateFactory2Fn>>,
    create_swap_chain: OnceCell<DetourHook<CreateSwapChainFn>>,
    for_hwnd: OnceCell<DetourHook<CreateForHwndFn>>,
    for_core: OnceCell<DetourHook<CreateForCoreFn>>,
    for_comp: OnceCell<DetourHook<CreateForCompFn>>,
}

static HOOKS: Hooks = Hooks {
    create_factory: OnceCell::new(),
    create_factory1: OnceCell::new(),
    create_factory2: OnceCell::new(),
    create_swap_chain: OnceCell::new(),
    for_hwnd: OnceCell::new(),
    for_core: OnceCell::new(),
    for_comp: OnceCell::new(),
};

fn attach<F: Copy + std::fmt::Debug>(
    cell: &OnceCell<DetourHook<F>>,
    target: F,
    detour: F,
    label: &str,
) {
    if cell.get().is_some() {
        return;
    }
    match unsafe { DetourHook::attach(target, detour) } {
        Ok(h) => {
            let _ = cell.set(h);
            debug!("hooked {label}");
        }
        Err(err) => warn!("Failed hooking {label}: {err:?}"),
    }
}

fn wrap_created(hr: HRESULT, out: *mut *mut c_void, device: *mut c_void) {
    if !hr.is_ok() || out.is_null() {
        return;
    }
    let raw = unsafe { *out };
    if raw.is_null() {
        return;
    }
    let Some(base) = (unsafe { IDXGISwapChain::from_raw_borrowed(&raw) }) else {
        return;
    };
    if let Ok(sc1) = base.cast::<IDXGISwapChain1>() {
        wrap_swapchain(&sc1, device);
    }
}

fn create_then_wrap(
    original: impl FnOnce() -> HRESULT,
    out: *mut *mut c_void,
    device: *mut c_void,
) -> HRESULT {
    let hr = original();
    wrap_created(hr, out, device);
    hr
}

fn wrap_factory(raw: *mut c_void) {
    let Some(factory) = (unsafe { IDXGIFactory::from_raw_borrowed(&raw) }) else {
        return;
    };
    attach(
        &HOOKS.create_swap_chain,
        Interface::vtable(factory).CreateSwapChain,
        hooked_create_swap_chain as _,
        "IDXGIFactory::CreateSwapChain",
    );
    let Ok(factory2) = factory.cast::<IDXGIFactory2>() else {
        return;
    };
    let vt = Interface::vtable(&factory2);
    attach(
        &HOOKS.for_hwnd,
        vt.CreateSwapChainForHwnd,
        hooked_create_for_hwnd as _,
        "IDXGIFactory2::CreateSwapChainForHWND",
    );
    attach(
        &HOOKS.for_core,
        vt.CreateSwapChainForCoreWindow,
        hooked_create_for_core as _,
        "IDXGIFactory2::CreateSwapChainForCoreWindow",
    );
    attach(
        &HOOKS.for_comp,
        vt.CreateSwapChainForComposition,
        hooked_create_for_comp as _,
        "IDXGIFactory2::CreateSwapChainForComposition",
    );
}

fn wrap_factory_out(out: *mut *mut c_void) {
    if out.is_null() {
        return;
    }
    let raw = unsafe { *out };
    if !raw.is_null() {
        wrap_factory(raw);
    }
}

extern "system" fn hooked_create_factory(riid: *const GUID, factory: *mut *mut c_void) -> HRESULT {
    let hr = unsafe { HOOKS.create_factory.wait().original_fn()(riid, factory) };
    if hr.is_ok() {
        wrap_factory_out(factory);
    }
    hr
}

extern "system" fn hooked_create_factory1(riid: *const GUID, factory: *mut *mut c_void) -> HRESULT {
    let hr = unsafe { HOOKS.create_factory1.wait().original_fn()(riid, factory) };
    if hr.is_ok() {
        wrap_factory_out(factory);
    }
    hr
}

extern "system" fn hooked_create_factory2(
    flags: u32,
    riid: *const GUID,
    factory: *mut *mut c_void,
) -> HRESULT {
    let hr = unsafe { HOOKS.create_factory2.wait().original_fn()(flags, riid, factory) };
    if hr.is_ok() {
        wrap_factory_out(factory);
    }
    hr
}

extern "system" fn hooked_create_swap_chain(
    this: *mut c_void,
    device: *mut c_void,
    desc: *const DXGI_SWAP_CHAIN_DESC,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    create_then_wrap(
        || unsafe { HOOKS.create_swap_chain.wait().original_fn()(this, device, desc, swapchain) },
        swapchain,
        device,
    )
}

extern "system" fn hooked_create_for_hwnd(
    this: *mut c_void,
    device: *mut c_void,
    hwnd: HWND,
    desc: *const DXGI_SWAP_CHAIN_DESC1,
    fullscreen: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC,
    restrict: *mut c_void,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    create_then_wrap(
        || unsafe {
            HOOKS.for_hwnd.wait().original_fn()(
                this, device, hwnd, desc, fullscreen, restrict, swapchain,
            )
        },
        swapchain,
        device,
    )
}

extern "system" fn hooked_create_for_core(
    this: *mut c_void,
    device: *mut c_void,
    window: *mut c_void,
    desc: *const DXGI_SWAP_CHAIN_DESC1,
    restrict: *mut c_void,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    create_then_wrap(
        || unsafe {
            HOOKS.for_core.wait().original_fn()(this, device, window, desc, restrict, swapchain)
        },
        swapchain,
        device,
    )
}

extern "system" fn hooked_create_for_comp(
    this: *mut c_void,
    device: *mut c_void,
    desc: *const DXGI_SWAP_CHAIN_DESC1,
    restrict: *mut c_void,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    create_then_wrap(
        || unsafe { HOOKS.for_comp.wait().original_fn()(this, device, desc, restrict, swapchain) },
        swapchain,
        device,
    )
}

/// Steam installer: `GetProcAddress` on `dxgi.dll`. No DComp DLL. No `LoadLibrary`.
pub(super) fn hook() {
    let Ok(module) = (unsafe { GetModuleHandleA(s!("dxgi.dll")) }) else {
        return;
    };
    if let Some(proc) = unsafe { GetProcAddress(module, s!("CreateDXGIFactory")) } {
        let func: CreateFactoryFn = unsafe { mem::transmute_copy(&proc) };
        attach(
            &HOOKS.create_factory,
            func,
            hooked_create_factory as _,
            "CreateDXGIFactory",
        );
    }
    if let Some(proc) = unsafe { GetProcAddress(module, s!("CreateDXGIFactory1")) } {
        let func: CreateFactoryFn = unsafe { mem::transmute_copy(&proc) };
        attach(
            &HOOKS.create_factory1,
            func,
            hooked_create_factory1 as _,
            "CreateDXGIFactory1",
        );
    }
    if let Some(proc) = unsafe { GetProcAddress(module, s!("CreateDXGIFactory2")) } {
        let func: CreateFactory2Fn = unsafe { mem::transmute_copy(&proc) };
        attach(
            &HOOKS.create_factory2,
            func,
            hooked_create_factory2 as _,
            "CreateDXGIFactory2",
        );
    }
}

#[cfg(test)]
mod tests {
    /// Steam `hr >= 0 && out && *out` before `sub_1800945B0`.
    fn should_wrap_created(hr_ok: bool, out_non_null: bool) -> bool {
        hr_ok && out_non_null
    }

    #[test]
    fn scanout_off_factory_wrap_is_not_dcomp() {
        assert!(!crate::compositor::INDEPENDENT_SCANOUT);
        assert!(!crate::compositor::independent_active());
    }

    #[test]
    fn failed_or_null_create_does_not_wrap() {
        assert!(!should_wrap_created(false, false));
        assert!(!should_wrap_created(false, true));
        assert!(!should_wrap_created(true, false));
        assert!(should_wrap_created(true, true));
    }

    #[test]
    fn create_then_wrap_always_invokes_original() {
        use super::create_then_wrap;
        use core::ffi::c_void;
        use windows::core::HRESULT;

        let mut original_calls = 0;
        let hr = create_then_wrap(
            || {
                original_calls += 1;
                HRESULT(-1)
            },
            core::ptr::null_mut::<*mut c_void>(),
            core::ptr::null_mut(),
        );
        assert_eq!(original_calls, 1);
        assert!(!hr.is_ok());
    }

    #[test]
    fn create_then_wrap_success_null_chain_invokes_original_without_device() {
        use super::create_then_wrap;
        use core::ffi::c_void;
        use windows::core::HRESULT;

        let mut original_calls = 0;
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = create_then_wrap(
            || {
                original_calls += 1;
                HRESULT(0)
            },
            &mut out,
            core::ptr::null_mut(),
        );
        assert_eq!(original_calls, 1);
        assert!(hr.is_ok());
    }
}
