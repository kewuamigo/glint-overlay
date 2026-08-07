//! Plugin sandbox invoke dispatch (non-empty `pluginId` path).

use std::path::Path;

use crate::apps::{self, AppAccess};
use crate::plugin_achievements;
use crate::plugin_db;
use crate::plugin_fs;
use crate::plugin_saves;
use crate::plugin_storage;
use glint_metrics_common::{read_metrics_for_pid, MetricsBlock};
use serde_json::Value;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::PWSTR;

/// Methods that need live session state (mode / pipe / CEF focus) in `main`.
pub fn needs_host_session(method: &str) -> bool {
    matches!(
        method,
        "overlay.open"
            | "overlay.close"
            | "overlay.isOpen"
            | "native.window.getSnapshot"
            | "native.window.toggleInteractive"
            | "native.window.setMode"
            | "native.overlay.getGameWindowId"
            | "native.overlay.listenInput"
            | "native.overlay.blockInput"
            | "native.overlay.setBlockingCursor"
            | "browser.focus"
            | "browser.blur"
            | "browser.setContentRect"
            | "browser.openSession"
            | "browser.closeSession"
    )
}

/// Lookup + permission/privileged gate (no side effects).
pub fn gate_plugin(plugin_id: &str, method: &str) -> Result<AppAccess, String> {
    let access = apps::lookup_enabled(plugin_id)
        .ok_or_else(|| format!("unknown plugin: {plugin_id}"))?;
    gate(&access, method)?;
    Ok(access)
}

/// Electron-shaped `WindowManagerSnapshot` JSON.
pub fn window_snapshot_json(
    mode: &str,
    pinned_keys: &std::collections::HashSet<String>,
    win_id: u32,
) -> String {
    let mut panel_pins = serde_json::Map::new();
    for key in pinned_keys {
        panel_pins.insert(key.clone(), Value::Bool(true));
    }
    serde_json::json!({
        "mode": mode,
        "hudPinned": pinned_keys.contains("metrics:main"),
        "panelPins": panel_pins,
        "gameWindowId": win_id,
    })
    .to_string()
}

/// Route a plugin invoke. Unimplemented methods return an explicit error that
/// names the method (FR-008).
pub fn dispatch(plugin_id: &str, method: &str, args_json: &str) -> Result<String, String> {
    let access = gate_plugin(plugin_id, method)?;
    match method {
        "native.metrics.getSnapshot" => metrics_get_snapshot(),
        "game.getProcessInfo" => game_get_process_info(),
        m if m.starts_with("game.saves.") => plugin_saves::dispatch(method, args_json),
        m if m.starts_with("achievements.") => {
            if plugin_id != "achievements" {
                return Err("achievements API is restricted to the achievements app".into());
            }
            plugin_achievements::dispatch(method, args_json)
        }
        "storage.get" | "storage.set" | "storage.remove" => {
            storage_dispatch(plugin_id, method, args_json)
        }
        m if m.starts_with("db.") => db_dispatch(plugin_id, method, args_json),
        m if m.starts_with("fs.") => fs_dispatch(plugin_id, &access, method, args_json),
        "shell.openPath" => shell_open_path(plugin_id, &access, args_json),
        // Geometry: no CEF compositor seam — Electron-shaped ack (Option B).
        "native.overlay.setPosition"
        | "native.overlay.setAnchor"
        | "native.overlay.setMargin" => Ok(String::new()),
        m if needs_host_session(m) => {
            Err(format!("session method requires host context: {method}"))
        }
        _ => Err(format!("unknown plugin method: {method}")),
    }
}

/// CEF path for `game.saves.*`: gate, then rebuild/query on a blocking thread.
pub async fn dispatch_saves_async(
    plugin_id: &str,
    method: &str,
    args_json: &str,
) -> Result<String, String> {
    let access = apps::lookup_enabled(plugin_id)
        .ok_or_else(|| format!("unknown plugin: {plugin_id}"))?;
    gate(&access, method)?;
    if !method.starts_with("game.saves.") {
        return Err(format!("not a saves method: {method}"));
    }
    plugin_saves::dispatch_async(method, args_json).await
}

/// Electron `storage.*`: arg0=key, arg1=value (set). Void → empty JSON (→ JS undefined).
fn storage_dispatch(plugin_id: &str, method: &str, args_json: &str) -> Result<String, String> {
    let args: Vec<Value> = serde_json::from_str(args_json)
        .map_err(|e| format!("invalid storage args: {e}"))?;
    let key = match args.first() {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "undefined".into(),
    };
    let dir = apps::apps_root().join(plugin_id);
    match method {
        "storage.get" => plugin_storage::get(&dir, &key).map(|v| v.to_string()),
        "storage.set" => {
            let value = args.get(1).cloned().unwrap_or(Value::Null);
            plugin_storage::set(&dir, &key, value)?;
            Ok(String::new())
        }
        "storage.remove" => {
            plugin_storage::remove(&dir, &key)?;
            Ok(String::new())
        }
        _ => Err(format!("unknown storage method: {method}")),
    }
}

/// Electron `db.*`: arg0=sql, arg1=params[].
fn db_dispatch(plugin_id: &str, method: &str, args_json: &str) -> Result<String, String> {
    let args: Vec<Value> = serde_json::from_str(args_json)
        .map_err(|e| format!("invalid db args: {e}"))?;
    let sql = match args.first() {
        Some(Value::String(s)) => s.as_str(),
        Some(v) => {
            return Err(format!("db sql must be string, got {v}"));
        }
        None => return Err("db sql required".into()),
    };
    let params: Vec<Value> = match args.get(1) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let dir = apps::apps_root().join(plugin_id);
    match method {
        "db.exec" => plugin_db::exec(&dir, sql),
        "db.run" => plugin_db::run(&dir, sql, &params),
        "db.get" => plugin_db::get(&dir, sql, &params),
        "db.all" => plugin_db::all(&dir, sql, &params),
        _ => Err(format!("unknown db method: {method}")),
    }
}

