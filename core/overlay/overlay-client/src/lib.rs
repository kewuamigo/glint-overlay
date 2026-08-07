//! Library for attaching `glint-overlay-core` to a process and initiating IPC channel.
//!
//! By utilizing this library, you can render overlay from any process and control it via IPC.
//! It's designed to give you maximum flexibility as you can keep most of the logic in this process.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use std::time::Duration;
//! use glint_overlay_client::{inject, OverlayDll};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let dll = OverlayDll {
//!         x64: Some(Path::new("glint-overlay-core-x64.dll")),
//!         x86: Some(Path::new("glint-overlay-core-x86.dll")),
//!         arm64: Some(Path::new("glint-overlay-core-arm64.dll")),
//!     };
//!
//!    let (mut conn, mut events, _pipe) = inject(
//!         1234, // target process pid
//!         dll, // overlay dll paths
//!         Some(Duration::from_secs(10)), // timeout for injection and ipc connection
//!    ).await?;
//!
//!   // Use `conn` to send requests to overlay, and `events` to receive events from the overlay.
//!
//!   Ok(())
//! }
//!

pub mod client;
mod injector;
pub mod paths;
#[cfg(feature = "surface")]
pub mod surface;
#[cfg(feature = "surface")]
pub mod ty;

pub use glint_overlay_common as common;
pub use glint_overlay_event as event;

use core::time::Duration;
use std::ffi::OsStr;
use std::path::Path;

use anyhow::{Context, bail};
use glint_overlay_common::ipc::create_ipc_addr;
use tokio::{net::windows::named_pipe::ClientOptions, select, time::sleep};

pub use crate::client::{IpcClientConn, IpcClientEventStream};
/// Inject the overlay DLL (arch-picked) and return the module handle, without
/// opening IPC — for callers that hand the pipe name to another process.
pub use crate::injector::inject as inject_overlay_module;
pub use paths::{
    overlay_dll_marker, overlay_dll_paths, overlay_dll_ref, OverlayDllPaths, OVERLAY_DLL_X64,
    METRICS_DLL_NAME,
};

/// Paths to overlay DLLs for different architectures.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayDll<'a> {
    /// Path to DLL to be used for x64 applications.
    pub x64: Option<&'a Path>,

    /// Path to DLL to be used for x86 applications.
    pub x86: Option<&'a Path>,

    /// Path to DLL to be used for ARM64 applications.
    pub arm64: Option<&'a Path>,
}

/// Inject overlay DLL into target process and create IPC connection.
/// * If you didn't supply DLL path for the target architecture, it will return an error.
/// * If injection or IPC connection fails, it will return an error.
/// * If timeout is `None`, it may wait indefinitely.
pub async fn inject(
    pid: u32,
    dll: OverlayDll<'_>,
    timeout: Option<Duration>,
) -> anyhow::Result<(IpcClientConn, IpcClientEventStream, String)> {
    let module_handle =
        injector::inject(pid, dll, timeout).context("failed to inject overlay DLL")?;
    let ipc_addr = create_ipc_addr(pid, module_handle);

    let connect = IpcClientConn::new(ClientOptions::new().open(&ipc_addr)?);
    let timeout = sleep(timeout.unwrap_or(Duration::MAX));
    let conn = select! {
        res = connect => res?,
        _ = timeout => bail!("ipc client wait timeout"),
    };

    Ok((conn.0, conn.1, ipc_addr))
}

/// Connect to an existing overlay IPC named pipe (no injection).
/// * If timeout is `None`, it may wait indefinitely.
/// * Retries `CreateFile` until the pipe exists (DLL may bind shell + cef pipes
///   a moment after inject).
pub async fn connect_pipe(
    ipc_addr: impl AsRef<OsStr>,
    timeout: Option<Duration>,
) -> anyhow::Result<(IpcClientConn, IpcClientEventStream)> {
    let addr = ipc_addr.as_ref();
    let deadline = timeout.map(|d| tokio::time::Instant::now() + d);
    let mut last_err = None;

    loop {
        match ClientOptions::new().open(addr) {
            Ok(client) => {
                let remaining = deadline.map(|d| d.saturating_duration_since(tokio::time::Instant::now()));
                let connect = IpcClientConn::new(client);
                if let Some(rem) = remaining {
                    return select! {
                        res = connect => res.context("ipc connect_pipe failed"),
                        _ = sleep(rem) => bail!("ipc connect_pipe timeout waiting handshake on {}", addr.to_string_lossy()),
                    };
                }
                return connect.await.context("ipc connect_pipe failed");
            }
            Err(err) => {
                last_err = Some(err.to_string());
                if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
        }
    }

    bail!(
        "ipc connect_pipe: pipe not found — {} — last: {}",
        addr.to_string_lossy(),
        last_err.as_deref().unwrap_or("unknown")
    );
}

/// Inject an arbitrary DLL into the target process using the same mechanism as overlay inject
/// (`NtOpenProcess` + `RtlCreateUserThread` + `LoadLibraryW`), with arch detection.
pub fn inject_dll(
    pid: u32,
    dll_path: &Path,
    timeout: Option<Duration>,
) -> anyhow::Result<u32> {
    injector::inject(
        pid,
        OverlayDll {
            x64: Some(dll_path),
            x86: Some(dll_path),
            arm64: Some(dll_path),
        },
        timeout,
    )
}
