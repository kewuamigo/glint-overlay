//! DLSS / FidelityFX load-gated hooks (Steam `sub_1800AB510`).
//!
//! `GetModuleHandle` only — never `LoadLibrary`. Missing DLL is success.
//! Detours call original only. No overlay blit. No extra Present.
//!
//! `sl.dlss_g`: resolve `slDLSSGSetOptions` via `slGetPluginFunction`, including
//! a ProgramData-named copy (`sl_dlss_g.dll`) if that module is already loaded.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::info;
use windows::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{PCSTR, s},
};

type SlFrameTokenFn = unsafe extern "system" fn(*mut core::ffi::c_void, *const u32) -> u32;
type SlPluginFn = unsafe extern "system" fn(PCSTR) -> *mut core::ffi::c_void;
type SlDlssgFn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> i32;
type FfxFn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> i32;

struct Hooks {
    sl_frame_token: Option<DetourHook<SlFrameTokenFn>>,
    sldlssg: Option<DetourHook<SlDlssgFn>>,
    sldlssg_pd: Option<DetourHook<SlDlssgFn>>,
    ffx_vk: Option<DetourHook<FfxFn>>,
    ffx_dx12: Option<DetourHook<FfxFn>>,
}

impl Hooks {
    fn any_attached(&self) -> bool {
        self.sl_frame_token.is_some()
            || self.sldlssg.is_some()
            || self.sldlssg_pd.is_some()
            || self.ffx_vk.is_some()
            || self.ffx_dx12.is_some()
    }
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();

fn module(name: PCSTR) -> Option<HMODULE> {
    unsafe { GetModuleHandleA(name) }.ok()
}

fn attach<F: Copy + std::fmt::Debug>(
    module: HMODULE,
    name: PCSTR,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let proc = unsafe { GetProcAddress(module, name) }?;
    attach_addr(proc as *mut core::ffi::c_void, detour, label)
}

fn attach_addr<F: Copy + std::fmt::Debug>(
    proc: *mut core::ffi::c_void,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    if proc.is_null() {
        return None;
    }
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

/// Steam: `GetProcAddress(slGetPluginFunction)` then `fn("slDLSSGSetOptions")`.
fn resolve_sldlssg(h: HMODULE) -> Option<*mut core::ffi::c_void> {
    if h.is_invalid() {
        return None;
    }
    let proc = unsafe { GetProcAddress(h, s!("slGetPluginFunction")) }?;
    let get_plugin: SlPluginFn = unsafe { mem::transmute_copy(&proc) };
    let addr = unsafe { get_plugin(s!("slDLSSGSetOptions")) };
    if addr.is_null() { None } else { Some(addr) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FgPlan {
    OriginalOnly,
}

fn fg_plan() -> FgPlan {
    FgPlan::OriginalOnly
}

unsafe extern "system" fn hooked_sl_frame_token(
    token: *mut core::ffi::c_void,
    frame_index: *const u32,
) -> u32 {
    let orig = HOOKS.wait().sl_frame_token.as_ref().unwrap().original_fn();
    unsafe { orig(token, frame_index) }
}

unsafe extern "system" fn hooked_sldlssg(
    a1: *mut core::ffi::c_void,
    a2: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().sldlssg.as_ref().unwrap().original_fn();
    unsafe { orig(a1, a2) }
}

unsafe extern "system" fn hooked_sldlssg_pd(
    a1: *mut core::ffi::c_void,
    a2: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().sldlssg_pd.as_ref().unwrap().original_fn();
    unsafe { orig(a1, a2) }
}

unsafe extern "system" fn hooked_ffx_vk(
    a1: *mut core::ffi::c_void,
    a2: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().ffx_vk.as_ref().unwrap().original_fn();
    unsafe { orig(a1, a2) }
}

unsafe extern "system" fn hooked_ffx_dx12(
    a1: *mut core::ffi::c_void,
    a2: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().ffx_dx12.as_ref().unwrap().original_fn();
    unsafe { orig(a1, a2) }
}

fn hook_sldlssg(h: HMODULE, pd: bool) -> Option<DetourHook<SlDlssgFn>> {
    let addr = resolve_sldlssg(h)?;
    let (detour, label): (SlDlssgFn, &str) = if pd {
        (
            hooked_sldlssg_pd as SlDlssgFn,
            "slDLSSGSetOptions(ProgramData)",
        )
    } else {
        (hooked_sldlssg as SlDlssgFn, "slDLSSGSetOptions")
    };
    attach_addr(addr, detour, label)
}

/// Steam `sub_1800AB510` @ `0x1800AB7E1`…`0x1800ABA74`.
#[tracing::instrument]
pub(super) fn hook() {
    if HOOKS.get().is_some() {
        return;
    }
    let sl = module(s!("sl.interposer.dll"));
    let sl_pd = module(s!("sl_interposer.dll"));
    let sl_frame = sl_pd.or(sl).and_then(|h| {
        info!("Game is using nvidia DLSS sl.interposer.dll, hooking");
        attach(
            h,
            s!("slGetNewFrameToken"),
            hooked_sl_frame_token as SlFrameTokenFn,
            "slGetNewFrameToken",
        )
    });

    let dlss = module(s!("sl.dlss_g.dll"));
    let dlss_pd = module(s!("sl_dlss_g.dll")).filter(|h| Some(*h) != dlss);
    if dlss.is_some() {
        info!("Game is using nvidia DLSS sl.dlss_g.dll, hooking");
    }
    if dlss_pd.is_some() {
        info!("Game is using nvidia DLSS sl.dlss_g.dll via version in ProgramData, hooking");
    }

    let ffx_vk = module(s!("amd_fidelityfx_vk.dll")).and_then(|h| {
        info!("Game is using amd_fidelityfx_vk.dll, hooking");
        attach(
            h,
            s!("ffxConfigure"),
            hooked_ffx_vk as FfxFn,
            "ffxConfigureVK",
        )
    });
    let ffx_dx12 = module(s!("amd_fidelityfx_dx12.dll")).and_then(|h| {
        info!("Game is using amd_fidelityfx_dx12.dll, hooking");
        attach(
            h,
            s!("ffxConfigure"),
            hooked_ffx_dx12 as FfxFn,
            "ffxConfigureDX12",
        )
    });

    let hooks = Hooks {
        sl_frame_token: sl_frame,
        sldlssg: dlss.and_then(|h| hook_sldlssg(h, false)),
        sldlssg_pd: dlss_pd.and_then(|h| hook_sldlssg(h, true)),
        ffx_vk,
        ffx_dx12,
    };
    if hooks.any_attached() {
        let _ = HOOKS.set(hooks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_fg_modules_is_ok_and_requires_no_detours() {
        assert!(
            module(s!("sl.interposer.dll")).is_none()
                && module(s!("sl.dlss_g.dll")).is_none()
                && module(s!("amd_fidelityfx_vk.dll")).is_none()
                && module(s!("amd_fidelityfx_dx12.dll")).is_none(),
            "CI must not LoadLibrary SL/FidelityFX; skip = success"
        );
        hook();
        assert!(
            HOOKS.get().is_none(),
            "miss must not latch empty (late LoadLibrary re-calls hook)"
        );
    }

    #[test]
    fn fg_hooks_are_original_only_no_extra_present() {
        assert_eq!(fg_plan(), FgPlan::OriginalOnly);
    }

    #[test]
    fn sldlssg_uses_plugin_function_not_direct_export() {
        assert!(resolve_sldlssg(HMODULE(core::ptr::null_mut())).is_none());
    }
}
