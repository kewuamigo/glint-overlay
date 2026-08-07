//! Hook `EOS_Achievements_UnlockAchievements` when EOSSDK is loaded (CODEX / Nemirtingas / stock).
//! Unlocks are appended as JSON lines for the Electron achievements watcher.

use std::ffi::c_void;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use minhook_sys::{MH_CreateHook, MH_EnableHook, MH_OK};
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

/// EOS_Achievements_UnlockAchievementsOptions (x64, ApiVersion 1).
#[repr(C)]
struct UnlockOptions {
    api_version: i32,
    _pad: u32,
    user_id: *mut c_void,
    achievements_count: u32,
    _pad2: u32,
    achievement_ids: *const *const i8,
}

type UnlockFn = unsafe extern "system" fn(
    handle: *mut c_void,
    options: *const UnlockOptions,
    client_data: *mut c_void,
    completion: *mut c_void,
);

static INSTALLED: AtomicBool = AtomicBool::new(false);
static mut ORIGINAL: Option<UnlockFn> = None;

const EOS_MODULES: &[&str] = &["EOSSDK-Win64-Shipping.dll", "EOSSDK-Win32-Shipping.dll"];
const UNLOCK_EXPORT: &[u8] = b"EOS_Achievements_UnlockAchievements\0";

pub fn try_install() {
    if INSTALLED.load(Ordering::Acquire) {
        return;
    }

    for module_name in EOS_MODULES {
        let wide: Vec<u16> = module_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let Ok(module) = (unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) }) else {
            continue;
        };
        let Some(proc) = (unsafe { GetProcAddress(module, PCSTR(UNLOCK_EXPORT.as_ptr())) }) else {
            continue;
        };

        let target = proc as *mut c_void;
        let mut original: *mut c_void = std::ptr::null_mut();
        unsafe {
            if MH_CreateHook(target, unlock_detour as *mut c_void, &mut original) == MH_OK
                && MH_EnableHook(target) == MH_OK
            {
                ORIGINAL = Some(std::mem::transmute::<*mut c_void, UnlockFn>(original));
                INSTALLED.store(true, Ordering::Release);
                write_hook_event(
                    "installed",
                    &current_exe_name(),
                    &[],
                );
            }
        }
        return;
    }
}

unsafe extern "system" fn unlock_detour(
    handle: *mut c_void,
    options: *const UnlockOptions,
    client_data: *mut c_void,
    completion: *mut c_void,
) {
    if !options.is_null() {
        let opts = unsafe { &*options };
        append_unlock_line(opts);
    }
    let original = unsafe { *std::ptr::addr_of!(ORIGINAL) };
    if let Some(original) = original {
        unsafe { original(handle, options, client_data, completion) };
    }
}

fn append_unlock_line(opts: &UnlockOptions) {
    let count = opts.achievements_count as usize;
    if count == 0 || opts.achievement_ids.is_null() {
        write_hook_event("unlock_empty", &current_exe_name(), &[]);
        return;
    }

    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let ptr = unsafe { *opts.achievement_ids.add(i) };
        if ptr.is_null() {
            continue;
        }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) };
        if let Ok(id) = s.to_str() {
            if !id.is_empty() {
                ids.push(id.to_string());
            }
        }
    }
    write_hook_event("unlock", &current_exe_name(), &ids);
}

fn write_hook_event(kind: &str, exe: &str, ids: &[String]) {
    let line = format!(
        "{}\n",
        serde_json::json!({
            "kind": kind,
            "pid": std::process::id(),
            "exe": exe,
            "ids": ids,
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        })
    );

    let path = unlock_log_path();
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

fn unlock_log_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(appdata)
        .join("Glint")
        .join("apps")
        .join("achievements")
        .join("eos-hooks.jsonl")
}

fn current_exe_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}
