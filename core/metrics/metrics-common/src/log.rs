//! Append-only debug log files under `%TEMP%` for injected DLL diagnostics.

use std::io::Write;

use windows::Win32::System::Threading::GetCurrentProcessId;

/// Append a line to `{tag}-{pid}.log` in the system temp directory.
pub fn debug_log(tag: &str, message: &str) {
    let pid = unsafe { GetCurrentProcessId() };
    let path = std::env::temp_dir().join(format!("{tag}-{pid}.log"));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{message}");
    }
}
