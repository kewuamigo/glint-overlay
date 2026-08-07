//! Electron-parity ETW metrics pump.
//!
//! Historical host (`apps/overlay-electron` / `host/overlay`) spawned
//! `glint-metrics-etw`, parsed JSON lines, and pushed that snapshot to
//! the UI. CEF cutover dropped the reader; restore it here so FPS is not
//! SHM/Present-hook-only.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{info, warn};

/// Electron `windowsHide: true` — no flash console for the ETW child.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static ETW_LATEST: OnceLock<Arc<Mutex<Option<Value>>>> = OnceLock::new();

/// Shared slot for the latest ETW→SDK metrics JSON (camelCase).
pub fn latest_slot() -> Arc<Mutex<Option<Value>>> {
    ETW_LATEST
        .get_or_init(|| Arc::new(Mutex::new(None)))
        .clone()
}

/// Best-effort start: missing binary / no elevation → slot stays empty; SHM
/// remains the fallback in `plugin_ipc::try_metrics_snapshot`.
pub fn start(pid: u32) {
    let slot = latest_slot();
    let Some(bin) = resolve_etw_bin() else {
        warn!("glint-metrics-etw.exe not found — FPS falls back to Present-hook SHM");
        return;
    };

    let session_id = format!("{pid}-{}-{}", std::process::id(), now_ms());
    info!(%pid, bin = %bin.display(), "starting ETW metrics reader (Electron parity)");

    tokio::spawn(async move {
        run_reader(bin, pid, session_id, slot).await;
    });
}

async fn run_reader(bin: PathBuf, pid: u32, session_id: String, slot: Arc<Mutex<Option<Value>>>) {
    let mut cmd = Command::new(&bin);
    cmd.args([
        pid.to_string(),
        "--interval-ms".into(),
        "500".into(),
        "--trace-suffix".into(),
        session_id,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);

    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = match cmd.spawn()
    {
        Ok(c) => c,
        Err(err) => {
            warn!(%err, bin = %bin.display(), "failed to spawn ETW metrics");
            return;
        }
    };

    let Some(stdout) = child.stdout.take() else {
        warn!("ETW metrics stdout missing");
        return;
    };
    let mut lines = BufReader::new(stdout).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let sdk = etw_snake_to_sdk(&raw);
        if let Ok(mut guard) = slot.lock() {
            *guard = Some(sdk);
        }
    }

    match child.wait().await {
        Ok(status) => {
            if !status.success() {
                warn!(?status, "ETW metrics process exited (Admin required for ETW)");
            }
        }
        Err(err) => warn!(%err, "ETW metrics wait failed"),
    }
}

/// Convert metrics-etw snake_case stdout → SDK `MetricsSnapshot` camelCase.
fn etw_snake_to_sdk(raw: &Value) -> Value {
    let f = |key: &str| raw.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut frame_gen_kind = raw
        .get("frame_gen_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
        .to_string();
    if frame_gen_kind.is_empty() {
        frame_gen_kind = "none".into();
    }
    let mut frame_split_source = raw
        .get("frame_split_source")
        .and_then(|v| v.as_str())
        .unwrap_or("etw")
        .to_string();
    if frame_split_source.is_empty() {
        frame_split_source = "etw".into();
    }
    json!({
        "nativeFps": f("native_fps"),
        "generatedFps": f("generated_fps"),
        "frameGenActive": raw.get("frame_gen_active").and_then(|v| v.as_bool()).unwrap_or(false),
        "frameGenRatio": f("frame_gen_ratio"),
        "frameGenKind": frame_gen_kind,
        "frameSplitSource": frame_split_source,
        "nativeMin": f("native_min"),
        "nativeMax": f("native_max"),
        "generatedMin": f("generated_min"),
        "generatedMax": f("generated_max"),
        "nativeFrameTimeMs": f("native_frame_time_ms"),
        "displayFrameTimeMs": f("display_frame_time_ms"),
    })
}

fn resolve_etw_bin() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GLINT_ETW_BIN") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    const NAME: &str = "glint-metrics-etw.exe";
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Packaged / staged: beside browser in `native/`.
            let beside = dir.join(NAME);
            if beside.is_file() {
                return Some(beside);
            }
            // Dev: browser under host/native, ETW often in target/release.
            let via_root = dir.join("../../target/release").join(NAME);
            if via_root.is_file() {
                return Some(via_root);
            }
        }
    }

    for rel in [
        "target/release/glint-metrics-etw.exe",
        "target/debug/glint-metrics-etw.exe",
        "native/glint-metrics-etw.exe",
        "host/native/glint-metrics-etw.exe",
    ] {
        if let Some(root) = find_project_root() {
            let candidate = root.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let candidate = PathBuf::from(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..8 {
        if dir.join("pnpm-workspace.yaml").is_file() && dir.join("Cargo.toml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent()?.to_path_buf();
        for _ in 0..8 {
            if dir.join("pnpm-workspace.yaml").is_file() && dir.join("Cargo.toml").is_file() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    None
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::etw_snake_to_sdk;
    use serde_json::json;

    #[test]
    fn maps_snake_case_etw_line_to_sdk() {
        let raw = json!({
            "native_fps": 60.0,
            "generated_fps": 120.0,
            "frame_gen_active": true,
            "frame_gen_ratio": 2.0,
            "frame_gen_kind": "dlss",
            "frame_split_source": "hook",
            "native_min": 58.0,
            "native_max": 61.0,
            "generated_min": 118.0,
            "generated_max": 122.0,
            "native_frame_time_ms": 16.6,
            "display_frame_time_ms": 8.3,
        });
        let sdk = etw_snake_to_sdk(&raw);
        assert_eq!(sdk["nativeFps"], 60.0);
        assert_eq!(sdk["generatedFps"], 120.0);
        assert_eq!(sdk["frameGenKind"], "dlss");
        assert_eq!(sdk["frameSplitSource"], "hook");
        assert_eq!(sdk["frameGenActive"], true);
    }
}
