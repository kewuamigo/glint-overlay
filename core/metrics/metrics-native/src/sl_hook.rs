//! Streamline / FidelityFX hooks — game FPS + Steam FG kind dword.
//!
//! `slGetNewFrameToken` counts simulated frames. `slDLSSGSetOptions` /
//! `ffxConfigure*` write GOR64 `dword_180197870` (0 none, 1 DLSS, 2 FSR).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use glint_metrics_common::{FG_KIND_DLSS, FG_KIND_FSR, FG_KIND_NONE, SharedMetrics};
use minhook_sys::{MH_CreateHook, MH_EnableHook, MH_OK};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{PCSTR, PCWSTR};

type SlGetNewFrameTokenFn = unsafe extern "system" fn(*mut c_void, *const u32) -> u32;
type SlPluginFn = unsafe extern "system" fn(*const u8) -> *mut c_void;
type SlDlssgFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
type FfxFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;

static TOKEN_INSTALLED: AtomicBool = AtomicBool::new(false);
static DLSSG_INSTALLED: AtomicBool = AtomicBool::new(false);
static DLSSG_PD_INSTALLED: AtomicBool = AtomicBool::new(false);
static FFX_VK_INSTALLED: AtomicBool = AtomicBool::new(false);
static FFX_DX12_INSTALLED: AtomicBool = AtomicBool::new(false);
static LAST_FFX_FRAME_ID: AtomicU64 = AtomicU64::new(0);
static FIRST_FFX_FRAME: AtomicBool = AtomicBool::new(true);

static mut ORIGINAL_TOKEN: Option<SlGetNewFrameTokenFn> = None;
static mut ORIGINAL_DLSSG: Option<SlDlssgFn> = None;
static mut ORIGINAL_DLSSG_PD: Option<SlDlssgFn> = None;
static mut ORIGINAL_FFX_VK: Option<FfxFn> = None;
static mut ORIGINAL_FFX_DX12: Option<FfxFn> = None;

const SL_EXPORT: &[u8] = b"slGetNewFrameToken\0";
const SL_PLUGIN: &[u8] = b"slGetPluginFunction\0";
const SL_DLSSG: &[u8] = b"slDLSSGSetOptions\0";
const FFX_EXPORT: &[u8] = b"ffxConfigure\0";

/// GOR64 `sub_1800C21A0`: `*(a2+32) != 0`.
pub fn dlssg_options_on(a2: *const u8) -> Option<bool> {
    if a2.is_null() {
        return None;
    }
    Some(unsafe { a2.add(32).cast::<u32>().read_unaligned() != 0 })
}

/// GOR64 `sub_1800C20A0`: `*a2 == 0x20002` (qword @ 0x1800C20A6) && `*(a2+56) != 0` (byte @ 0x1800C20B6).
pub fn ffx_configure_fg(a2: *const u8) -> Option<bool> {
    if a2.is_null() {
        return None;
    }
    let header = unsafe { a2.cast::<u64>().read_unaligned() };
    if header != 0x20002 {
        return None;
    }
    Some(unsafe { *a2.add(56) != 0 })
}

/// GOR64 `sub_1800C20A0` @ 0x1800C20D3: `byte_180160F28 || *(a2+136) > last` then `sub_1800C1620(ctx, 1)`.
pub fn ffx_new_game_frame(a2: *const u8, last_frame_id: u64, first: bool) -> Option<u64> {
    if a2.is_null() {
        return None;
    }
    let frame_id = unsafe { a2.add(136).cast::<u64>().read_unaligned() };
    if first || frame_id > last_frame_id {
        Some(frame_id)
    } else {
        None
    }
}

fn apply_dlssg(a2: *const u8) {
    let Some(on) = dlssg_options_on(a2) else {
        return;
    };
    if let Some(shm) = SharedMetrics::instance() {
        shm.set_fg_active(if on { FG_KIND_DLSS } else { FG_KIND_NONE });
    }
}

