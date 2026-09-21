//! Injection helpers for gameoverlay overlay and metrics DLLs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use glint_overlay_client::{
    ACHIEVEMENTS_DLL_NAME, METRICS_DLL_NAME, common::ipc::create_ipc_addr_cef, inject_dll,
    inject_dll_stopped, inject_overlay_module, overlay_dll_paths, overlay_dll_ref,
};
use tracing::info;

use crate::paths::{find_built_dll, find_overlay_dll_dir};

const INJECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Inject the overlay DLL and return the CEF pipe address (`…-cef`) the UI
/// helper connects as `GLINT_PIPE`. The caller keeps no IPC connection —
/// the helper owns the CEF pipe.
pub fn inject_overlay_cef_pipe(pid: u32, dll_dir: &Path) -> Result<String> {
    let dll_paths = overlay_dll_paths(dll_dir);
    let module_handle =
        inject_overlay_module(pid, overlay_dll_ref(&dll_paths), Some(INJECT_TIMEOUT))
            .context("failed to inject glint-overlay-core")?;

    let addr = create_ipc_addr_cef(pid, module_handle);
    info!(pid, module_handle, %addr, "overlay injected — CEF pipe ready");
    Ok(addr)
}

/// CEF pipe for a process that already has the overlay DLL (no inject).
pub fn overlay_cef_pipe(pid: u32, module_handle: u32) -> String {
    create_ipc_addr_cef(pid, module_handle)
}

pub fn default_overlay_dll_dir() -> PathBuf {
    find_overlay_dll_dir()
}

pub fn default_metrics_dll() -> Option<PathBuf> {
    find_built_dll(METRICS_DLL_NAME)
}

pub fn inject_metrics_dll(pid: u32, dll_path: &Path) -> Result<()> {
    inject_dll_path(pid, dll_path, "metrics")
}

pub fn default_achievements_dll() -> Option<PathBuf> {
    find_built_dll(ACHIEVEMENTS_DLL_NAME)
}

pub fn inject_achievements_dll(pid: u32, dll_path: &Path) -> Result<()> {
    inject_dll_path(pid, dll_path, "achievements")
}

pub fn inject_achievements_dll_stopped(
    process: windows::Win32::Foundation::HANDLE,
    thread: windows::Win32::Foundation::HANDLE,
    dll_path: &Path,
) -> Result<()> {
    let path = dll_path
        .canonicalize()
        .with_context(|| format!("achievements DLL not found: {}", dll_path.display()))?;
    info!(path = %path.display(), "injecting achievements DLL (stopped)");
    inject_dll_stopped(process, thread, &path, Some(INJECT_TIMEOUT))
        .context("failed to inject achievements DLL while stopped")
}

/// Same injection path as glint-overlay-core: NtOpenProcess + RtlCreateUserThread + LoadLibraryW.
fn inject_dll_path(pid: u32, dll_path: &Path, label: &str) -> Result<()> {
    let path = dll_path
        .canonicalize()
        .with_context(|| format!("{label} DLL not found: {}", dll_path.display()))?;

    info!(pid, path = %path.display(), "injecting {label} DLL");

    let module = inject_dll(pid, &path, Some(INJECT_TIMEOUT))
        .with_context(|| format!("failed to inject {label} DLL into pid {pid}"))?;

    info!(pid, module, "{label} DLL loaded in target process");
    Ok(())
}
