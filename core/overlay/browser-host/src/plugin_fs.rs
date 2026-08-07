//! Plugin filesystem sandbox — Electron `plugin-fs.ts` / pick / `shell.openPath` parity.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem,
    SIGDN_FILESYSPATH, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HRESULT, PCWSTR, PWSTR, w};

use crate::apps;
use crate::plugin_storage;

/// Electron `assertPathAllowed`.
pub fn assert_path_allowed(target_path: &str, allowed: &HashSet<PathBuf>) -> Result<(), String> {
    let resolved = absolute_norm(Path::new(target_path))?;
    for prefix in allowed {
        let normalized = absolute_norm(prefix)?;
        if resolved == normalized || resolved.starts_with(&normalized) {
            return Ok(());
        }
    }
    Err(format!("path not allowed: {target_path}"))
}

/// Electron `buildAllowedPaths` including save-service game/save roots (F007).
pub fn build_allowed_paths(plugin_id: &str, access: &apps::AppAccess) -> Result<HashSet<PathBuf>, String> {
    let plugin_dir = apps::apps_root().join(plugin_id);
    let mut allowed = HashSet::new();
    allowed.insert(absolute_norm(&plugin_dir)?);
    allowed.insert(absolute_norm(&plugin_dir.join("data"))?);

    for root in crate::plugin_saves::allowlist_roots(plugin_id, access) {
        if let Ok(p) = absolute_norm(&root) {
            allowed.insert(p);
        }
    }

    if access.permissions.iter().any(|p| p == "fs:pick") {
        let granted = plugin_storage::get(&plugin_dir, "fs:grantedPaths")?;
        if let Value::Array(entries) = granted {
            for entry in entries {
                if let Some(s) = entry.as_str() {
                    if !s.is_empty() {
                        if let Ok(p) = absolute_norm(Path::new(s)) {
                            allowed.insert(p);
                        }
                    }
                }
            }
        }
    }

    Ok(allowed)
}

fn absolute_norm(path: &Path) -> Result<PathBuf, String> {
    let abs = std::path::absolute(path).map_err(|e| format!("path resolve failed: {e}"))?;
    Ok(normalize_dots(&abs))
}

fn normalize_dots(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                let _ = out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn assert_allowed(plugin_id: &str, access: &apps::AppAccess, target: &str) -> Result<(), String> {
    let allowed = build_allowed_paths(plugin_id, access)?;
    assert_path_allowed(target, &allowed)
}

pub fn read_text(plugin_id: &str, access: &apps::AppAccess, target: &str) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    let text = std::fs::read_to_string(target).map_err(|e| format!("fs.readText failed: {e}"))?;
    Ok(json!(text).to_string())
}

/// Wire: JSON `number[]` (PluginContext converts to Uint8Array).
pub fn read_bytes(plugin_id: &str, access: &apps::AppAccess, target: &str) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    let bytes = std::fs::read(target).map_err(|e| format!("fs.readBytes failed: {e}"))?;
    Ok(Value::Array(bytes.into_iter().map(|b| json!(b)).collect()).to_string())
}

pub fn write_text(
    plugin_id: &str,
    access: &apps::AppAccess,
    target: &str,
    content: &str,
) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    if let Some(parent) = Path::new(target).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("fs.writeText mkdir failed: {e}"))?;
    }
    std::fs::write(target, content).map_err(|e| format!("fs.writeText failed: {e}"))?;
    Ok(String::new())
}

pub fn write_bytes(
    plugin_id: &str,
    access: &apps::AppAccess,
    target: &str,
    data: &Value,
) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    let bytes = parse_bytes(data);
    if let Some(parent) = Path::new(target).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("fs.writeBytes mkdir failed: {e}"))?;
    }
    std::fs::write(target, bytes).map_err(|e| format!("fs.writeBytes failed: {e}"))?;
    Ok(String::new())
}

/// Accept JSON `number[]` or `JSON.stringify(Uint8Array)` object form `{"0":n,…}`.
fn parse_bytes(data: &Value) -> Vec<u8> {
    match data {
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u8))
            .collect(),
        Value::Object(map) => {
            let mut pairs: Vec<(usize, u8)> = map
                .iter()
                .filter_map(|(k, v)| {
                    let i: usize = k.parse().ok()?;
                    Some((i, v.as_u64()? as u8))
                })
                .collect();
            pairs.sort_by_key(|(i, _)| *i);
            pairs.into_iter().map(|(_, b)| b).collect()
        }
        _ => Vec::new(),
    }
}

pub fn exists(plugin_id: &str, access: &apps::AppAccess, target: &str) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    Ok(json!(Path::new(target).exists()).to_string())
}

pub fn is_directory(
    plugin_id: &str,
    access: &apps::AppAccess,
    target: &str,
) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    Ok(json!(Path::new(target).is_dir()).to_string())
}