fn apply_ffx(a2: *const u8) {
    let Some(on) = ffx_configure_fg(a2) else {
        return;
    };
    if let Some(shm) = SharedMetrics::instance() {
        shm.set_fg_active(if on { FG_KIND_FSR } else { FG_KIND_NONE });
        if on {
            let last = LAST_FFX_FRAME_ID.load(Ordering::Relaxed);
            let first = FIRST_FFX_FRAME.load(Ordering::Relaxed);
            if let Some(frame_id) = ffx_new_game_frame(a2, last, first) {
                FIRST_FFX_FRAME.store(false, Ordering::Relaxed);
                LAST_FFX_FRAME_ID.store(frame_id, Ordering::Relaxed);
                shm.tick_game_frame();
            }
        }
    }
}

fn module(name: &str) -> Option<windows::Win32::Foundation::HMODULE> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) }.ok()
}

fn resolve_sldlssg(module_name: &str) -> Option<*mut c_void> {
    let h = module(module_name)?;
    let proc = unsafe { GetProcAddress(h, PCSTR(SL_PLUGIN.as_ptr())) }?;
    let get_plugin: SlPluginFn = unsafe { std::mem::transmute(proc) };
    let addr = unsafe { get_plugin(SL_DLSSG.as_ptr()) };
    if addr.is_null() { None } else { Some(addr) }
}

unsafe fn attach<F>(target: *mut c_void, detour: *mut c_void, slot: &mut Option<F>) -> bool {
    let mut original = std::ptr::null_mut();
    if unsafe {
        MH_CreateHook(target, detour, &mut original) == MH_OK && MH_EnableHook(target) == MH_OK
    } {
        *slot = Some(unsafe { std::mem::transmute_copy(&original) });
        true
    } else {
        false
    }
}

pub fn try_install() {
    if !TOKEN_INSTALLED.load(Ordering::Acquire) {
        for name in ["sl.interposer.dll", "sl_interposer.dll"] {
            let Some(h) = module(name) else { continue };
            let Some(proc) = (unsafe { GetProcAddress(h, PCSTR(SL_EXPORT.as_ptr())) }) else {
                continue;
            };
            unsafe {
                if attach(
                    proc as *mut c_void,
                    frame_token_detour as *mut c_void,
                    &mut *std::ptr::addr_of_mut!(ORIGINAL_TOKEN),
                ) {
                    TOKEN_INSTALLED.store(true, Ordering::Release);
                }
            }
            break;
        }
    }

    if !DLSSG_INSTALLED.load(Ordering::Acquire) {
        if let Some(addr) = resolve_sldlssg("sl.dlss_g.dll") {
            unsafe {
                if attach(
                    addr,
                    dlssg_detour as *mut c_void,
                    &mut *std::ptr::addr_of_mut!(ORIGINAL_DLSSG),
                ) {
                    DLSSG_INSTALLED.store(true, Ordering::Release);
                }
            }
        }
    }

    if !DLSSG_PD_INSTALLED.load(Ordering::Acquire) {
        if let Some(addr) = resolve_sldlssg("sl_dlss_g.dll") {
            if module("sl.dlss_g.dll").is_none()
                || module("sl_dlss_g.dll") != module("sl.dlss_g.dll")
            {
                unsafe {
                    if attach(
                        addr,
                        dlssg_pd_detour as *mut c_void,
                        &mut *std::ptr::addr_of_mut!(ORIGINAL_DLSSG_PD),
                    ) {
                        DLSSG_PD_INSTALLED.store(true, Ordering::Release);
                    }
                }
            }
        }
    }

    if !FFX_VK_INSTALLED.load(Ordering::Acquire) {
        if let Some(h) = module("amd_fidelityfx_vk.dll") {
            if let Some(proc) = unsafe { GetProcAddress(h, PCSTR(FFX_EXPORT.as_ptr())) } {
                unsafe {
                    if attach(
                        proc as *mut c_void,
                        ffx_vk_detour as *mut c_void,
                        &mut *std::ptr::addr_of_mut!(ORIGINAL_FFX_VK),
                    ) {
                        FFX_VK_INSTALLED.store(true, Ordering::Release);
                    }
                }
            }
        }
    }

    if !FFX_DX12_INSTALLED.load(Ordering::Acquire) {
        if let Some(h) = module("amd_fidelityfx_dx12.dll") {
            if let Some(proc) = unsafe { GetProcAddress(h, PCSTR(FFX_EXPORT.as_ptr())) } {
                unsafe {
                    if attach(
                        proc as *mut c_void,
                        ffx_dx12_detour as *mut c_void,
                        &mut *std::ptr::addr_of_mut!(ORIGINAL_FFX_DX12),
                    ) {
                        FFX_DX12_INSTALLED.store(true, Ordering::Release);
                    }
                }
            }
        }
    }
}

