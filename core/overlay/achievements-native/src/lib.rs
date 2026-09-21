//! In-process achievement unlock hooks (EOS + Uplay). Not metrics.

pub mod eos_hook;
pub mod upc_hook;

use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;

use minhook_sys::{MH_Initialize, MH_OK};

const DLL_PROCESS_ATTACH: u32 = 1;

pub fn debug_log(msg: &str) {
    let path = std::env::var("APPDATA")
        .map(|a| std::path::PathBuf::from(a).join("Glint").join("achievements-hook.log"))
        .unwrap_or_else(|_| std::path::PathBuf::from("achievements-hook.log"));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _module: windows::Win32::Foundation::HMODULE,
    reason: u32,
    _reserved: *mut std::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        debug_log("DllMain attach");
        start_worker_thread();
    }
    1
}

fn start_worker_thread() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(250));
        unsafe {
            let init = MH_Initialize();
            if init != MH_OK && init != 1 {
                debug_log(&format!("MH_Initialize failed {init}"));
                return;
            }
        }
        debug_log("worker start");
        loop {
            eos_hook::try_install();
            upc_hook::try_install();
            std::thread::sleep(Duration::from_millis(1_000));
        }
    });
}
