use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{Context, Result};
use glint_injector::{
    default_metrics_dll, inject_metrics_dll, inject_overlay_cef_pipe, project_root, runtime_root,
    require_overlay_dll_dir,
};

use crate::console::session_log_path;

const BROWSER_REL: &str = "host/native/glint-browser.exe";
const SHELL_DIST_REL: &str = "ui/shell/dist";
pub const ELECTRON_REL: &str =
    "node_modules/.pnpm/electron@40.10.6/node_modules/electron/dist/electron.exe";
pub const ELECTRON_FALLBACK_REL: &str = "node_modules/electron/dist/electron.exe";

pub fn resolve_electron_binary(root: &Path) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("GLINT_ELECTRON_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    for rel in [
        "host/electron/electron.exe",
        "host/launcher/node_modules/electron/dist/electron.exe",
        ELECTRON_REL,
        ELECTRON_FALLBACK_REL,
    ] {
        let path = root.join(rel);
        if path.is_file() {
            return Ok(path);
        }
    }

    anyhow::bail!(
        "electron.exe not found. Run: npx pnpm@10.12.1 install (and let electron download its binary)"
    )
}

pub fn resolve_browser_exe(root: &Path) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("GLINT_BROWSER_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    for rel in [
        "native/glint-browser.exe",
        BROWSER_REL,
        "target/release/glint-browser.exe",
        "target/debug/glint-browser.exe",
    ] {
        let path = root.join(rel);
        if path.is_file() {
            return Ok(path);
        }
    }

    anyhow::bail!(
        "glint-browser.exe not found at {BROWSER_REL}. \
         Build it with: .\\scripts\\build-browser.ps1 (or build-all.ps1)"
    )
}

/// Attach path: injects the overlay DLL and starts the CEF UI helper, which
/// hosts the shell document itself (no Electron in the session).
/// Returns (child, log file path).
fn spawn_cef_ui_helper(pid: u32) -> Result<(Child, PathBuf)> {
    let root = runtime_root()
        .or_else(project_root)
        .context(
        "could not locate Glint project root (missing pnpm-workspace.yaml + Cargo.toml)",
    )?;
    let exe = resolve_browser_exe(&root)?;

    let shell_dist = root.join(SHELL_DIST_REL);
    if !shell_dist.join("index.html").is_file() {
        anyhow::bail!(
            "shell UI not built at {SHELL_DIST_REL}/index.html. \
             Build it with: npm run build (or build-all.ps1)"
        );
    }

    let log_file = session_log_path(pid);
    if let Some(parent) = log_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Helper logs via tracing to stderr; detached, so the session log is the sink.
    let log = fs::File::create(&log_file)
        .with_context(|| format!("cannot create session log {}", log_file.display()))?;

    let dll_dir = require_overlay_dll_dir()?;
    let cef_pipe = inject_overlay_cef_pipe(pid, &dll_dir)?;
    // Best-effort — same as the pre-cutover Electron host; attach must not fail.
    if let Some(metrics) = default_metrics_dll() {
        if let Err(err) = inject_metrics_dll(pid, &metrics) {
            eprintln!("[glint] metrics DLL inject failed: {err:#}");
        }
    } else {
        eprintln!(
            "[glint] glint_metrics_native.dll not found — \
             Present-hook FPS unavailable (build -p glint-metrics-native)"
        );
    }

    let mut command = Command::new(&exe);
    command
        .current_dir(&root)
        .env("GLINT_PIPE", &cef_pipe)
        .env("GLINT_GAME_PID", pid.to_string())
        .env("GLINT_UI_URL", &shell_dist)
        .env("GLINT_BUILTIN_APPS_DIR", root.join("internal-apps"))
        .env(
            "GLINT_PLUGIN_SHARED_DIR",
            root.join("host/cef/plugin-shared"),
        )
        .env("RUST_LOG", "info")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));

    let etw_bin = root.join("native/glint-metrics-etw.exe");
    if etw_bin.is_file() {
        command.env("GLINT_ETW_BIN", &etw_bin);
    }

    #[cfg(windows)]
    {
        // Detach UI helper — must not share parent lifetime with launcher CLI.
        // Do not set CREATE_NO_WINDOW here: this spawns the CEF UI helper and the
        // flag can break Chromium subprocess / OSR paint (same as cef_child.rs).
        // Stdio is redirected to the session log; DETACHED_PROCESS avoids a console.
        const DETACHED_PROCESS: u32 = 0x00000008;
        command.creation_flags(DETACHED_PROCESS);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("failed to start CEF UI helper for pid {pid}"))?;

    // Brief settle: catch immediate crash so attach returns a clear error (not silent).
    std::thread::sleep(Duration::from_millis(1000));
    if let Some(status) = child
        .try_wait()
        .context("failed to poll CEF UI helper after spawn")?
    {
        anyhow::bail!(
            "CEF UI helper exited immediately ({status}). \
             Overlay is injected but there is no UI. See log: {}",
            log_file.display()
        );
    }

    Ok((child, log_file))
}

/// Starts the in-game UI for an attach: always the CEF UI helper.
pub fn spawn_overlay_host(pid: u32, target_exe: Option<&str>) -> Result<(Child, PathBuf)> {
    let _ = target_exe;
    spawn_cef_ui_helper(pid)
}