unsafe extern "system" fn frame_token_detour(token: *mut c_void, frame_index: *const u32) -> u32 {
    if let Some(shm) = SharedMetrics::instance() {
        shm.tick_game_frame();
    }
    match unsafe { *std::ptr::addr_of!(ORIGINAL_TOKEN) } {
        Some(original) => unsafe { original(token, frame_index) },
        None => 0,
    }
}

unsafe extern "system" fn dlssg_detour(a1: *mut c_void, a2: *mut c_void) -> i32 {
    apply_dlssg(a2 as *const u8);
    match unsafe { *std::ptr::addr_of!(ORIGINAL_DLSSG) } {
        Some(original) => unsafe { original(a1, a2) },
        None => 0,
    }
}

unsafe extern "system" fn dlssg_pd_detour(a1: *mut c_void, a2: *mut c_void) -> i32 {
    apply_dlssg(a2 as *const u8);
    match unsafe { *std::ptr::addr_of!(ORIGINAL_DLSSG_PD) } {
        Some(original) => unsafe { original(a1, a2) },
        None => 0,
    }
}

unsafe extern "system" fn ffx_vk_detour(a1: *mut c_void, a2: *mut c_void) -> i32 {
    apply_ffx(a2 as *const u8);
    match unsafe { *std::ptr::addr_of!(ORIGINAL_FFX_VK) } {
        Some(original) => unsafe { original(a1, a2) },
        None => 0,
    }
}

unsafe extern "system" fn ffx_dx12_detour(a1: *mut c_void, a2: *mut c_void) -> i32 {
    apply_ffx(a2 as *const u8);
    match unsafe { *std::ptr::addr_of!(ORIGINAL_FFX_DX12) } {
        Some(original) => unsafe { original(a1, a2) },
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dlssg_on_when_dword_at_32_nonzero() {
        let mut buf = [0u8; 64];
        buf[32..36].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(dlssg_options_on(buf.as_ptr()), Some(true));
        buf[32..36].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(dlssg_options_on(buf.as_ptr()), Some(false));
        assert_eq!(dlssg_options_on(std::ptr::null()), None);
    }

    #[test]
    fn ffx_on_when_qword_20002_and_byte_at_56_nonzero() {
        let mut buf = [0u8; 144];
        buf[0..8].copy_from_slice(&0x20002u64.to_le_bytes());
        buf[56] = 1;
        assert_eq!(ffx_configure_fg(buf.as_ptr()), Some(true));
        buf[56] = 0;
        assert_eq!(ffx_configure_fg(buf.as_ptr()), Some(false));
        buf[0..8].copy_from_slice(&0x1_0002_0002u64.to_le_bytes());
        buf[56] = 1;
        assert_eq!(ffx_configure_fg(buf.as_ptr()), None);
        assert_eq!(ffx_configure_fg(std::ptr::null()), None);
    }

    #[test]
    fn ffx_ticks_only_on_newer_frame_id() {
        let mut buf = [0u8; 144];
        assert_eq!(ffx_new_game_frame(buf.as_ptr(), 0, true), Some(0));
        assert_eq!(ffx_new_game_frame(buf.as_ptr(), 0, false), None);
        buf[136..144].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(ffx_new_game_frame(buf.as_ptr(), 0, false), Some(1));
        assert_eq!(ffx_new_game_frame(buf.as_ptr(), 1, false), None);
        buf[136..144].copy_from_slice(&2u64.to_le_bytes());
        assert_eq!(ffx_new_game_frame(buf.as_ptr(), 1, false), Some(2));
        assert_eq!(ffx_new_game_frame(std::ptr::null(), 0, true), None);
    }
}
