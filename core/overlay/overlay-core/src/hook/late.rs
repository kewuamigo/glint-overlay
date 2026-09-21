//! Late-load re-init (Steam `LoadLibrary*` → `sub_1800AB510`).
//!
//! Skip when the graphics module is missing. Do not latch empty — re-call
//! after a later load must still attach. No dummy GPU. No basename filter.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::{GetLastError, HANDLE, HMODULE, SetLastError},
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{PCSTR, PCWSTR, s},
};

use super::{d3d8, ddraw, dx, fg, mantle, opengl};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttachWhenLoaded {
    Skipped,
    Attached,
    AlreadyAttached,
}

/// Steam `sub_18008E8D0` analog: attach once when the module is present.
pub(super) fn attach_when_loaded<M, T>(
    module: Option<M>,
    cell: &OnceCell<T>,
    make: impl FnOnce(M) -> T,
) -> AttachWhenLoaded {
    if cell.get().is_some() {
        return AttachWhenLoaded::AlreadyAttached;
    }
    let Some(module) = module else {
        return AttachWhenLoaded::Skipped;
    };
    match cell.set(make(module)) {
        Ok(()) => AttachWhenLoaded::Attached,
        Err(_) => AttachWhenLoaded::AlreadyAttached,
    }
}

/// Steam `sub_1800AB510` graphics block. Idempotent; missing DLLs skip.
pub(super) fn graphics_reinstall() {
    dx::hook();
    fg::hook();
    d3d8::hook();
    ddraw::hook();
    opengl::hook();
    mantle::hook();
}

type LoadLibraryAFn = unsafe extern "system" fn(PCSTR) -> HMODULE;
type LoadLibraryWFn = unsafe extern "system" fn(PCWSTR) -> HMODULE;
type LoadLibraryExAFn = unsafe extern "system" fn(PCSTR, HANDLE, u32) -> HMODULE;
type LoadLibraryExWFn = unsafe extern "system" fn(PCWSTR, HANDLE, u32) -> HMODULE;

struct Hooks {
    kernelbase_exw: Option<DetourHook<LoadLibraryExWFn>>,
    kernelbase_exa: Option<DetourHook<LoadLibraryExAFn>>,
    kernel32_exw: Option<DetourHook<LoadLibraryExWFn>>,
    kernel32_exa: Option<DetourHook<LoadLibraryExAFn>>,
    kernel32_w: Option<DetourHook<LoadLibraryWFn>>,
    kernel32_a: Option<DetourHook<LoadLibraryAFn>>,
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();

fn attach<F: Copy + std::fmt::Debug>(
    dll: PCSTR,
    name: PCSTR,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let module = unsafe { GetModuleHandleA(dll) }.ok()?;
    let proc = unsafe { GetProcAddress(module, name) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => {
            info!("hooked {label}");
            Some(h)
        }
        Err(err) => {
            warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

fn after_load() {
    let last = unsafe { GetLastError() };
    graphics_reinstall();
    unsafe { SetLastError(last) };
}

fn reinit_from_wide() {
    if HOOKS.get().is_some_and(|h| h.kernelbase_exw.is_some()) {
        return;
    }
    after_load();
}

fn reinit_from_ansi() {
    if HOOKS.get().is_some_and(|h| h.kernelbase_exa.is_some()) {
        return;
    }
    after_load();
}

unsafe extern "system" fn hooked_kernelbase_exw(name: PCWSTR, file: HANDLE, flags: u32) -> HMODULE {
    let Some(hook) = HOOKS.wait().kernelbase_exw.as_ref() else {
        return HMODULE::default();
    };
    let module = unsafe { hook.original_fn()(name, file, flags) };
    after_load();
    module
}

unsafe extern "system" fn hooked_kernel32_exw(name: PCWSTR, file: HANDLE, flags: u32) -> HMODULE {
    let Some(hook) = HOOKS.wait().kernel32_exw.as_ref() else {
        return HMODULE::default();
    };
    let module = unsafe { hook.original_fn()(name, file, flags) };
    reinit_from_wide();
    module
}

unsafe extern "system" fn hooked_kernel32_w(name: PCWSTR) -> HMODULE {
    let Some(hook) = HOOKS.wait().kernel32_w.as_ref() else {
        return HMODULE::default();
    };
    let module = unsafe { hook.original_fn()(name) };
    reinit_from_wide();
    module
}

unsafe extern "system" fn hooked_kernelbase_exa(name: PCSTR, file: HANDLE, flags: u32) -> HMODULE {
    let Some(hook) = HOOKS.wait().kernelbase_exa.as_ref() else {
        return HMODULE::default();
    };
    let module = unsafe { hook.original_fn()(name, file, flags) };
    reinit_from_wide();
    module
}

unsafe extern "system" fn hooked_kernel32_exa(name: PCSTR, file: HANDLE, flags: u32) -> HMODULE {
    let Some(hook) = HOOKS.wait().kernel32_exa.as_ref() else {
        return HMODULE::default();
    };
    let module = unsafe { hook.original_fn()(name, file, flags) };
    reinit_from_ansi();
    module
}

unsafe extern "system" fn hooked_kernel32_a(name: PCSTR) -> HMODULE {
    let Some(hook) = HOOKS.wait().kernel32_a.as_ref() else {
        return HMODULE::default();
    };
    let module = unsafe { hook.original_fn()(name) };
    reinit_from_ansi();
    module
}

/// Steam DllMain `LoadLibraryA` / `W` / `ExA` / `ExW` sites. Fail-soft.
pub(super) fn hook() {
    let _ = HOOKS.set(Hooks {
        kernelbase_exw: attach(
            s!("kernelbase.dll"),
            s!("LoadLibraryExW"),
            hooked_kernelbase_exw,
            "kernelbase LoadLibraryExW",
        ),
        kernelbase_exa: attach(
            s!("kernelbase.dll"),
            s!("LoadLibraryExA"),
            hooked_kernelbase_exa,
            "kernelbase LoadLibraryExA",
        ),
        kernel32_exw: attach(
            s!("kernel32.dll"),
            s!("LoadLibraryExW"),
            hooked_kernel32_exw,
            "kernel32 LoadLibraryExW",
        ),
        kernel32_exa: attach(
            s!("kernel32.dll"),
            s!("LoadLibraryExA"),
            hooked_kernel32_exa,
            "kernel32 LoadLibraryExA",
        ),
        kernel32_w: attach(
            s!("kernel32.dll"),
            s!("LoadLibraryW"),
            hooked_kernel32_w,
            "kernel32 LoadLibraryW",
        ),
        kernel32_a: attach(
            s!("kernel32.dll"),
            s!("LoadLibraryA"),
            hooked_kernel32_a,
            "kernel32 LoadLibraryA",
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_then_present_attaches_without_gpu() {
        let cell = OnceCell::new();
        let mut attaches = 0;

        assert_eq!(
            attach_when_loaded(None::<()>, &cell, |_| {
                attaches += 1;
                "hooked"
            }),
            AttachWhenLoaded::Skipped
        );
        assert_eq!(attaches, 0);
        assert!(cell.get().is_none());

        assert_eq!(
            attach_when_loaded(Some(()), &cell, |_| {
                attaches += 1;
                "hooked"
            }),
            AttachWhenLoaded::Attached
        );
        assert_eq!(attaches, 1);
        assert_eq!(cell.get().copied(), Some("hooked"));

        assert_eq!(
            attach_when_loaded(Some(()), &cell, |_| {
                attaches += 1;
                "again"
            }),
            AttachWhenLoaded::AlreadyAttached
        );
        assert_eq!(attaches, 1);
    }
}
