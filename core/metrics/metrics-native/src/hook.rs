use std::ffi::c_void;
use std::sync::OnceLock;

use minhook_sys::{
    MH_CreateHook, MH_DisableHook, MH_EnableHook, MH_Initialize, MH_OK, MH_RemoveHook,
    MH_Uninitialize,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIFactory, IDXGIFactory1, IDXGIFactory2, IDXGISwapChain, IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetModuleHandleW, GetProcAddress};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::core::PCWSTR;
use windows::core::{GUID, HRESULT, Interface, Result, s};

use crate::glint_on_native_present;

const DXGI_PRESENT_TEST: u32 = 0x1;

type CreateFactoryFn = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> HRESULT;
type CreateFactory2Fn = unsafe extern "system" fn(u32, *const GUID, *mut *mut c_void) -> HRESULT;
type CreateSwapChainFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *const c_void,
    *mut *mut c_void,
) -> HRESULT;
type CreateForHwndFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *const c_void,
    *const c_void,
    *mut c_void,
    *mut *mut c_void,
) -> HRESULT;
type CreateForCoreFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *const c_void,
    *mut c_void,
    *mut *mut c_void,
) -> HRESULT;
type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> HRESULT;

struct Orig<F> {
    target: usize,
    trampoline: F,
}

static ORIG_FACTORY: OnceLock<Orig<CreateFactoryFn>> = OnceLock::new();
static ORIG_FACTORY1: OnceLock<Orig<CreateFactoryFn>> = OnceLock::new();
static ORIG_FACTORY2: OnceLock<Orig<CreateFactory2Fn>> = OnceLock::new();
static ORIG_CREATE_SC: OnceLock<Orig<CreateSwapChainFn>> = OnceLock::new();
static ORIG_FOR_HWND: OnceLock<Orig<CreateForHwndFn>> = OnceLock::new();
static ORIG_FOR_CORE: OnceLock<Orig<CreateForCoreFn>> = OnceLock::new();
static ORIG_PRESENT: OnceLock<Orig<PresentFn>> = OnceLock::new();
static ORIG_PRESENT1: OnceLock<Orig<Present1Fn>> = OnceLock::new();

unsafe fn attach<F: Copy>(
    target: *mut c_void,
    detour: *mut c_void,
    slot: &OnceLock<Orig<F>>,
) -> bool {
    if slot.get().is_some() || target.is_null() {
        return slot.get().is_some();
    }
    let mut original = std::ptr::null_mut();
    if unsafe { MH_CreateHook(target, detour, &mut original) == MH_OK && MH_EnableHook(target) == MH_OK }
    {
        let _ = slot.set(Orig {
            target: target as usize,
            trampoline: unsafe { std::mem::transmute_copy(&original) },
        });
        true
    } else {
        false
    }
}

fn hook_present_vtables(raw: *mut c_void) {
    let Some(sc) = (unsafe { IDXGISwapChain::from_raw_borrowed(&raw) }) else {
        return;
    };
    let present = Interface::vtable(sc).Present;
    unsafe {
        attach(
            present as *mut c_void,
            present_detour as *mut c_void,
            &ORIG_PRESENT,
        );
    }
    if let Ok(sc1) = sc.cast::<IDXGISwapChain1>() {
        let present1 = Interface::vtable(&sc1).Present1;
        unsafe {
            attach(
                present1 as *mut c_void,
                present1_detour as *mut c_void,
                &ORIG_PRESENT1,
            );
        }
    }
}

fn wrap_created(hr: HRESULT, out: *mut *mut c_void) {
    if !hr.is_ok() || out.is_null() {
        return;
    }
    let raw = unsafe { *out };
    if !raw.is_null() {
        hook_present_vtables(raw);
    }
}

