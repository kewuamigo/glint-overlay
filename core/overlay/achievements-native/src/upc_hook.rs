use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use minhook_sys::{MH_CreateHook, MH_EnableHook, MH_OK};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleW, GetProcAddress,
};
use windows::core::{PCSTR, PCWSTR};

type UnlockFn = unsafe extern "system" fn(
    context: *mut c_void,
    in_id: u32,
    callback: *mut c_void,
    user_data: usize,
) -> i32;

static INSTALLED: AtomicBool = AtomicBool::new(false);
static mut ORIGINAL: Option<UnlockFn> = None;

const MODULES: &[&str] = &["upc_r2_loader64.dll", "upc_r2_loader.dll"];
const UNLOCK_EXPORT: &[u8] = b"UPC_AchievementUnlock\0";
const BIN_DIRS: &[&str] = &["bin", "bin64", "binaries", "win64"];

pub fn try_install() {
    if INSTALLED.load(Ordering::Acquire) {
        return;
    }

    for module_name in MODULES {
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
                crate::debug_log(&format!("upc hook installed on {module_name}"));
            } else {
                crate::debug_log(&format!("upc hook create/enable failed on {module_name}"));
            }
        }
        return;
    }
}

unsafe extern "system" fn unlock_detour(
    context: *mut c_void,
    in_id: u32,
    callback: *mut c_void,
    user_data: usize,
) -> i32 {
    bridge_unlock(in_id);
    let original = unsafe { *std::ptr::addr_of!(ORIGINAL) };
    match original {
        Some(original) => unsafe { original(context, in_id, callback, user_data) },
        None => 0,
    }
}

fn bridge_unlock(in_id: u32) {
    crate::debug_log(&format!("UPC_AchievementUnlock inId={in_id}"));
    let Some(exe) = current_exe_path() else {
        crate::debug_log("no exe path");
        return;
    };
    let Some(app_id) = resolve_steam_app_id(&exe) else {
        crate::debug_log(&format!("no steam appid next to {}", exe.display()));
        return;
    };
    let names = read_schema_names(&schema_dir(&exe));
    let Some(steam_name) = map_upc_in_id(&in_id.to_string(), &names) else {
        crate::debug_log(&format!("unmapped inId={in_id} schema_len={}", names.len()));
        return;
    };
    crate::debug_log(&format!("mapped {in_id} -> {steam_name} appid={app_id}"));
    let dest = gse_unlock_path(&app_id);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    merge_gse_unlock(&dest, &steam_name, now);
}

pub fn map_upc_in_id(in_id: &str, names: &[String]) -> Option<String> {
    let raw = in_id.trim();
    if raw.is_empty() || names.is_empty() {
        return None;
    }
    if names.iter().any(|n| n == raw) {
        return Some(raw.to_string());
    }
    let n: usize = raw.parse().ok()?;
    if n >= 1 && n <= names.len() {
        return Some(names[n - 1].clone());
    }
    if n < names.len() {
        return Some(names[n].clone());
    }
    None
}

fn read_schema_names(game_dir: &Path) -> Vec<String> {
    let file = game_dir.join("steam_settings").join("achievements.json");
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|row| row.get("name")?.as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect()
}

fn merge_gse_unlock(file: &Path, steam_name: &str, earned_time: i64) {
    let mut map = match fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    {
        Some(serde_json::Value::Object(o)) => o,
        _ => serde_json::Map::new(),
    };
    map.insert(
        steam_name.to_string(),
        serde_json::json!({ "earned": true, "earned_time": earned_time }),
    );
    if let Some(parent) = file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        file,
        format!("{}\n", serde_json::Value::Object(map)),
    );
}

fn gse_unlock_path(app_id: &str) -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(appdata)
        .join("GSE Saves")
        .join(app_id)
        .join("achievements.json")
}

fn schema_dir(exe: &Path) -> PathBuf {
    for dir in scan_dirs(exe) {
        if dir.join("steam_settings").join("achievements.json").is_file() {
            return dir;
        }
    }
    exe.parent().unwrap_or(exe).to_path_buf()
}

fn resolve_steam_app_id(exe: &Path) -> Option<String> {
    for dir in scan_dirs(exe) {
        for rel in [
            "steam_appid.txt",
            "steam_settings/steam_appid.txt",
        ] {
            if let Some(id) = read_appid_file(&dir.join(rel)) {
                return Some(id);
            }
        }
    }
    None
}

fn read_appid_file(path: &Path) -> Option<String> {
    let t = fs::read_to_string(path).ok()?.trim().to_string();
    if t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty() {
        Some(t)
    } else {
        None
    }
}

fn scan_dirs(exe: &Path) -> Vec<PathBuf> {
    let dir = exe.parent().unwrap_or(exe).to_path_buf();
    let mut dirs = vec![dir.clone()];
    if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        if BIN_DIRS.iter().any(|b| b.eq_ignore_ascii_case(name)) {
            if let Some(parent) = dir.parent() {
                dirs.push(parent.to_path_buf());
            }
        }
    }
    dirs
}

fn current_exe_path() -> Option<PathBuf> {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(None, &mut buf) };
    if n == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
}

#[cfg(test)]
mod tests {
    use super::map_upc_in_id;

    #[test]
    fn exact_name() {
        let names = vec!["ACObsidian_Ach_1".into(), "ACObsidian_Ach_2".into()];
        assert_eq!(
            map_upc_in_id("ACObsidian_Ach_2", &names).as_deref(),
            Some("ACObsidian_Ach_2")
        );
    }

    #[test]
    fn numeric_index() {
        let names = vec!["A".into(), "B".into(), "C".into()];
        assert_eq!(map_upc_in_id("2", &names).as_deref(), Some("B"));
        assert_eq!(map_upc_in_id("0", &names).as_deref(), Some("A"));
        assert_eq!(map_upc_in_id("99", &names), None);
    }
}