pub fn list_dir(plugin_id: &str, access: &apps::AppAccess, target: &str) -> Result<String, String> {
    assert_allowed(plugin_id, access, target)?;
    let meta = std::fs::metadata(target).map_err(|e| format!("fs.listDir failed: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("not a directory: {target}"));
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(target).map_err(|e| format!("fs.listDir failed: {e}"))? {
        let entry = entry.map_err(|e| format!("fs.listDir failed: {e}"))?;
        names.push(json!(entry.file_name().to_string_lossy()));
    }
    Ok(Value::Array(names).to_string())
}

/// `GLINT_FS_PICK_STUB=cancel` → null (tests/CI). Else Win32 `IFileOpenDialog`.
pub fn pick_folder(plugin_id: &str) -> Result<String, String> {
    if let Some(stub) = pick_stub() {
        return finalize_pick(plugin_id, stub, false);
    }
    let chosen = run_file_dialog(true)?;
    finalize_pick(plugin_id, chosen, false)
}

pub fn pick_file(plugin_id: &str) -> Result<String, String> {
    if let Some(stub) = pick_stub() {
        return finalize_pick(plugin_id, stub, true);
    }
    let chosen = run_file_dialog(false)?;
    finalize_pick(plugin_id, chosen, true)
}

fn pick_stub() -> Option<Option<String>> {
    match std::env::var("GLINT_FS_PICK_STUB") {
        Ok(v) if v.eq_ignore_ascii_case("cancel") || v == "1" || v.is_empty() => Some(None),
        Ok(v) => Some(Some(v)),
        Err(_) => None,
    }
}

fn finalize_pick(
    plugin_id: &str,
    chosen: Option<String>,
    add_parent: bool,
) -> Result<String, String> {
    let Some(chosen) = chosen else {
        return Ok("null".into());
    };
    let plugin_dir = apps::apps_root().join(plugin_id);
    let granted = plugin_storage::get(&plugin_dir, "fs:grantedPaths")?;
    let mut next: Vec<String> = match granted {
        Value::Array(arr) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    if !next.iter().any(|p| p == &chosen) {
        next.push(chosen.clone());
    }
    if add_parent {
        if let Some(parent) = Path::new(&chosen).parent() {
            let parent = parent.to_string_lossy().to_string();
            if !parent.is_empty() && !next.iter().any(|p| p == &parent) {
                next.push(parent);
            }
        }
    }
    plugin_storage::set(&plugin_dir, "fs:grantedPaths", Value::Array(next.into_iter().map(Value::String).collect()))?;
    Ok(json!(chosen).to_string())
}

fn run_file_dialog(folder: bool) -> Result<Option<String>, String> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // RPC_E_CHANGED_MODE / S_FALSE are fine if COM already live.
        let should_uninit = hr.is_ok();
        let result = run_file_dialog_inner(folder);
        if should_uninit {
            CoUninitialize();
        }
        result
    }
}

unsafe fn run_file_dialog_inner(folder: bool) -> Result<Option<String>, String> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("file dialog create failed: {e}"))?;

        let mut opts = dialog
            .GetOptions()
            .map_err(|e| format!("file dialog options failed: {e}"))?;
        opts |= FOS_FORCEFILESYSTEM;
        if folder {
            opts |= FOS_PICKFOLDERS;
        }
        dialog
            .SetOptions(opts)
            .map_err(|e| format!("file dialog set options failed: {e}"))?;

        if !folder {
            let filters = [COMDLG_FILTERSPEC {
                pszName: w!("Executable"),
                pszSpec: w!("*.exe"),
            }];
            dialog
                .SetFileTypes(&filters)
                .map_err(|e| format!("file dialog filters failed: {e}"))?;
        }

        match dialog.Show(None) {
            Ok(()) => {}
            Err(e) if e.code() == HRESULT(0x800704C7u32 as i32) /* ERROR_CANCELLED */ => {
                return Ok(None);
            }
            Err(e) => return Err(format!("file dialog failed: {e}")),
        }

        let item: IShellItem = dialog
            .GetResult()
            .map_err(|e| format!("file dialog result failed: {e}"))?;
        let path_ptr: PWSTR = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| format!("file dialog path failed: {e}"))?;
        let path = path_ptr
            .to_string()
            .map_err(|e| format!("file dialog path utf16: {e}"))?;
        CoTaskMemFree(Some(path_ptr.0 as *const _));
        Ok(Some(path))
    }
}

/// Electron `shell.openPath` — empty string on success, error message otherwise.
pub fn open_path(plugin_id: &str, access: &apps::AppAccess, target: &str) -> Result<String, String> {
    let allowed = build_allowed_paths(plugin_id, access)?;
    assert_path_allowed(target, &allowed)?;
    let err = shell_execute_open(target);
    Ok(json!(err).to_string())
}

pub(crate) fn shell_execute_open(target: &str) -> String {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(target)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let rc = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        let code = rc.0 as usize;
        if code > 32 {
            String::new()
        } else {
            format!("ShellExecute failed ({code})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_allows_prefix_and_rejects_sibling() {
        let root = std::env::temp_dir().join("go-fs-allow-root");
        let _ = std::fs::create_dir_all(&root);
        let mut allowed = HashSet::new();
        allowed.insert(absolute_norm(&root).unwrap());
        let inside = root.join("a.txt");
        assert!(assert_path_allowed(inside.to_str().unwrap(), &allowed).is_ok());
        let sibling = root.with_extension("evil");
        assert!(assert_path_allowed(sibling.to_str().unwrap(), &allowed).is_err());
    }

    #[test]
    fn parse_bytes_array_and_uint8_object() {
        assert_eq!(parse_bytes(&json!([1, 2, 255])), vec![1, 2, 255]);
        assert_eq!(
            parse_bytes(&json!({"0": 9, "2": 11, "1": 10})),
            vec![9, 10, 11]
        );
        assert!(parse_bytes(&json!(null)).is_empty());
    }
}