fn wrap_factory(raw: *mut c_void) {
    let Some(factory) = (unsafe { IDXGIFactory::from_raw_borrowed(&raw) }) else {
        return;
    };
    let create = Interface::vtable(factory).CreateSwapChain;
    unsafe {
        attach(
            create as *mut c_void,
            hooked_create_swap_chain as *mut c_void,
            &ORIG_CREATE_SC,
        );
    }
    let Ok(factory2) = factory.cast::<IDXGIFactory2>() else {
        return;
    };
    let vt = Interface::vtable(&factory2);
    unsafe {
        attach(
            vt.CreateSwapChainForHwnd as *mut c_void,
            hooked_create_for_hwnd as *mut c_void,
            &ORIG_FOR_HWND,
        );
        attach(
            vt.CreateSwapChainForCoreWindow as *mut c_void,
            hooked_create_for_core as *mut c_void,
            &ORIG_FOR_CORE,
        );
    }
}

fn system_dxgi() -> Option<windows::Win32::Foundation::HMODULE> {
    let mut buf = [0u16; 296];
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n + 10 >= buf.len() {
        return None;
    }
    let suffix: Vec<u16> = "\\dxgi.dll".encode_utf16().collect();
    buf[n..n + suffix.len()].copy_from_slice(&suffix);
    buf[n + suffix.len()] = 0;
    unsafe { GetModuleHandleW(PCWSTR(buf.as_ptr())) }.ok()
}