/// Electron `fs.*`.
fn fs_dispatch(
    plugin_id: &str,
    access: &AppAccess,
    method: &str,
    args_json: &str,
) -> Result<String, String> {
    let args: Vec<Value> = serde_json::from_str(args_json)
        .map_err(|e| format!("invalid fs args: {e}"))?;
    let path_arg = |i: usize| -> String {
        match args.get(i) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => "undefined".into(),
        }
    };
    match method {
        "fs.readText" => plugin_fs::read_text(plugin_id, access, &path_arg(0)),
        "fs.readBytes" => plugin_fs::read_bytes(plugin_id, access, &path_arg(0)),
        "fs.writeText" => {
            let content = match args.get(1) {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            };
            plugin_fs::write_text(plugin_id, access, &path_arg(0), &content)
        }
        "fs.writeBytes" => {
            let data = args.get(1).cloned().unwrap_or(Value::Null);
            plugin_fs::write_bytes(plugin_id, access, &path_arg(0), &data)
        }
        "fs.exists" => plugin_fs::exists(plugin_id, access, &path_arg(0)),
        "fs.isDirectory" => plugin_fs::is_directory(plugin_id, access, &path_arg(0)),
        "fs.listDir" => plugin_fs::list_dir(plugin_id, access, &path_arg(0)),
        "fs.pickFolder" => plugin_fs::pick_folder(plugin_id),
        "fs.pickFile" => plugin_fs::pick_file(plugin_id),
        _ => Err(format!("unknown fs method: {method}")),
    }
}

fn shell_open_path(
    plugin_id: &str,
    access: &AppAccess,
    args_json: &str,
) -> Result<String, String> {
    let args: Vec<Value> = serde_json::from_str(args_json)
        .map_err(|e| format!("invalid shell.openPath args: {e}"))?;
    let target = match args.first() {
        Some(Value::String(s)) => s.as_str(),
        Some(v) => {
            return Err(format!("shell.openPath path must be string, got {v}"));
        }
        None => return Err("shell.openPath path required".into()),
    };
    plugin_fs::open_path(plugin_id, access, target)
}

/// Prefer live ETW snapshot (Electron parity); fall back to Present-hook SHM.
/// Shared by `getSnapshot` invoke and the ≤1 Hz bridge push (FR-004).
pub fn try_metrics_snapshot() -> Result<serde_json::Value, String> {
    if let Some(etw) = etw_snapshot_if_live() {
        return Ok(etw);
    }

    let pid: u32 = std::env::var("GLINT_GAME_PID")
        .map_err(|_| "metrics unavailable: GLINT_GAME_PID not set".to_string())?
        .parse()
        .map_err(|_| "metrics unavailable: invalid GLINT_GAME_PID".to_string())?;
    let block = read_metrics_for_pid(pid)
        .ok_or_else(|| format!("metrics unavailable: no shared memory for pid {pid}"))?;
    Ok(metrics_block_to_json(&block))
}

fn etw_snapshot_if_live() -> Option<serde_json::Value> {
    let slot = crate::etw_reader::latest_slot();
    let guard = slot.lock().ok()?;
    let v = guard.as_ref()?;
    let native = v.get("nativeFps").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let generated = v
        .get("generatedFps")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0);
    if native > 0.0 || generated > 0.0 {
        Some(v.clone())
    } else {
        None
    }
}

fn metrics_get_snapshot() -> Result<String, String> {
    try_metrics_snapshot().map(|v| v.to_string())
}

