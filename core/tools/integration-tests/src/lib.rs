//! Shared helpers for dynamic integration tests.
//!
//! Set `GLINT_INTEGRATION=1` to run tests that inject into `SpinningCube.exe`.

use std::process::{Child, Command};
use std::time::Duration;

use anyhow::Context;
use glint_injector::{
    overlay_dll_paths, overlay_dll_ref, require_overlay_dll_dir, spinning_cube_path,
};
use glint_overlay_client::{inject, IpcClientConn, IpcClientEventStream};

pub use glint_injector::OverlayDllPaths;

/// True when dynamic inject tests should run (`GLINT_INTEGRATION=1`).
pub fn integration_enabled() -> bool {
    std::env::var("GLINT_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Spawn `SpinningCube.exe` and return the child handle + PID.
pub fn spawn_spinning_cube() -> anyhow::Result<(SpinningCubeProcess, u32)> {
    let path = spinning_cube_path()
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "SpinningCube.exe not found at repo root — place the D3D11 sample there"
            )
        })?;
    let child = Command::new(&path)
        .spawn()
        .with_context(|| format!("failed to spawn {}", path.display()))?;
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(800));
    Ok((SpinningCubeProcess { child }, pid))
}

/// Inject overlay into a running SpinningCube process; returns RAII process + IPC handles.
pub async fn attach_spinning_cube_overlay(
) -> anyhow::Result<(SpinningCubeProcess, u32, IpcClientConn, IpcClientEventStream)> {
    let dll_dir = require_overlay_dll_dir()?;
    let dll_paths = overlay_dll_paths(&dll_dir);
    let (proc, pid) = spawn_spinning_cube()?;
    let (conn, events, _) = inject(
        pid,
        overlay_dll_ref(&dll_paths),
        Some(Duration::from_secs(30)),
    )
    .await?;
    Ok((proc, pid, conn, events))
}

/// Kills the sample process on drop.
pub struct SpinningCubeProcess {
    child: Child,
}

impl Drop for SpinningCubeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Skip helper for tests when integration mode is off.
pub fn skip_unless_integration() -> bool {
    if integration_enabled() {
        false
    } else {
        eprintln!("skip: set GLINT_INTEGRATION=1 to run dynamic inject tests");
        true
    }
}

/// Wait until [`WindowEvent::Added`] or timeout. Returns `(window_id, width, height, gpu_id)`.
pub async fn wait_for_window_added(
    events: &mut IpcClientEventStream,
    timeout: Duration,
) -> anyhow::Result<(u32, u32, u32, glint_overlay_event::GpuLuid)> {
    use glint_overlay_event::{OverlayEvent, WindowEvent};

    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Some(OverlayEvent::Window {
                id,
                event: WindowEvent::Added {
                    width,
                    height,
                    gpu_id,
                },
            })) if width > 0 && height > 0 => {
                return Ok((id, width, height, gpu_id));
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    anyhow::bail!("timed out waiting for WindowEvent::Added from overlay DLL");
}

/// Resize the target game window client area (window `id` is the HWND from overlay events).
pub fn resize_game_window(hwnd: u32, width: i32, height: i32) -> anyhow::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    };

    if width <= 0 || height <= 0 {
        anyhow::bail!("resize dimensions must be positive");
    }

    unsafe {
        SetWindowPos(
            HWND(hwnd as _),
            None,
            0,
            0,
            width,
            height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

/// Wait for [`WindowEvent::Resized`] matching `window_id` (or any size change if `expected` is None).
pub async fn wait_for_window_resized(
    events: &mut IpcClientEventStream,
    window_id: u32,
    expected: Option<(u32, u32)>,
    timeout: Duration,
) -> anyhow::Result<(u32, u32)> {
    use glint_overlay_event::{OverlayEvent, WindowEvent};

    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Some(OverlayEvent::Window {
                id,
                event: WindowEvent::Resized { width, height },
            })) if id == window_id && width > 0 && height > 0 => {
                if let Some((ew, eh)) = expected {
                    if width == ew && height == eh {
                        return Ok((width, height));
                    }
                } else {
                    return Ok((width, height));
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    anyhow::bail!("timed out waiting for WindowEvent::Resized on window {window_id}");
}
