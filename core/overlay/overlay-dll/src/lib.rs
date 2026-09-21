//! Glint GPU compositor DLL — inject into target process for IPC-controlled overlay.

#![windows_subsystem = "windows"]

//! Official DLL crate for attaching [`glint_overlay_core`] to other processes.
//! Using this DLL, the overlay can be controlled via cross-process IPC.
//!
//! Injection can be done using `glint-overlay-client` crate.

#[cfg(debug_assertions)]
mod dbg;

mod clients;
mod server;

extern crate glint_overlay_vulkan_layer;

use glint_overlay_common::ipc::{create_ipc_addr, create_ipc_addr_cef};
use glint_overlay_core::{
    b_overlay_needs_present, create_process_w_original, ingest_captured_frame, initialize,
    is_overlay_enabled, overlay_is_using_input, set_child_inject,
};
use std::{
    ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, sync::OnceLock, thread,
    time::Duration,
};
use tokio::runtime::Runtime;
use tracing::{debug, error};
use windows::Win32::{
    Foundation::{HANDLE, HINSTANCE, HMODULE},
    System::{
        LibraryLoader::GetModuleFileNameW, SystemServices::DLL_PROCESS_ATTACH,
        Threading::GetCurrentProcessId,
    },
};

use crate::server::{PipeRole, bind_named_pipe, listen_loop};

static OVERLAY_DLL_PATH: OnceLock<PathBuf> = OnceLock::new();

fn register_child_inject(hinstance: usize) {
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(Some(HMODULE(hinstance as _)), &mut buf) };
    if len == 0 || len as usize >= buf.len() {
        error!("child inject: cannot resolve overlay DLL path");
        return;
    }
    let path = PathBuf::from(OsString::from_wide(&buf[..len as usize]));
    let _ = OVERLAY_DLL_PATH.set(path);
    set_child_inject(inject_into_child);
}

fn inject_into_child(pid: u32, process: HANDLE, thread: HANDLE) {
    let Some(path) = OVERLAY_DLL_PATH.get() else {
        return;
    };
    let timeout = Some(Duration::from_secs(30));
    let result = if !process.is_invalid() && !thread.is_invalid() {
        glint_overlay_client::inject_dll_stopped(process, thread, path, timeout)
    } else {
        glint_overlay_client::inject_dll(pid, path, timeout).map(|_| ())
    };
    if let Err(err) = result {
        error!(pid, "child overlay inject failed: {err:?}");
    }
}

/// Steam ordinal 11 analog (`0x1800C69E0`). a4 == 0 → no draw.
/// Non-null a4 → blit the acquired (not-yet-presented) mailbox. Caller presents.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn VulkanSteamOverlayPresent(
    _a1: *mut core::ffi::c_void,
    _width: i32,
    _height: i32,
    draw_fn: *mut core::ffi::c_void,
    _p5: *mut core::ffi::c_void,
    _p6: *mut core::ffi::c_void,
    _p7: *mut core::ffi::c_void,
    _p8: *mut core::ffi::c_void,
    _p9: *mut core::ffi::c_void,
    _p10: *mut core::ffi::c_void,
) {
    glint_overlay_vulkan_layer::device::queue::steam_overlay_present(!draw_fn.is_null());
}

/// Steam `VulkanSteamOverlayProcessCapturedFrame` @ `0x1800C6AC0`.
/// Game-frame ingest (RGB / NV12 / shared) into one last-frame slot.
/// MUST NOT blit overlay. MUST NOT Present / `vkQueuePresentKHR`.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn VulkanSteamOverlayProcessCapturedFrame(
    _a1: i8,
    a2: i32,
    _a3: i32,
    _a4: i32,
    _a5: i64,
    a6: i64,
    a7: u32,
    a8: i32,
    a9: i32,
    _a10: i32,
    _a11: i32,
    _a12: i32,
    _a13: i32,
) -> i64 {
    ingest_captured_frame(a2, a6, a7, a8, a9)
}

