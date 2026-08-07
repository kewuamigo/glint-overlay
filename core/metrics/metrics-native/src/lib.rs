//! Native Present hook counter exposed via shared memory.

pub mod eos_hook;
pub mod fg_detect;
pub mod hook;
pub mod sl_hook;

pub use glint_metrics_common::{
    counter, now_ms, read_metrics_for_pid, MetricsBlock, SharedMetrics,
};

use std::sync::atomic::Ordering;
use std::time::Duration;

use glint_metrics_common::counter::NATIVE_FRAME_COUNT;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;

/// # Safety
/// Standard DllMain contract: called by the Windows loader under loader lock.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: windows::Win32::Foundation::HMODULE,
    reason: u32,
    _reserved: *mut std::ffi::c_void,
) -> i32 {
    match reason {
        DLL_PROCESS_ATTACH => {
            let _ = module;
            SharedMetrics::create_or_open();
            start_worker_thread();
            1
        }
        DLL_PROCESS_DETACH => {
            hook::uninstall_hooks();
            SharedMetrics::close();
            1
        }
        _ => 1,
    }
}

/// All real work happens off DllMain: `install_hooks` creates a transient
/// D3D11 device (loads d3d11.dll/dxgi.dll), which deadlocks under loader lock.
/// The same thread keeps re-scanning for FG SDK modules, because they load
/// lazily (often only when the user toggles frame generation in-game).
fn start_worker_thread() {
    std::thread::spawn(|| {
        // Let the loader finish before touching D3D.
        std::thread::sleep(Duration::from_millis(250));
        let _ = hook::install_hooks();

        loop {
            if let Some(shm) = SharedMetrics::instance() {
                shm.set_fg_kind(fg_detect::detect_fg_kind());
            }
            // Streamline loads lazily; hook its frame token as soon as it appears
            // to measure the true game-engine rate under DLSS-FG.
            sl_hook::try_install();
            // EOSSDK may load after the game boots — retry until hooked.
            eos_hook::try_install();
            std::thread::sleep(Duration::from_millis(1_000));
        }
    });
}

/// Called from hooked Present / Present1 — exported for testing.
#[unsafe(no_mangle)]
pub extern "C" fn glint_on_native_present() {
    NATIVE_FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    if let Some(shm) = SharedMetrics::instance() {
        shm.tick_native();
    }
}
