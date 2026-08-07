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
use glint_overlay_core::initialize;
use std::thread;
use tokio::runtime::Runtime;
use tracing::{debug, error};
use windows::Win32::{
    Foundation::HINSTANCE,
    System::{
        SystemServices::DLL_PROCESS_ATTACH,
        Threading::GetCurrentProcessId,
    },
};

use crate::server::{PipeRole, bind_named_pipe, listen_loop};

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
        initialize(module_handle as _).expect("initialization failed");
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
