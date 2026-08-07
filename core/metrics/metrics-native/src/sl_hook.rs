//! Streamline `slGetNewFrameToken` hook — true game-engine frame rate under DLSS-FG.
//!
//! With DLSS Frame Generation, Streamline presents both real and interpolated
//! frames on the real swapchain, so any DXGI `Present` hook measures the
//! *display* rate. The game itself calls `slGetNewFrameToken` exactly once per
//! simulated frame; counting those calls gives the true native rate — the same
//! game-side signal the Steam overlay uses for its FPS split.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use minhook_sys::{MH_CreateHook, MH_EnableHook, MH_OK};
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use glint_metrics_common::SharedMetrics;

/// `sl::Result slGetNewFrameToken(sl::FrameToken*&, const uint32_t*)`
type SlGetNewFrameTokenFn = unsafe extern "system" fn(*mut c_void, *const u32) -> u32;

static INSTALLED: AtomicBool = AtomicBool::new(false);
static mut ORIGINAL: Option<SlGetNewFrameTokenFn> = None;

const SL_MODULES: &[&str] = &["sl.interposer.dll"];
const SL_EXPORT: &[u8] = b"slGetNewFrameToken\0";

/// Try to install the Streamline frame-token hook. Safe to call repeatedly —
/// installs once, when `sl.interposer.dll` shows up in the process.
pub fn try_install() {
    if INSTALLED.load(Ordering::Acquire) {
        return;
    }

    for module_name in SL_MODULES {
        let wide: Vec<u16> = module_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let Ok(module) = (unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) }) else {
            continue;
        };
        let Some(proc) = (unsafe { GetProcAddress(module, PCSTR(SL_EXPORT.as_ptr())) }) else {
            continue;
        };

        let target = proc as *mut c_void;
        let mut original: *mut c_void = std::ptr::null_mut();
        unsafe {
            if MH_CreateHook(target, frame_token_detour as *mut c_void, &mut original) == MH_OK
                && MH_EnableHook(target) == MH_OK
            {
                ORIGINAL = Some(std::mem::transmute::<*mut c_void, SlGetNewFrameTokenFn>(
                    original,
                ));
                INSTALLED.store(true, Ordering::Release);
            }
        }
        return;
    }
}

unsafe extern "system" fn frame_token_detour(token: *mut c_void, frame_index: *const u32) -> u32 {
    if let Some(shm) = SharedMetrics::instance() {
        shm.tick_game_frame();
    }
    // ORIGINAL is written once before INSTALLED is set; only read afterwards.
    let original = unsafe { *std::ptr::addr_of!(ORIGINAL) };
    match original {
        Some(original) => unsafe { original(token, frame_index) },
        None => 0,
    }
}
