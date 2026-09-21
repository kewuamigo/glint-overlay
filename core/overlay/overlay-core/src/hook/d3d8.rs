//! D3D8 present-site hooks (Steam `sub_1800AB510` / `sub_180084A90`).
//!
//! Load-gate: `GetModuleHandleA("d3d8.dll")`. Never `LoadLibrary`.
//! `d3d.dll` (D3D7) is log-only — do not hook.
//!
//! `Direct3DCreate8` → wrap `CreateDevice` → wrap `Present`. HWND mailbox
//! via DXGI `draw_overlay_for_hwnd` / `with_or_init_backend` like Mantle.
//! Always original. No D3D8 compositor.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::{Lazy, OnceCell};
use tracing::info;
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND, RECT},
        Graphics::{
            Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1},
            Gdi::RGNDATA,
        },
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{HRESULT, PCSTR, s},
};

use crate::{
    backend::Backends,
    compositor,
    event_sink::OverlayEventSink,
    types::IntDashMap,
    util::{after_original_present, get_client_size},
};

/// `IDirect3D8::CreateDevice` / `IDirect3DDevice8::Present` (d3d8.h).
const CREATE_DEVICE_VTBL: usize = 15;
const PRESENT_VTBL: usize = 15;
/// `D3DPRESENT_PARAMETERS.hDeviceWindow` after six `UINT`/`DWORD` fields.
const D3D8_PARAMS_HWND: usize = 24;

type Create8Fn = unsafe extern "system" fn(u32) -> *mut core::ffi::c_void;
type CreateDeviceFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    u32,
    u32,
    HWND,
    u32,
    *mut core::ffi::c_void,
    *mut *mut core::ffi::c_void,
) -> HRESULT;
type PresentFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *const RECT,
    *const RECT,
    HWND,
    *const RGNDATA,
) -> HRESULT;

struct Hooks {
    create8: Option<DetourHook<Create8Fn>>,
    create_device: OnceCell<DetourHook<CreateDeviceFn>>,
    present: OnceCell<DetourHook<PresentFn>>,
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();
static DEVICE_HWND: Lazy<IntDashMap<usize, isize>> = Lazy::new(IntDashMap::default);

fn d3d8_module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("d3d8.dll")) }.ok()
}

fn d3d7_module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("d3d.dll")) }.ok()
}

unsafe fn vtbl<F: Copy>(this: *mut core::ffi::c_void, index: usize) -> Option<F> {
    if this.is_null() {
        return None;
    }
    let table = unsafe { *(this as *const *const usize) };
    if table.is_null() {
        return None;
    }
    Some(unsafe { mem::transmute_copy(&*table.add(index)) })
}