pub fn install_hooks() -> Result<()> {
    unsafe {
        let init = MH_Initialize();
        if init != MH_OK && init != 1 {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
        let Some(module) = system_dxgi().or_else(|| GetModuleHandleA(s!("dxgi.dll")).ok()) else {
            return Ok(());
        };
        if let Some(proc) = GetProcAddress(module, s!("CreateDXGIFactory")) {
            attach(
                proc as *mut c_void,
                hooked_create_factory as *mut c_void,
                &ORIG_FACTORY,
            );
        }
        if let Some(proc) = GetProcAddress(module, s!("CreateDXGIFactory1")) {
            attach(
                proc as *mut c_void,
                hooked_create_factory1 as *mut c_void,
                &ORIG_FACTORY1,
            );
        }
        if let Some(proc) = GetProcAddress(module, s!("CreateDXGIFactory2")) {
            attach(
                proc as *mut c_void,
                hooked_create_factory2 as *mut c_void,
                &ORIG_FACTORY2,
            );
        }
        seed_factory_vtable();
        hook_stock_present_fallback();
    }
    Ok(())
}

fn seed_factory_vtable() {
    let Some(orig) = ORIG_FACTORY1
        .get()
        .map(|o| o.trampoline)
        .or_else(|| ORIG_FACTORY.get().map(|o| o.trampoline))
    else {
        return;
    };
    let mut factory = std::ptr::null_mut();
    let iid = IDXGIFactory1::IID;
    if unsafe { orig(&iid, &mut factory) }.is_ok() && !factory.is_null() {
        wrap_factory(factory);
        drop(unsafe { IDXGIFactory1::from_raw(factory) });
    }
}

fn hook_stock_present_fallback() {
    if ORIG_PRESENT.get().is_some() {
        return;
    }
    let Ok((present, present1)) = glint_gpu_texture::resolve_dxgi_present_addresses() else {
        return;
    };
    unsafe {
        attach(
            present as *mut c_void,
            present_detour as *mut c_void,
            &ORIG_PRESENT,
        );
        if let Some(p1) = present1 {
            attach(
                p1 as *mut c_void,
                present1_detour as *mut c_void,
                &ORIG_PRESENT1,
            );
        }
    }
}

pub fn uninstall_hooks() {
    unsafe {
        for target in [
            ORIG_FACTORY.get().map(|o| o.target),
            ORIG_FACTORY1.get().map(|o| o.target),
            ORIG_FACTORY2.get().map(|o| o.target),
            ORIG_CREATE_SC.get().map(|o| o.target),
            ORIG_FOR_HWND.get().map(|o| o.target),
            ORIG_FOR_CORE.get().map(|o| o.target),
            ORIG_PRESENT.get().map(|o| o.target),
            ORIG_PRESENT1.get().map(|o| o.target),
        ]
        .into_iter()
        .flatten()
        {
            let _ = MH_DisableHook(target as *mut c_void);
            let _ = MH_RemoveHook(target as *mut c_void);
        }
        let _ = MH_Uninitialize();
    }
}

unsafe extern "system" fn hooked_create_factory(
    riid: *const GUID,
    factory: *mut *mut c_void,
) -> HRESULT {
    let hr = match ORIG_FACTORY.get() {
        Some(o) => unsafe { (o.trampoline)(riid, factory) },
        None => HRESULT(0),
    };
    if hr.is_ok() && !factory.is_null() {
        wrap_factory(unsafe { *factory });
    }
    hr
}

unsafe extern "system" fn hooked_create_factory1(
    riid: *const GUID,
    factory: *mut *mut c_void,
) -> HRESULT {
    let hr = match ORIG_FACTORY1.get() {
        Some(o) => unsafe { (o.trampoline)(riid, factory) },
        None => HRESULT(0),
    };
    if hr.is_ok() && !factory.is_null() {
        wrap_factory(unsafe { *factory });
    }
    hr
}

unsafe extern "system" fn hooked_create_factory2(
    flags: u32,
    riid: *const GUID,
    factory: *mut *mut c_void,
) -> HRESULT {
    let hr = match ORIG_FACTORY2.get() {
        Some(o) => unsafe { (o.trampoline)(flags, riid, factory) },
        None => HRESULT(0),
    };
    if hr.is_ok() && !factory.is_null() {
        wrap_factory(unsafe { *factory });
    }
    hr
}

unsafe extern "system" fn hooked_create_swap_chain(
    this: *mut c_void,
    device: *mut c_void,
    desc: *const c_void,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    let hr = match ORIG_CREATE_SC.get() {
        Some(o) => unsafe { (o.trampoline)(this, device, desc, swapchain) },
        None => HRESULT(0),
    };
    wrap_created(hr, swapchain);
    hr
}

unsafe extern "system" fn hooked_create_for_hwnd(
    this: *mut c_void,
    device: *mut c_void,
    hwnd: *mut c_void,
    desc: *const c_void,
    fullscreen: *const c_void,
    restrict: *mut c_void,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    let hr = match ORIG_FOR_HWND.get() {
        Some(o) => unsafe { (o.trampoline)(this, device, hwnd, desc, fullscreen, restrict, swapchain) },
        None => HRESULT(0),
    };
    wrap_created(hr, swapchain);
    hr
}

unsafe extern "system" fn hooked_create_for_core(
    this: *mut c_void,
    device: *mut c_void,
    window: *mut c_void,
    desc: *const c_void,
    restrict: *mut c_void,
    swapchain: *mut *mut c_void,
) -> HRESULT {
    let hr = match ORIG_FOR_CORE.get() {
        Some(o) => unsafe { (o.trampoline)(this, device, window, desc, restrict, swapchain) },
        None => HRESULT(0),
    };
    wrap_created(hr, swapchain);
    hr
}

unsafe extern "system" fn present_detour(
    this: *mut c_void,
    sync_interval: u32,
    flags: u32,
) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        glint_on_native_present();
    }
    match ORIG_PRESENT.get() {
        Some(o) => unsafe { (o.trampoline)(this, sync_interval, flags) },
        None => HRESULT(0),
    }
}

unsafe extern "system" fn present1_detour(
    this: *mut c_void,
    sync_interval: u32,
    flags: u32,
    present_params: *const c_void,
) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        glint_on_native_present();
    }
    match ORIG_PRESENT1.get() {
        Some(o) => unsafe { (o.trampoline)(this, sync_interval, flags, present_params) },
        None => HRESULT(0),
    }
}