/// Electron-shaped `{ pid, exePath, exeName, windowTitle }` from `GLINT_GAME_PID`.
fn game_get_process_info() -> Result<String, String> {
    let pid: u32 = std::env::var("GLINT_GAME_PID")
        .map_err(|_| "game process unavailable: GLINT_GAME_PID not set".to_string())?
        .parse()
        .map_err(|_| "game process unavailable: invalid GLINT_GAME_PID".to_string())?;
    let exe_path = resolve_process_exe_path(pid).unwrap_or_default();
    let exe_name = Path::new(&exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    Ok(serde_json::json!({
        "pid": pid,
        "exePath": exe_path,
        "exeName": exe_name,
        "windowTitle": serde_json::Value::Null,
    })
    .to_string())
}

/// Best-effort full image path for `pid` (QueryFullProcessImageNameW).
fn resolve_process_exe_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// Map `MetricsBlock` → JSON matching SDK `MetricsSnapshot` as closely as SHM allows.
fn metrics_block_to_json(block: &MetricsBlock) -> serde_json::Value {
    let present = f64::from(block.native_fps);
    let game = f64::from(block.game_frame_fps);
    let ft = f64::from(block.native_frame_time_ms);

    // Under FG, Present-hook FPS is display rate; game_frame_fps is true native.
    let (native_fps, generated_fps) = if game > 0.0 && game.is_finite() {
        let display = if present > 0.0 && present.is_finite() {
            present
        } else {
            game
        };
        (game, display)
    } else if present > 0.0 && present.is_finite() {
        (present, present)
    } else {
        (0.0, 0.0)
    };

    let frame_gen_kind = match block.fg_kind {
        1 => "dlss",
        2 => "fsr",
        3 => "xefg",
        _ => "none",
    };
    let frame_gen_active =
        native_fps > 0.0 && generated_fps > native_fps * 1.08;
    let frame_gen_ratio = if frame_gen_active {
        generated_fps / native_fps
    } else {
        0.0
    };
    let native_frame_time_ms = if ft > 0.0 {
        ft
    } else if native_fps > 0.0 {
        1000.0 / native_fps
    } else {
        0.0
    };
    let display_frame_time_ms = if generated_fps > 0.0 {
        1000.0 / generated_fps
    } else {
        0.0
    };

    serde_json::json!({
        "nativeFps": native_fps,
        "generatedFps": generated_fps,
        "frameGenActive": frame_gen_active,
        "frameGenRatio": frame_gen_ratio,
        "frameGenKind": frame_gen_kind,
        "frameSplitSource": "hook",
        "nativeMin": native_fps,
        "nativeMax": native_fps,
        "generatedMin": generated_fps,
        "generatedMax": generated_fps,
        "nativeFrameTimeMs": native_frame_time_ms,
        "displayFrameTimeMs": display_frame_time_ms,
    })
}

/// Deny if `method` maps to a permission the plugin lacks. No side effects.
fn gate(access: &AppAccess, method: &str) -> Result<(), String> {
    if let Some(perm) = required_permission(method) {
        if !access.permissions.iter().any(|p| p == perm) {
            return Err(format!("missing permission: {perm}"));
        }
    }
    if is_privileged_browser_method(method) && !access.privileged {
        return Err("browser API requires privileged app".into());
    }
    Ok(())
}

fn is_privileged_browser_method(method: &str) -> bool {
    matches!(
        method,
        "browser.focus" | "browser.blur" | "browser.setContentRect"
    )
}

/// Method / prefix → required permission (parity with Electron `plugin-ipc.ts`
/// `requirePermission` / `hasPermission`, plus contract `native.metrics.*`).
fn required_permission(method: &str) -> Option<&'static str> {
    if method.starts_with("native.metrics.") {
        Some("metrics")
    } else if method.starts_with("storage.") {
        Some("storage")
    } else if method.starts_with("db.") {
        Some("db")
    } else if matches!(
        method,
        "fs.readText" | "fs.readBytes" | "fs.exists" | "fs.isDirectory" | "fs.listDir"
    ) {
        Some("fs:read")
    } else if matches!(method, "fs.writeText" | "fs.writeBytes") {
        Some("fs:write")
    } else if matches!(method, "fs.pickFolder" | "fs.pickFile") {
        Some("fs:pick")
    } else if method == "game.getProcessInfo" || method.starts_with("game.saves.") {
        Some("game:process")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate `GLINT_GAME_PID` (cargo runs tests in parallel).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn metrics_denied_without_permission() {
        let access = AppAccess {
            permissions: vec![],
            privileged: false,
        };
        assert_eq!(
            gate(&access, "native.metrics.getSnapshot").unwrap_err(),
            "missing permission: metrics"
        );
    }

    #[test]
    fn metrics_allowed_passes_gate() {
        let access = AppAccess {
            permissions: vec!["metrics".into()],
            privileged: false,
        };
        assert!(gate(&access, "native.metrics.getSnapshot").is_ok());
    }

    #[test]
    fn unmapped_method_skips_permission_gate() {
        let access = AppAccess {
            permissions: vec![],
            privileged: false,
        };
        assert!(gate(&access, "overlay.isOpen").is_ok());
    }

    /// SC-003: enabled plugin without `metrics` → permission error (not success /
    /// not unknown-method alone). Fixture: builtin `browser` (`permissions: []`).
    #[test]
    fn sc003_dispatch_denies_metrics_without_permission() {
        let err = dispatch("browser", "native.metrics.getSnapshot", "[]")
            .expect_err("undeclared metrics must be denied");
        assert_eq!(err, "missing permission: metrics");
        assert!(!err.contains("unknown plugin method"));
    }

    /// FR-003: enabled metrics fixture + SHM/pid metrics absent → graceful Err
    /// (no panic, not blanket "not supported yet", not "unknown plugin method").
    #[test]
    fn metrics_snapshot_shm_absent_returns_graceful_error() {
        let _guard = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("metrics")
            .expect("builtin metrics app must be discoverable under internal-apps");
        assert!(
            access.permissions.iter().any(|p| p == "metrics"),
            "metrics fixture must declare metrics permission"
        );

        // No attached game pid → no SHM to read (restore after).
        let prev = std::env::var_os("GLINT_GAME_PID");
        unsafe { std::env::remove_var("GLINT_GAME_PID") };

        let result = dispatch("metrics", "native.metrics.getSnapshot", "[]");

        match prev {
            Some(v) => unsafe { std::env::set_var("GLINT_GAME_PID", v) },
            None => {}
        }

        let err = result.expect_err("SHM/pid metrics absent must return Err, not Ok");
        assert!(
            !err.contains("not supported yet"),
            "must not blanket not-supported: {err}"
        );
        assert!(
            !err.contains("unknown plugin method"),
            "method must be implemented (await T011); got: {err}"
        );
    }

    #[test]
    fn get_process_info_denied_without_permission() {
        let err = dispatch("browser", "game.getProcessInfo", "[]")
            .expect_err("undeclared game:process must be denied");
        assert_eq!(err, "missing permission: game:process");
        assert!(!err.contains("unknown plugin method"));
    }

    #[test]
    fn get_process_info_unset_pid_is_not_unknown_method() {
        let _guard = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("achievements")
            .expect("builtin achievements app must be discoverable");
        assert!(
            access.permissions.iter().any(|p| p == "game:process"),
            "achievements fixture must declare game:process"
        );

        let prev = std::env::var_os("GLINT_GAME_PID");
        unsafe { std::env::remove_var("GLINT_GAME_PID") };

        let result = dispatch("achievements", "game.getProcessInfo", "[]");

        match prev {
            Some(v) => unsafe { std::env::set_var("GLINT_GAME_PID", v) },
            None => {}
        }

        let err = result.expect_err("unset GLINT_GAME_PID must return Err");
        assert!(
            err.contains("GLINT_GAME_PID"),
            "clear pid error expected, got: {err}"
        );
        assert!(
            !err.contains("unknown plugin method"),
            "gated method must not be unknown: {err}"
        );
    }

    #[test]
    fn get_process_info_returns_electron_shape() {
        let _guard = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("achievements")
            .expect("builtin achievements app must be discoverable");
        assert!(access.permissions.iter().any(|p| p == "game:process"));

        let pid = std::process::id();
        let prev = std::env::var_os("GLINT_GAME_PID");
        unsafe { std::env::set_var("GLINT_GAME_PID", pid.to_string()) };

        let result = dispatch("achievements", "game.getProcessInfo", "[]");

        match prev {
            Some(v) => unsafe { std::env::set_var("GLINT_GAME_PID", v) },
            None => unsafe { std::env::remove_var("GLINT_GAME_PID") },
        }

        let body = result.expect("current process pid must resolve");
        let v: serde_json::Value =
            serde_json::from_str(&body).expect("process info must be JSON");
        assert_eq!(v["pid"], pid);
        assert!(v["exePath"].as_str().is_some());
        assert!(v["exeName"].as_str().is_some());
        assert!(v["windowTitle"].is_null());
        let exe_path = v["exePath"].as_str().unwrap();
        let exe_name = v["exeName"].as_str().unwrap();
        if !exe_path.is_empty() {
            assert!(
                exe_path.ends_with(exe_name),
                "exeName should be basename of exePath: {exe_path} / {exe_name}"
            );
        }
        assert!(!body.contains("unknown plugin method"));
    }

    #[test]
    fn storage_denied_without_permission() {
        let err = dispatch("browser", "storage.get", r#"["k"]"#)
            .expect_err("undeclared storage must be denied");
        assert_eq!(err, "missing permission: storage");
        assert!(!err.contains("unknown plugin method"));
    }

    #[test]
    fn storage_roundtrip_get_set_remove() {
        let _guard = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("achievements")
            .expect("builtin achievements app must be discoverable");
        assert!(
            access.permissions.iter().any(|p| p == "storage"),
            "achievements fixture must declare storage"
        );

        let tmp = std::env::temp_dir().join(format!(
            "go-f005-apps-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("GLINT_APPS_DIR");
        unsafe { std::env::set_var("GLINT_APPS_DIR", &tmp) };

        let get_missing = dispatch("achievements", "storage.get", r#"["f005-key"]"#);
        let set_ok = dispatch("achievements", "storage.set", r#"["f005-key","hello"]"#);
        let get_ok = dispatch("achievements", "storage.get", r#"["f005-key"]"#);
        let remove_ok = dispatch("achievements", "storage.remove", r#"["f005-key"]"#);
        let get_gone = dispatch("achievements", "storage.get", r#"["f005-key"]"#);

        match prev {
            Some(v) => unsafe { std::env::set_var("GLINT_APPS_DIR", v) },
            None => unsafe { std::env::remove_var("GLINT_APPS_DIR") },
        }
        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(get_missing.expect("get missing"), "null");
        assert_eq!(set_ok.expect("set"), "");
        assert_eq!(get_ok.expect("get after set"), r#""hello""#);
        assert_eq!(remove_ok.expect("remove"), "");
        assert_eq!(get_gone.expect("get after remove"), "null");
    }

    #[test]
    fn fs_denied_without_permission() {
        let err = dispatch("browser", "fs.exists", r#"["C:\\\\tmp"]"#)
            .expect_err("undeclared fs:read must be denied");
        assert_eq!(err, "missing permission: fs:read");
        assert!(!err.contains("unknown plugin method"));
    }

    #[test]
    fn db_denied_without_permission() {
        let err = dispatch("browser", "db.exec", r#"["SELECT 1"]"#)
            .expect_err("undeclared db must be denied");
        assert_eq!(err, "missing permission: db");
        assert!(!err.contains("unknown plugin method"));
    }

    #[test]
    fn fs_db_shell_inventory_not_unknown_when_permitted() {
        let _guard = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("achievements")
            .expect("builtin achievements app must be discoverable");
        assert!(access.permissions.iter().any(|p| p == "fs:read"));
        assert!(access.permissions.iter().any(|p| p == "db"));

        let tmp = std::env::temp_dir().join(format!(
            "go-f006-apps-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let plugin_dir = tmp.join("achievements");
        std::fs::create_dir_all(plugin_dir.join("data")).unwrap();
        let sample = plugin_dir.join("data").join("f006.txt");
        std::fs::write(&sample, "hello").unwrap();

        let prev_apps = std::env::var_os("GLINT_APPS_DIR");
        let prev_pick = std::env::var_os("GLINT_FS_PICK_STUB");
        unsafe {
            std::env::set_var("GLINT_APPS_DIR", &tmp);
            std::env::set_var("GLINT_FS_PICK_STUB", "cancel");
        }

        let path = sample.to_string_lossy().replace('\\', "\\\\");
        let exists = dispatch("achievements", "fs.exists", &format!(r#"["{path}"]"#));
        let read = dispatch("achievements", "fs.readText", &format!(r#"["{path}"]"#));
        let bytes = dispatch("achievements", "fs.readBytes", &format!(r#"["{path}"]"#));
        let is_dir = dispatch(
            "achievements",
            "fs.isDirectory",
            &format!(
                r#"["{}"]"#,
                plugin_dir.join("data").to_string_lossy().replace('\\', "\\\\")
            ),
        );
        let listed = dispatch(
            "achievements",
            "fs.listDir",
            &format!(
                r#"["{}"]"#,
                plugin_dir.join("data").to_string_lossy().replace('\\', "\\\\")
            ),
        );
        let pick_folder = dispatch("achievements", "fs.pickFolder", "[]");
        let pick_file = dispatch("achievements", "fs.pickFile", "[]");
        let db_exec = dispatch(
            "achievements",
            "db.exec",
            r#"["CREATE TABLE IF NOT EXISTS f006 (id INTEGER PRIMARY KEY)"]"#,
        );
        let db_run = dispatch(
            "achievements",
            "db.run",
            r#"["INSERT INTO f006 DEFAULT VALUES", []]"#,
        );
        let db_get = dispatch(
            "achievements",
            "db.get",
            r#"["SELECT id FROM f006 LIMIT 1", []]"#,
        );
        let db_all = dispatch("achievements", "db.all", r#"["SELECT id FROM f006", []]"#);
        let open = dispatch(
            "achievements",
            "shell.openPath",
            r#"["C:\\\\Windows\\\\System32\\\\drivers"]"#,
        );

        // write via a user app under apps_root (achievements lacks fs:write)
        let writer = tmp.join("fs-writer");
        std::fs::create_dir_all(&writer).unwrap();
        std::fs::write(writer.join("index.js"), "//").unwrap();
        std::fs::write(
            writer.join("manifest.json"),
            r#"{
  "id": "fs-writer",
  "name": "FS Writer",
  "version": "1.0.0",
  "entry": "./index.js",
  "permissions": ["fs:read", "fs:write"],
  "panels": [{"id": "main", "title": "W"}]
}"#,
        )
        .unwrap();
        let out = writer.join("data").join("out.bin");
        let out_path = out.to_string_lossy().replace('\\', "\\\\");
        let write_text = dispatch(
            "fs-writer",
            "fs.writeText",
            &format!(r#"["{out_path}","hi"]"#),
        );
        let write_bytes = dispatch(
            "fs-writer",
            "fs.writeBytes",
            &format!(r#"["{out_path}",[104,105]]"#),
        );

        let results = [
            ("fs.exists", exists),
            ("fs.readText", read),
            ("fs.readBytes", bytes),
            ("fs.isDirectory", is_dir),
            ("fs.listDir", listed),
            ("fs.pickFolder", pick_folder),
            ("fs.pickFile", pick_file),
            ("db.exec", db_exec),
            ("db.run", db_run),
            ("db.get", db_get),
            ("db.all", db_all),
            ("shell.openPath", open),
            ("fs.writeText", write_text),
            ("fs.writeBytes", write_bytes),
        ];

        match prev_apps {
            Some(v) => unsafe { std::env::set_var("GLINT_APPS_DIR", v) },
            None => unsafe { std::env::remove_var("GLINT_APPS_DIR") },
        }
        match prev_pick {
            Some(v) => unsafe { std::env::set_var("GLINT_FS_PICK_STUB", v) },
            None => unsafe { std::env::remove_var("GLINT_FS_PICK_STUB") },
        }
        let _ = std::fs::remove_dir_all(&tmp);

        for (name, result) in results {
            match name {
                "shell.openPath" => {
                    let err = result.expect_err("outside allowlist must Err");
                    assert!(
                        err.contains("path not allowed"),
                        "shell.openPath allowlist: {err}"
                    );
                    assert!(!err.contains("unknown plugin method"));
                }
                _ => {
                    let body = result.unwrap_or_else(|e| panic!("{name} must not fail: {e}"));
                    assert!(
                        !body.contains("unknown plugin method"),
                        "{name} unknown: {body}"
                    );
                }
            }
        }
    }

    #[test]
    fn game_saves_denied_without_permission() {
        let err = dispatch("browser", "game.saves.findGame", r#"["Hades"]"#)
            .expect_err("undeclared game:process must be denied");
        assert_eq!(err, "missing permission: game:process");
        assert!(!err.contains("unknown plugin method"));
    }

    #[test]
    fn game_saves_find_and_update_not_unknown() {
        let _guard = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("achievements")
            .expect("builtin achievements app must be discoverable");
        assert!(access.permissions.iter().any(|p| p == "game:process"));

        let tmp = std::env::temp_dir().join(format!(
            "go-f007-apps-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data = tmp.join("save-manager").join("data");
        std::fs::create_dir_all(&data).unwrap();

        // Seed a tiny Ludusavi-shaped DB (fixture semantics).
        {
            let db_path = data.join("manifest.db");
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE games (title TEXT PRIMARY KEY, steam_id INTEGER, notes TEXT);
                CREATE TABLE game_aliases (alias_title TEXT PRIMARY KEY, target_title TEXT NOT NULL);
                CREATE TABLE game_files (
                  game_title TEXT NOT NULL,
                  path_template TEXT NOT NULL,
                  tags TEXT NOT NULL DEFAULT '[\"save\"]'
                );
                CREATE TABLE game_lookup (
                  game_title TEXT NOT NULL,
                  norm TEXT NOT NULL,
                  kind TEXT NOT NULL
                );
                INSERT INTO games (title, steam_id, notes) VALUES ('Hades', NULL, NULL);
                INSERT INTO game_lookup (game_title, norm, kind) VALUES ('Hades', 'hades', 'title');
                INSERT INTO game_files (game_title, path_template, tags)
                  VALUES ('Hades', '<winAppData>/Hades', '[\"save\"]');
                ",
            )
            .unwrap();
            std::fs::write(
                data.join("manifest.meta.json"),
                r#"{"downloadedAt":"2099-01-01T00:00:00.000Z"}"#,
            )
            .unwrap();
        }

        let prev_apps = std::env::var_os("GLINT_APPS_DIR");
        let prev_offline = std::env::var_os("GLINT_SAVE_MANIFEST_OFFLINE");
        let prev_pid = std::env::var_os("GLINT_GAME_PID");
        unsafe {
            std::env::set_var("GLINT_APPS_DIR", &tmp);
            std::env::set_var("GLINT_SAVE_MANIFEST_OFFLINE", "1");
            std::env::remove_var("GLINT_GAME_PID");
        }

        let find = dispatch("achievements", "game.saves.findGame", r#"["Hades"]"#);
        let update = dispatch("achievements", "game.saves.updateManifest", "[]");
        let get_dir = dispatch("achievements", "game.saves.getGameDir", "[]");
        let entry = dispatch("achievements", "game.saves.getManifestEntry", "[]");
        let locs = dispatch("achievements", "game.saves.getSaveLocations", "[]");

        match prev_apps {
            Some(v) => unsafe { std::env::set_var("GLINT_APPS_DIR", v) },
            None => unsafe { std::env::remove_var("GLINT_APPS_DIR") },
        }
        match prev_offline {
            Some(v) => unsafe { std::env::set_var("GLINT_SAVE_MANIFEST_OFFLINE", v) },
            None => unsafe { std::env::remove_var("GLINT_SAVE_MANIFEST_OFFLINE") },
        }
        match prev_pid {
            Some(v) => unsafe { std::env::set_var("GLINT_GAME_PID", v) },
            None => unsafe { std::env::remove_var("GLINT_GAME_PID") },
        }
        let _ = std::fs::remove_dir_all(&tmp);

        let find_body = find.expect("findGame must succeed");
        assert!(
            !find_body.contains("unknown plugin method"),
            "findGame unknown: {find_body}"
        );
        let found: serde_json::Value = serde_json::from_str(&find_body).unwrap();
        assert!(
            found.as_array().is_some_and(|a| !a.is_empty()),
            "findGame should return Hades: {find_body}"
        );

        let update_body = update.expect("updateManifest must succeed");
        assert!(!update_body.contains("unknown plugin method"));
        let updated: serde_json::Value = serde_json::from_str(&update_body).unwrap();
        assert_eq!(updated["updated"], true);
        assert!(updated["gameCount"].as_i64().unwrap_or(0) >= 1);

        assert_eq!(get_dir.expect("getGameDir"), r#""""#);
        assert_eq!(entry.expect("getManifestEntry"), "null");
        assert_eq!(locs.expect("getSaveLocations"), "[]");
    }

    #[test]
    fn achievements_restricted_to_achievements_app() {
        let _lock = ENV_LOCK.lock().unwrap();
        let err = dispatch("metrics", "achievements.listGames", "[]")
            .expect_err("must restrict");
        assert!(
            err.contains("restricted to the achievements app"),
            "got: {err}"
        );
        assert!(
            !err.contains("unknown plugin method"),
            "must not look unknown: {err}"
        );
    }

    #[test]
    fn achievements_bind_set_inventory() {
        let _lock = ENV_LOCK.lock().unwrap();
        let access = apps::lookup_enabled("achievements")
            .expect("builtin achievements app must be discoverable");
        assert_eq!(access.permissions.is_empty(), false);

        let tmp = std::env::temp_dir().join(format!(
            "go-f008-ach-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("achievements")).unwrap();
        let prev_apps = std::env::var_os("GLINT_APPS_DIR");
        let prev_stub = std::env::var_os("GLINT_OPENURL_STUB");
        unsafe {
            std::env::set_var("GLINT_APPS_DIR", &tmp);
            std::env::set_var("GLINT_OPENURL_STUB", "1");
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        plugin_achievements::install_toast_sender(tx);

        let list = dispatch("achievements", "achievements.listGames", "[]");
        let for_game = dispatch("achievements", "achievements.listForGame", r#"["missing"]"#);
        let detect = dispatch(
            "achievements",
            "achievements.detectExe",
            &format!(
                r#"["{}"]"#,
                tmp.join("SpinningCube.exe").to_string_lossy().replace('\\', "\\\\")
            ),
        );
        // Create a fake exe dir with steam_api so detect finds markers.
        let game_dir = tmp.join("SpinningCube");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(game_dir.join("steam_api64.dll"), b"").unwrap();
        let exe = game_dir.join("SpinningCube.exe");
        std::fs::write(&exe, b"").unwrap();
        let exe_json = format!(
            r#"["{}"]"#,
            exe.to_string_lossy().replace('\\', "\\\\")
        );
        let detect2 = dispatch("achievements", "achievements.detectExe", &exe_json);
        let track = dispatch("achievements", "achievements.trackExe", &exe_json);
        let for_proc = dispatch(
            "achievements",
            "achievements.forProcess",
            &format!(
                r#"["{}","SpinningCube.exe"]"#,
                exe.to_string_lossy().replace('\\', "\\\\")
            ),
        );
        let guide = dispatch("achievements", "achievements.getGuide", r#"["gse"]"#);
        let toast = dispatch("achievements", "achievements.testToast", "[]");
        // openUrl: don't actually open; skip live ShellExecute in CI — call getGuide only.
        // Still assert openUrl is not "unknown".
        let open = dispatch(
            "achievements",
            "achievements.openUrl",
            r#"["about:blank"]"#,
        );

        match prev_apps {
            Some(v) => unsafe { std::env::set_var("GLINT_APPS_DIR", v) },
            None => unsafe { std::env::remove_var("GLINT_APPS_DIR") },
        }
        match prev_stub {
            Some(v) => unsafe { std::env::set_var("GLINT_OPENURL_STUB", v) },
            None => unsafe { std::env::remove_var("GLINT_OPENURL_STUB") },
        }
        let _ = std::fs::remove_dir_all(&tmp);

        for (name, res) in [
            ("listGames", list),
            ("listForGame", for_game),
            ("detectExe", detect),
            ("detectExe2", detect2),
            ("trackExe", track),
            ("forProcess", for_proc),
            ("getGuide", guide),
            ("testToast", toast),
            ("openUrl", open),
        ] {
            match res {
                Ok(body) => assert!(
                    !body.contains("unknown plugin method"),
                    "{name} unknown: {body}"
                ),
                Err(err) => assert!(
                    !err.contains("unknown plugin method")
                        && !err.contains("unknown achievements method"),
                    "{name} err looks unknown: {err}"
                ),
            }
        }

        // testToast must have pushed at least the empty-replay toast.
        let pushed = rx.try_recv();
        assert!(
            pushed.is_ok(),
            "testToast must push a toast on the host→UI channel"
        );
    }

    #[test]
    fn geometry_ack_is_not_unknown_method() {
        for method in [
            "native.overlay.setPosition",
            "native.overlay.setAnchor",
            "native.overlay.setMargin",
        ] {
            let body = dispatch("browser", method, "[0,0,0,0]").expect(method);
            assert!(body.is_empty(), "{method} should ack empty");
        }
    }

    #[test]
    fn privileged_browser_set_content_rect_needs_session() {
        let err = dispatch("browser", "browser.setContentRect", "[null]")
            .expect_err("setContentRect requires host session context");
        assert!(
            err.contains("session method requires host context"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn browser_focus_denied_without_privileged() {
        let err = dispatch("metrics", "browser.focus", "[]")
            .expect_err("non-privileged must be denied");
        assert!(err.contains("privileged"));
        assert!(!err.contains("unknown plugin method"));
    }

    #[test]
    fn session_methods_are_not_unknown_when_gated() {
        for method in [
            "overlay.open",
            "overlay.close",
            "overlay.isOpen",
            "native.window.getSnapshot",
            "native.window.toggleInteractive",
            "native.window.setMode",
            "native.overlay.getGameWindowId",
            "native.overlay.listenInput",
            "native.overlay.blockInput",
            "native.overlay.setBlockingCursor",
            "browser.focus",
            "browser.blur",
        ] {
            let err = dispatch("browser", method, "[]").expect_err(method);
            assert!(
                !err.contains("unknown plugin method"),
                "{method}: {err}"
            );
            assert!(
                err.contains("session method requires host context"),
                "{method}: {err}"
            );
        }
    }

    /// SC-009 / FR-013: full Electron `plugin-ipc` inventory from
    /// `contracts/plugin-invoke.md` — success, domain error, permission deny,
    /// or session-host-context Err only; never `unknown plugin method`.
    #[test]
    fn sc009_full_fr013_inventory_never_unknown_method() {
        let _guard = ENV_LOCK.lock().unwrap();

        let tmp = std::env::temp_dir().join(format!(
            "go-f012-inv-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("achievements").join("data")).unwrap();
        std::fs::create_dir_all(tmp.join("save-manager").join("data")).unwrap();

        // Tiny Ludusavi-shaped cache so saves methods stay offline/fast.
        {
            let data = tmp.join("save-manager").join("data");
            let conn = rusqlite::Connection::open(data.join("manifest.db")).unwrap();
            conn.execute_batch(
                "
                CREATE TABLE games (title TEXT PRIMARY KEY, steam_id INTEGER, notes TEXT);
                CREATE TABLE game_aliases (alias_title TEXT PRIMARY KEY, target_title TEXT NOT NULL);
                CREATE TABLE game_files (
                  game_title TEXT NOT NULL,
                  path_template TEXT NOT NULL,
                  tags TEXT NOT NULL DEFAULT '[\"save\"]'
                );
                CREATE TABLE game_lookup (
                  game_title TEXT NOT NULL,
                  norm TEXT NOT NULL,
                  kind TEXT NOT NULL
                );
                INSERT INTO games (title, steam_id, notes) VALUES ('SpinningCube', NULL, NULL);
                INSERT INTO game_lookup (game_title, norm, kind)
                  VALUES ('SpinningCube', 'spinningcube', 'title');
                INSERT INTO game_files (game_title, path_template, tags)
                  VALUES ('SpinningCube', '<winAppData>/SpinningCube', '[\"save\"]');
                ",
            )
            .unwrap();
            std::fs::write(
                data.join("manifest.meta.json"),
                r#"{"downloadedAt":"2099-01-01T00:00:00.000Z"}"#,
            )
            .unwrap();
            std::fs::write(
                data.join("manifest.yaml"),
                "SpinningCube:\n  files:\n    \"<winAppData>/SpinningCube\":\n      tags: [save]\n",
            )
            .unwrap();
        }

        // fs:write fixture (achievements lacks fs:write).
        let writer = tmp.join("fs-writer");
        std::fs::create_dir_all(writer.join("data")).unwrap();
        std::fs::write(writer.join("index.js"), "//").unwrap();
        std::fs::write(
            writer.join("manifest.json"),
            r#"{
  "id": "fs-writer",
  "name": "FS Writer",
  "version": "1.0.0",
  "entry": "./index.js",
  "permissions": ["fs:read", "fs:write"],
  "panels": [{"id": "main", "title": "W"}]
}"#,
        )
        .unwrap();
        let sample = tmp.join("achievements").join("data").join("f012.txt");
        std::fs::write(&sample, "hi").unwrap();
        let sample_arg = format!(
            r#"["{}"]"#,
            sample.to_string_lossy().replace('\\', "\\\\")
        );
        let data_arg = format!(
            r#"["{}"]"#,
            tmp.join("achievements")
                .join("data")
                .to_string_lossy()
                .replace('\\', "\\\\")
        );
        let write_text_arg = format!(
            r#"["{}","hi"]"#,
            writer
                .join("data")
                .join("out.txt")
                .to_string_lossy()
                .replace('\\', "\\\\")
        );
        let write_bytes_arg = format!(
            r#"["{}",[104,105]]"#,
            writer
                .join("data")
                .join("out.txt")
                .to_string_lossy()
                .replace('\\', "\\\\")
        );
        let cube_dir = tmp.join("SpinningCube");
        std::fs::create_dir_all(&cube_dir).unwrap();
        std::fs::write(cube_dir.join("steam_api64.dll"), b"").unwrap();
        let cube_exe = cube_dir.join("SpinningCube.exe");
        std::fs::write(&cube_exe, b"").unwrap();
        let cube_exe_esc = cube_exe.to_string_lossy().replace('\\', "\\\\");
        let cube_exe_arg = format!(r#"["{cube_exe_esc}"]"#);
        let for_proc_arg = format!(r#"["{cube_exe_esc}","SpinningCube.exe"]"#);

        let prev_apps = std::env::var_os("GLINT_APPS_DIR");
        let prev_pick = std::env::var_os("GLINT_FS_PICK_STUB");
        let prev_url = std::env::var_os("GLINT_OPENURL_STUB");
        let prev_offline = std::env::var_os("GLINT_SAVE_MANIFEST_OFFLINE");
        let prev_pid = std::env::var_os("GLINT_GAME_PID");
        unsafe {
            std::env::set_var("GLINT_APPS_DIR", &tmp);
            std::env::set_var("GLINT_FS_PICK_STUB", "cancel");
            std::env::set_var("GLINT_OPENURL_STUB", "1");
            std::env::set_var("GLINT_SAVE_MANIFEST_OFFLINE", "1");
            std::env::remove_var("GLINT_GAME_PID");
        }

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        plugin_achievements::install_toast_sender(tx);

        // (pluginId, method, args) — gates still apply; domain / session Err OK.
        let cases: Vec<(&str, &str, String)> = vec![
            ("metrics", "native.metrics.getSnapshot", "[]".into()),
            ("browser", "native.window.getSnapshot", "[]".into()),
            ("browser", "native.window.toggleInteractive", "[]".into()),
            ("browser", "native.window.setMode", r#"["Interactive"]"#.into()),
            ("browser", "native.overlay.getGameWindowId", "[]".into()),
            ("browser", "native.overlay.setPosition", "[0,0,0,0]".into()),
            ("browser", "native.overlay.setAnchor", "[0,0]".into()),
            ("browser", "native.overlay.setMargin", "[0,0,0,0]".into()),
            ("browser", "native.overlay.listenInput", "[false,false]".into()),
            ("browser", "native.overlay.blockInput", "[false]".into()),
            ("browser", "native.overlay.setBlockingCursor", "[null]".into()),
            ("achievements", "storage.get", r#"["f012"]"#.into()),
            ("achievements", "storage.set", r#"["f012","v"]"#.into()),
            ("achievements", "storage.remove", r#"["f012"]"#.into()),
            (
                "achievements",
                "db.exec",
                r#"["CREATE TABLE IF NOT EXISTS f012 (id INTEGER PRIMARY KEY)"]"#.into(),
            ),
            (
                "achievements",
                "db.run",
                r#"["INSERT INTO f012 DEFAULT VALUES", []]"#.into(),
            ),
            (
                "achievements",
                "db.get",
                r#"["SELECT id FROM f012 LIMIT 1", []]"#.into(),
            ),
            ("achievements", "db.all", r#"["SELECT id FROM f012", []]"#.into()),
            ("achievements", "fs.readText", sample_arg.clone()),
            ("achievements", "fs.readBytes", sample_arg.clone()),
            ("achievements", "fs.exists", sample_arg),
            ("achievements", "fs.isDirectory", data_arg.clone()),
            ("achievements", "fs.listDir", data_arg.clone()),
            ("fs-writer", "fs.writeText", write_text_arg),
            ("fs-writer", "fs.writeBytes", write_bytes_arg),
            ("achievements", "fs.pickFolder", "[]".into()),
            ("achievements", "fs.pickFile", "[]".into()),
            (
                "achievements",
                "shell.openPath",
                r#"["C:\\Windows\\System32\\drivers"]"#.into(),
            ),
            ("achievements", "game.getProcessInfo", "[]".into()),
            ("achievements", "game.saves.getGameDir", "[]".into()),
            ("achievements", "game.saves.getManifestEntry", "[]".into()),
            ("achievements", "game.saves.getSaveLocations", "[]".into()),
            ("achievements", "game.saves.findGame", r#"["SpinningCube"]"#.into()),
            ("achievements", "game.saves.updateManifest", "[]".into()),
            ("achievements", "achievements.listGames", "[]".into()),
            ("achievements", "achievements.listForGame", r#"["missing"]"#.into()),
            ("achievements", "achievements.detectExe", cube_exe_arg.clone()),
            ("achievements", "achievements.trackExe", cube_exe_arg),
            ("achievements", "achievements.forProcess", for_proc_arg),
            ("achievements", "achievements.getGuide", r#"["gse"]"#.into()),
            ("achievements", "achievements.openUrl", r#"["about:blank"]"#.into()),
            ("achievements", "achievements.testToast", "[]".into()),
            ("browser", "overlay.open", "[]".into()),
            ("browser", "overlay.close", "[]".into()),
            ("browser", "overlay.isOpen", "[]".into()),
            ("browser", "browser.focus", "[]".into()),
            ("browser", "browser.blur", "[]".into()),
            ("browser", "browser.setContentRect", "[null]".into()),
        ];

        let mut failures = Vec::new();
        for (plugin, method, args) in &cases {
            match dispatch(plugin, method, args) {
                Ok(body) => {
                    if body.contains("unknown plugin method") {
                        failures.push(format!("{method}: Ok body has unknown: {body}"));
                    }
                }
                Err(err) => {
                    if err.contains("unknown plugin method") {
                        failures.push(format!("{method}: {err}"));
                    }
                }
            }
        }

        match prev_apps {
            Some(v) => unsafe { std::env::set_var("GLINT_APPS_DIR", v) },
            None => unsafe { std::env::remove_var("GLINT_APPS_DIR") },
        }
        match prev_pick {
            Some(v) => unsafe { std::env::set_var("GLINT_FS_PICK_STUB", v) },
            None => unsafe { std::env::remove_var("GLINT_FS_PICK_STUB") },
        }
        match prev_url {
            Some(v) => unsafe { std::env::set_var("GLINT_OPENURL_STUB", v) },
            None => unsafe { std::env::remove_var("GLINT_OPENURL_STUB") },
        }
        match prev_offline {
            Some(v) => unsafe { std::env::set_var("GLINT_SAVE_MANIFEST_OFFLINE", v) },
            None => unsafe { std::env::remove_var("GLINT_SAVE_MANIFEST_OFFLINE") },
        }
        match prev_pid {
            Some(v) => unsafe { std::env::set_var("GLINT_GAME_PID", v) },
            None => unsafe { std::env::remove_var("GLINT_GAME_PID") },
        }
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(
            failures.is_empty(),
            "SC-009 inventory unknown-method failures:\n{}",
            failures.join("\n")
        );
        assert_eq!(cases.len(), 48, "FR-013 inventory row count drift");
    }

    #[test]
    fn window_snapshot_json_shape() {
        let mut pins = std::collections::HashSet::new();
        pins.insert("metrics:main".into());
        let body = window_snapshot_json("interactive", &pins, 42);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["mode"], "interactive");
        assert_eq!(v["hudPinned"], true);
        assert_eq!(v["gameWindowId"], 42);
        assert_eq!(v["panelPins"]["metrics:main"], true);
    }
}