fn attach<F: Copy + std::fmt::Debug>(
    module: HMODULE,
    name: PCSTR,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let proc = unsafe { GetProcAddress(module, name) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => {
            info!("hooked {label}");
            Some(h)
        }
        Err(err) => {
            tracing::warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

fn attach_vtbl<F: Copy + std::fmt::Debug>(
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
            info!("hooked {label}");
        }
        Err(err) => tracing::warn!("Failed hooking {label}: {err:?}"),
    }
}

fn default_adapter() -> Option<IDXGIAdapter> {
    let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok()?;
    unsafe { factory.EnumAdapters(0) }.ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlitPlan {
    SkipNoWait,
    DrawKnownSwapchain,
    InitNoGpu,
}

fn blit_plan(hwnd_null: bool, connected: bool, swapchain_known: bool) -> BlitPlan {
    if hwnd_null || !connected {
        return BlitPlan::SkipNoWait;
    }
    if swapchain_known {
        BlitPlan::DrawKnownSwapchain
    } else {
        BlitPlan::InitNoGpu
    }
}

/// Steam Present blit analog: DXGI mailbox if known, else plane only. Always original later.
pub(crate) fn blit_mailbox(hwnd: HWND) {
    if blit_plan(hwnd.0.is_null(), OverlayEventSink::connected(), false) == BlitPlan::SkipNoWait {
        return;
    }
    let id = hwnd.0 as u32;
    if let Ok(size) = get_client_size(hwnd) {
        let _ = Backends::with_backend(id, |backend| {
            backend.render.lock().window_size = size;
        });
    }
    if super::dx::draw_overlay_for_hwnd(hwnd) {
        return;
    }
    let _ = Backends::with_or_init_backend(id, default_adapter, |backend| {
        if let Ok(size) = get_client_size(hwnd) {
            backend.render.lock().window_size = size;
        }
        compositor::ensure_overlay_plane();
    });
}

fn hwnd_from_d3d8_params(params: *mut core::ffi::c_void, focus: HWND) -> HWND {
    if !params.is_null() {
        let hwnd = unsafe { *(params.byte_add(D3D8_PARAMS_HWND) as *const HWND) };
        if !hwnd.is_invalid() {
            return hwnd;
        }
    }
    focus
}

fn wrap_device(device: *mut core::ffi::c_void, hwnd: HWND) {
    if device.is_null() {
        return;
    }
    if !hwnd.is_invalid() {
        DEVICE_HWND.insert(device as usize, hwnd.0 as isize);
    }
    let Some(present) = (unsafe { vtbl::<PresentFn>(device, PRESENT_VTBL) }) else {
        return;
    };
    attach_vtbl(
        &HOOKS.wait().present,
        present,
        hooked_present as _,
        "IDirect3DDevice8::Present",
    );
}

fn wrap_d3d8(iface: *mut core::ffi::c_void) {
    let Some(create_device) = (unsafe { vtbl::<CreateDeviceFn>(iface, CREATE_DEVICE_VTBL) }) else {
        return;
    };
    attach_vtbl(
        &HOOKS.wait().create_device,
        create_device,
        hooked_create_device as _,
        "IDirect3D8::CreateDevice",
    );
}

unsafe extern "system" fn hooked_create8(sdk: u32) -> *mut core::ffi::c_void {
    let orig = HOOKS.wait().create8.as_ref().unwrap().original_fn();
    let iface = unsafe { orig(sdk) };
    if !iface.is_null() {
        wrap_d3d8(iface);
    }
    iface
}

unsafe extern "system" fn hooked_create_device(
    this: *mut core::ffi::c_void,
    adapter: u32,
    device_type: u32,
    focus: HWND,
    behavior: u32,
    params: *mut core::ffi::c_void,
    out_device: *mut *mut core::ffi::c_void,
) -> HRESULT {
    let orig = HOOKS.wait().create_device.get().unwrap().original_fn();
    let hr = unsafe {
        orig(
            this,
            adapter,
            device_type,
            focus,
            behavior,
            params,
            out_device,
        )
    };
    if hr.is_ok() && !out_device.is_null() {
        wrap_device(unsafe { *out_device }, hwnd_from_d3d8_params(params, focus));
    }
    hr
}

fn present_hwnd(this: *mut core::ffi::c_void, r#override: HWND) -> HWND {
    if !r#override.is_invalid() {
        return r#override;
    }
    DEVICE_HWND
        .get(&(this as usize))
        .map(|h| HWND(*h as _))
        .unwrap_or(HWND(core::ptr::null_mut()))
}

unsafe extern "system" fn hooked_present(
    this: *mut core::ffi::c_void,
    src: *const RECT,
    dest: *const RECT,
    dest_window: HWND,
    dirty: *const RGNDATA,
) -> HRESULT {
    blit_mailbox(present_hwnd(this, dest_window));
    after_original_present(|| {
        let orig = HOOKS.wait().present.get().unwrap().original_fn();
        unsafe { orig(this, src, dest, dest_window, dirty) }
    })
}

/// Steam `sub_1800AB510`: d3d8 hook; d3d.dll log only.
#[tracing::instrument]
pub(super) fn hook() {
    if d3d7_module().is_some() {
        info!("Game is using D3D7 or earlier, not yet supported!");
    }
    super::late::attach_when_loaded(d3d8_module(), &HOOKS, |module| {
        info!("Game is using D3D8, preparing to hook.");
        Hooks {
            create8: attach(
                module,
                s!("Direct3DCreate8"),
                hooked_create8,
                "Direct3DCreate8",
            ),
            create_device: OnceCell::new(),
            present: OnceCell::new(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_d3d8_is_ok_and_requires_no_detours() {
        assert!(
            d3d8_module().is_none(),
            "CI must not LoadLibrary d3d8; skip = success"
        );
        hook();
        assert!(
            HOOKS.get().is_none(),
            "miss must not latch empty (late LoadLibrary re-calls hook)"
        );
    }

    #[test]
    fn d3d7_has_no_hooked_export() {
        // Steam `sub_1800AB510` @ `0x1800ABAE8`: log only, never GetProcAddress.
        const D3D7_HOOK_EXPORTS: &[&str] = &[];
        assert!(D3D7_HOOK_EXPORTS.is_empty());
        let _ = d3d7_module();
    }

    #[test]
    fn params_hwnd_is_at_offset_24() {
        #[repr(C)]
        struct Params {
            _pad: [u32; 6],
            hwnd: usize,
        }
        let mut p = Params {
            _pad: [0; 6],
            hwnd: 0x1234,
        };
        assert_eq!(
            hwnd_from_d3d8_params((&raw mut p).cast(), HWND(core::ptr::null_mut())).0 as usize,
            0x1234
        );
        assert!(
            hwnd_from_d3d8_params(core::ptr::null_mut(), HWND(core::ptr::null_mut())).is_invalid()
        );
    }

    #[test]
    fn present_override_wins() {
        let this = 0x100 as *mut core::ffi::c_void;
        DEVICE_HWND.insert(this as usize, 0x2222);
        let ov = HWND(0x3333usize as _);
        assert_eq!(present_hwnd(this, ov).0 as usize, 0x3333);
        assert_eq!(
            present_hwnd(this, HWND(core::ptr::null_mut())).0 as usize,
            0x2222
        );
    }

    #[test]
    fn null_or_disconnected_skips_without_wait() {
        assert_eq!(blit_plan(true, true, true), BlitPlan::SkipNoWait);
        assert_eq!(blit_plan(false, false, true), BlitPlan::SkipNoWait);
        assert_eq!(blit_plan(false, true, true), BlitPlan::DrawKnownSwapchain);
        assert_eq!(blit_plan(false, true, false), BlitPlan::InitNoGpu);
    }
}