/// Steam `VulkanSteamOverlaySetWindowType` @ `0x18003DED0`. Empty (`ret` only).
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn VulkanSteamOverlaySetWindowType() {}

/// Steam `BOverlayNeedsPresent` @ `0x1800AE420`. Hint only; hook does not Present.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn BOverlayNeedsPresent() -> i32 {
    i32::from(b_overlay_needs_present())
}

/// Steam `IsOverlayEnabled` @ `0x1800B07F0`. True once this DLL is attached.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn IsOverlayEnabled() -> i32 {
    i32::from(is_overlay_enabled())
}

/// Steam `SteamOverlayIsUsingGamepad` @ `0x1800B0830`. True while Interactive.
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn SteamOverlayIsUsingGamepad() -> i32 {
    i32::from(overlay_is_using_input())
}

/// Steam `SteamOverlayIsUsingKeyboard` @ `0x1800B0850` (shared impl with Mouse).
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn SteamOverlayIsUsingKeyboard() -> i32 {
    i32::from(overlay_is_using_input())
}

/// Steam `SteamOverlayIsUsingMouse` @ `0x1800B0850` (shared impl with Keyboard).
#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn SteamOverlayIsUsingMouse() -> i32 {
    i32::from(overlay_is_using_input())
}

/// Main entry point for DLL.
///
/// # Safety
/// Can be called by loader only. Must not be called manually.
#[unsafe(no_mangle)]
#[allow(non_snake_case, unused_variables)]
pub unsafe extern "system" fn DllMain(dll_module: HINSTANCE, fdw_reason: u32, _: *mut ()) -> bool {
    #[cfg(debug_assertions)]
    fn setup_tracing() {
        use tracing::level_filters::LevelFilter;

        use crate::dbg::WinDbgMakeWriter;

        tracing_subscriber::fmt::fmt()
            .with_ansi(false)
            .with_thread_ids(true)
            .with_max_level(LevelFilter::TRACE)
            .with_writer(WinDbgMakeWriter::new())
            .init();
    }

    if fdw_reason != DLL_PROCESS_ATTACH {
        return true;
    }

    #[cfg(debug_assertions)]
    setup_tracing();

    let Ok(rt) = Runtime::new() else {
        error!("cannot create tokio runtime");
        return false;
    };
    let _guard = rt.enter();

    let pid = unsafe { GetCurrentProcessId() };
    let module_handle = dll_module.0 as usize;
    // Two pipes: Electron shell (layer 0) and CEF browser-host (layer ≥1). No shared
    // connection → no request crosstalk between the book layers.
    let addr_shell = create_ipc_addr(pid, module_handle as u32);
    let addr_cef = create_ipc_addr_cef(pid, module_handle as u32);

    let server_shell = match bind_named_pipe(&addr_shell, true) {
        Ok(server) => server,
        Err(err) => {
            error!("cannot open shell ipc server. err: {err:?}");
            return false;
        }
    };
    let server_cef = match bind_named_pipe(&addr_cef, true) {
        Ok(server) => server,
        Err(err) => {
            error!("cannot open cef ipc server. err: {err:?}");
            return false;
        }
    };

    let create_shell = {
        let addr = addr_shell.clone();
        move || bind_named_pipe(&addr, false)
    };
    let create_cef = {
        let addr = addr_cef.clone();
        move || bind_named_pipe(&addr, false)
    };

    thread::spawn(move || {
        register_child_inject(module_handle);
        initialize(module_handle as _).expect("initialization failed");
        glint_overlay_client::set_create_process_w_original(
            create_process_w_original().map(|orig| unsafe { std::mem::transmute(orig) }),
        );
        debug!(%addr_shell, %addr_cef, "hook installed; dual pipes listening");

        rt.block_on(async move {
            tokio::join!(
                listen_loop(server_shell, create_shell, PipeRole::Shell),
                listen_loop(server_cef, create_cef, PipeRole::Cef),
            );
        });
    });
    true
}
