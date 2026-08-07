use std::ffi::c_void;
use std::sync::OnceLock;

use glint_gpu_texture::{DxgiPresent1Fn, DxgiPresentFn};
use minhook_sys::{
    MH_CreateHook, MH_DisableHook, MH_EnableHook, MH_Initialize, MH_OK, MH_RemoveHook,
    MH_Uninitialize,
};
use windows::core::{Result, HRESULT};

use crate::glint_on_native_present;

/// DXGI_PRESENT_TEST — probe-only present, no frame is shown. Don't count it.
const DXGI_PRESENT_TEST: u32 = 0x1;

static ORIGINAL_PRESENT: OnceLock<DxgiPresentFn> = OnceLock::new();
static PRESENT_TARGET: OnceLock<usize> = OnceLock::new();
static ORIGINAL_PRESENT1: OnceLock<DxgiPresent1Fn> = OnceLock::new();
static PRESENT1_TARGET: OnceLock<usize> = OnceLock::new();

pub fn install_hooks() -> Result<()> {
    unsafe {
        let init = MH_Initialize();
        if init != MH_OK && init != 1 {
            // 1 = MH_ERROR_ALREADY_INITIALIZED (e.g. glint-overlay-core loaded first)
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
            ));
        }

        let (present_fn, present1_fn) =
            glint_gpu_texture::resolve_dxgi_present_addresses()
                .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;

        // IDXGISwapChain::Present (slot 8) — DX10/11 titles.
        let present_addr = present_fn as *mut c_void;
        let mut original: *mut c_void = std::ptr::null_mut();
        if MH_CreateHook(present_addr, present_detour as *mut c_void, &mut original) == MH_OK
            && MH_EnableHook(present_addr) == MH_OK
        {
            let _ = ORIGINAL_PRESENT
                .set(std::mem::transmute::<*mut c_void, DxgiPresentFn>(original));
            let _ = PRESENT_TARGET.set(present_addr as usize);
        }

        // IDXGISwapChain1::Present1 (slot 22) — DX12 titles and frame-gen
        // swapchains (Streamline / FSR3) present through this entry point,
        // so without it FG games produce no hook data at all.
        if let Some(present1_fn) = present1_fn {
            let present1_addr = present1_fn as *mut c_void;
            let mut original1: *mut c_void = std::ptr::null_mut();
            if MH_CreateHook(present1_addr, present1_detour as *mut c_void, &mut original1)
                == MH_OK
                && MH_EnableHook(present1_addr) == MH_OK
            {
                let _ = ORIGINAL_PRESENT1
                    .set(std::mem::transmute::<*mut c_void, DxgiPresent1Fn>(original1));
                let _ = PRESENT1_TARGET.set(present1_addr as usize);
            }
        }

        if PRESENT_TARGET.get().is_none() && PRESENT1_TARGET.get().is_none() {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_FAIL,
            ));
        }
    }
    Ok(())
}

pub fn uninstall_hooks() {
    unsafe {
        for target in [PRESENT_TARGET.get(), PRESENT1_TARGET.get()]
            .into_iter()
            .flatten()
        {
            let _ = MH_DisableHook(*target as *mut c_void);
            let _ = MH_RemoveHook(*target as *mut c_void);
        }
        let _ = MH_Uninitialize();
    }
}

unsafe extern "system" fn present_detour(
    this: *mut c_void,
    sync_interval: u32,
    flags: u32,
) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        glint_on_native_present();
    }
    if let Some(original) = ORIGINAL_PRESENT.get() {
        unsafe { original(this, sync_interval, flags) }
    } else {
        HRESULT(0)
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
    if let Some(original) = ORIGINAL_PRESENT1.get() {
        unsafe { original(this, sync_interval, flags, present_params) }
    } else {
        HRESULT(0)
    }
}
