use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use glint_injector::runtime_root;

use crate::host::resolve_electron_binary;

const LAUNCHER_HOST_REL_DEV: &str = "host/launcher/dist/main.js";
const LAUNCHER_HOST_REL_PKG: &str = "host/launcher/main.js";

pub fn resolve_launcher_host_script(root: &Path) -> Result<PathBuf> {
    if let Ok(path) = env::var("GLINT_LAUNCHER_SCRIPT") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    // Packaged payload keeps TypeScript output under dist/ (same as dev).
    // Flat host/launcher/main.js is only a legacy layout.
    for rel in [LAUNCHER_HOST_REL_DEV, LAUNCHER_HOST_REL_PKG] {
        let script = root.join(rel);
        if script.is_file() {
            return Ok(script);
        }
    }
    anyhow::bail!(
        "launcher-electron not found at {LAUNCHER_HOST_REL_PKG} or {LAUNCHER_HOST_REL_DEV}. \
         Build with: npm run build / scripts/package-release.ps1"
    )
}

pub fn resolve_electron(root: &Path) -> Result<PathBuf> {
    resolve_electron_binary(root)
}

pub fn current_launcher_exe() -> Result<PathBuf> {
    env::current_exe().context("current_exe failed")
}

pub fn spawn_launcher_electron() -> Result<()> {
    let root = runtime_root().context(
        "could not locate Glint install root or repo (missing host/ + native/, or pnpm-workspace.yaml)",
    )?;
    let script = resolve_launcher_host_script(&root)?;
    let electron = resolve_electron(&root)?;
    let launcher_bin = current_launcher_exe()?;

    let mut command = Command::new(&electron);
    command
        .arg(&script)
        .current_dir(&root)
        .env("GLINT_LAUNCHER_BIN", &launcher_bin)
        .env("GLINT_PROJECT_ROOT", &root)
        .env("GLINT_DEBUG", "1")
        // GUI host: do not attach Electron to a console (avoids a CMD window).
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Ok(ui_url) = env::var("GLINT_LAUNCHER_UI_URL") {
        command.env("GLINT_LAUNCHER_UI_URL", ui_url);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Prevent a console flash from Electron/Chromium console-subsystem helpers.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let status = command
        .status()
        .with_context(|| format!("failed to start launcher electron"))?;
    if !status.success() {
        anyhow::bail!("launcher electron exited with {status}");
    }
    Ok(())
}

pub fn ensure_log_dir() -> Result<PathBuf> {
    let dir = crate::console::log_dir();
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn write_startup_error(err: &anyhow::Error) -> Result<()> {
    let dir = ensure_log_dir()?;
    let path = dir.join("launcher-startup-error.log");
    let mut file = fs::File::create(&path)?;
    writeln!(file, "Launcher failed: {err:#}")?;
    writeln!(
        file,
        "Hint: run from the repo root after .\\build-all.ps1"
    )?;
    Ok(())
}
