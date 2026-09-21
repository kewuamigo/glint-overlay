//! DirectDraw Flip hooks (Steam `sub_1800AB510` / `sub_18008A430` / `sub_18008A4B0`).
//!
//! Load-gate: `GetModuleHandleA("ddraw.dll")`. Never `LoadLibrary`.
//! `DirectDrawCreate` / `DirectDrawCreateEx` then Flip. HWND mailbox like
//! Mantle/D3D8. Always original. No DDraw compositor. No `d3d.dll`.

use std::{
    mem,
    sync::atomic::{AtomicPtr, Ordering},
};

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::info;
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{GUID, HRESULT, PCSTR, s},
};

use crate::util::after_original_present;

use super::d3d8::blit_mailbox;

/// `IDirectDraw::CreateSurface` / `SetCooperativeLevel` / `IDirectDrawSurface::Flip`.
const CREATE_SURFACE_VTBL: usize = 6;
const SET_COOP_VTBL: usize = 20;
const FLIP_VTBL: usize = 11;

/// Steam `sub_18008A4B0`: wrap CreateEx only for `IID_IDirectDraw7`.
const IID_IDIRECTDRAW7: GUID = GUID {
    data1: 0x15e65ec0,
    data2: 0x3b9c,
    data3: 0x11d2,
    data4: [0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b],
};

type DirectDrawCreateFn = unsafe extern "system" fn(
    *const GUID,
    *mut *mut core::ffi::c_void,
    *mut core::ffi::c_void,
) -> HRESULT;
type DirectDrawCreateExFn = unsafe extern "system" fn(
    *const GUID,
    *mut *mut core::ffi::c_void,
    *const GUID,
    *mut core::ffi::c_void,
) -> HRESULT;
type CreateSurfaceFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    *mut *mut core::ffi::c_void,
    *mut core::ffi::c_void,
) -> HRESULT;
type SetCoopFn = unsafe extern "system" fn(*mut core::ffi::c_void, HWND, u32) -> HRESULT;
type FlipFn =
    unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, u32) -> HRESULT;

struct Hooks {
    create: Option<DetourHook<DirectDrawCreateFn>>,
    create_ex: Option<DetourHook<DirectDrawCreateExFn>>,
    create_surface: OnceCell<DetourHook<CreateSurfaceFn>>,
    set_coop: OnceCell<DetourHook<SetCoopFn>>,
    flip: OnceCell<DetourHook<FlipFn>>,
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();
static COOP_HWND: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(core::ptr::null_mut());

fn ddraw_module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("ddraw.dll")) }.ok()
}

fn is_dd7(iid: *const GUID) -> bool {
    !iid.is_null() && unsafe { *iid } == IID_IDIRECTDRAW7
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

fn wrap_ddraw(iface: *mut core::ffi::c_void) {
    if iface.is_null() {
        return;
    }
    if let Some(create_surface) = unsafe { vtbl::<CreateSurfaceFn>(iface, CREATE_SURFACE_VTBL) } {
        attach_vtbl(
            &HOOKS.wait().create_surface,
            create_surface,
            hooked_create_surface as _,
            "IDirectDraw::CreateSurface",
        );
    }
    if let Some(set_coop) = unsafe { vtbl::<SetCoopFn>(iface, SET_COOP_VTBL) } {
        attach_vtbl(
            &HOOKS.wait().set_coop,
            set_coop,
            hooked_set_coop as _,
            "IDirectDraw::SetCooperativeLevel",
        );
    }
}

fn wrap_surface(surface: *mut core::ffi::c_void) {
    let Some(flip) = (unsafe { vtbl::<FlipFn>(surface, FLIP_VTBL) }) else {
        return;
    };
    attach_vtbl(
        &HOOKS.wait().flip,
        flip,
        hooked_flip as _,
        "IDirectDrawSurface::Flip",
    );
}

fn coop_hwnd() -> HWND {
    HWND(COOP_HWND.load(Ordering::Relaxed))
}

unsafe extern "system" fn hooked_create(
    guid: *const GUID,
    out: *mut *mut core::ffi::c_void,
    outer: *mut core::ffi::c_void,
) -> HRESULT {
    let orig = HOOKS.wait().create.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(guid, out, outer) };
    if hr.is_ok() && !out.is_null() {
        wrap_ddraw(unsafe { *out });
    }
    hr
}

unsafe extern "system" fn hooked_create_ex(
    guid: *const GUID,
    out: *mut *mut core::ffi::c_void,
    iid: *const GUID,
    outer: *mut core::ffi::c_void,
) -> HRESULT {
    let orig = HOOKS.wait().create_ex.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(guid, out, iid, outer) };
    if hr.is_ok() && !out.is_null() && is_dd7(iid) {
        wrap_ddraw(unsafe { *out });
    }
    hr
}

unsafe extern "system" fn hooked_create_surface(
    this: *mut core::ffi::c_void,
    desc: *mut core::ffi::c_void,
    out: *mut *mut core::ffi::c_void,
    outer: *mut core::ffi::c_void,
) -> HRESULT {
    let orig = HOOKS.wait().create_surface.get().unwrap().original_fn();
    let hr = unsafe { orig(this, desc, out, outer) };
    if hr.is_ok() && !out.is_null() {
        wrap_surface(unsafe { *out });
    }
    hr
}

unsafe extern "system" fn hooked_set_coop(
    this: *mut core::ffi::c_void,
    hwnd: HWND,
    flags: u32,
) -> HRESULT {
    if !hwnd.is_invalid() {
        COOP_HWND.store(hwnd.0, Ordering::Relaxed);
    }
    let orig = HOOKS.wait().set_coop.get().unwrap().original_fn();
    unsafe { orig(this, hwnd, flags) }
}

unsafe extern "system" fn hooked_flip(
    this: *mut core::ffi::c_void,
    target: *mut core::ffi::c_void,
    flags: u32,
) -> HRESULT {
    blit_mailbox(coop_hwnd());
    after_original_present(|| {
        let orig = HOOKS.wait().flip.get().unwrap().original_fn();
        unsafe { orig(this, target, flags) }
    })
}

#[tracing::instrument]
pub(super) fn hook() {
    super::late::attach_when_loaded(ddraw_module(), &HOOKS, |module| {
        info!("Game is using ddraw.dll (dx7 or lower)... hooking.");
        Hooks {
            create: attach(
                module,
                s!("DirectDrawCreate"),
                hooked_create,
                "DirectDrawCreate",
            ),
            create_ex: attach(
                module,
                s!("DirectDrawCreateEx"),
                hooked_create_ex,
                "DirectDrawCreateEx",
            ),
            create_surface: OnceCell::new(),
            set_coop: OnceCell::new(),
            flip: OnceCell::new(),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_ddraw_is_ok_and_requires_no_detours() {
        assert!(
            ddraw_module().is_none(),
            "CI must not LoadLibrary ddraw; skip = success"
        );
        hook();
        assert!(
            HOOKS.get().is_none(),
            "miss must not latch empty (late LoadLibrary re-calls hook)"
        );
    }

    #[test]
    fn create_ex_wraps_only_dd7() {
        assert!(is_dd7(&IID_IDIRECTDRAW7));
        let other = GUID::zeroed();
        assert!(!is_dd7(&other));
        assert!(!is_dd7(core::ptr::null()));
        let q = unsafe { *(&IID_IDIRECTDRAW7 as *const GUID as *const [u64; 2]) };
        assert_eq!(q[0], 0x11D23B9C15E65EC0);
        assert_eq!(q[1], 0x5BEA979760002FB9);
    }
}
