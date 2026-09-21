use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use anyhow::{Context, Result};
use glint_injector::{
    default_achievements_dll, default_metrics_dll, inject_achievements_dll,
    inject_achievements_dll_stopped, inject_metrics_dll, inject_overlay_cef_pipe, inject_stopped,
    overlay_cef_pipe, overlay_dll_paths, overlay_dll_ref,
    project_root, require_overlay_dll_dir, runtime_root,
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

/// Live-PID attach: RtlCreateUserThread inject, then CEF UI helper.
fn spawn_cef_ui_helper(pid: u32) -> Result<(Child, PathBuf)> {
    let dll_dir = require_overlay_dll_dir()?;
    let cef_pipe = inject_overlay_cef_pipe(pid, &dll_dir)?;
    spawn_cef_ui_with_pipe(pid, cef_pipe)
}

/// Overlay already in the process — do not inject again. Starts the CEF UI helper.
fn spawn_cef_ui_with_pipe(pid: u32, cef_pipe: String) -> Result<(Child, PathBuf)> {
    let root = runtime_root().or_else(project_root).context(
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

    if let Some(achievements) = default_achievements_dll() {
        if let Err(err) = inject_achievements_dll(pid, &achievements) {
            eprintln!("[glint] achievements DLL inject failed: {err:#}");
        }
    } else {
        eprintln!(
            "[glint] glint_achievements_native.dll not found — \
             in-game achievement hooks unavailable (build -p glint-achievements-native)"
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

/// Steam-like Glint launch: CreateProcess `CREATE_SUSPENDED` (0x4), 2.2
/// `inject_stopped`, `ResumeThread`, wait for the overlay module, then CEF
/// without a second overlay inject. `skip_overlay` creates the process only.
pub fn launch_game(
    exe: &Path,
    cwd: Option<&Path>,
    args: &[String],
    skip_overlay: bool,
) -> Result<(u32, Option<PathBuf>)> {
    #[cfg(not(windows))]
    {
        let _ = (exe, cwd, args, skip_overlay);
        anyhow::bail!("launch is only supported on Windows");
    }
    #[cfg(windows)]
    {
        windows_launch_game(exe, cwd, args, skip_overlay)
    }
}

#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x4;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x00000008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
#[cfg(windows)]
const STILL_ACTIVE: u32 = 259;
#[cfg(windows)]
const OVERLAY_WAIT: Duration = Duration::from_secs(30);

#[cfg(windows)]
fn launch_creation_flags(skip_overlay: bool) -> u32 {
    let mut flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    if !skip_overlay {
        flags |= CREATE_SUSPENDED;
    }
    flags
}

#[cfg(windows)]
fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".into();
    }
    if !arg
        .bytes()
        .any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'"'))
    {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0u32;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                for _ in 0..(slashes * 2 + 1) {
                    out.push('\\');
                }
                out.push('"');
                slashes = 0;
            }
            _ => {
                for _ in 0..slashes {
                    out.push('\\');
                }
                out.push(c);
                slashes = 0;
            }
        }
    }
    for _ in 0..(slashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

#[cfg(windows)]
fn windows_cmdline(exe: &Path, args: &[String]) -> String {
    let mut line = quote_windows_arg(&exe.to_string_lossy());
    for arg in args {
        line.push(' ');
        line.push_str(&quote_windows_arg(arg));
    }
    line
}

#[cfg(windows)]
fn windows_launch_game(
    exe: &Path,
    cwd: Option<&Path>,
    args: &[String],
    skip_overlay: bool,
) -> Result<(u32, Option<PathBuf>)> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
        TerminateProcess,
    };
    use windows::core::{PCWSTR, PWSTR};

    if !exe.is_file() {
        anyhow::bail!("executable not found: {}", exe.display());
    }
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| exe.parent().map(Path::to_path_buf));

    let exe_wide: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut cmd_wide: Vec<u16> = windows_cmdline(exe, args)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let cwd_wide: Option<Vec<u16>> = cwd
        .as_ref()
        .map(|p| p.as_os_str().encode_wide().chain(Some(0)).collect());

    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(exe_wide.as_ptr()),
            Some(PWSTR(cmd_wide.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(launch_creation_flags(skip_overlay)),
            None,
            cwd_wide
                .as_ref()
                .map(|w| PCWSTR(w.as_ptr()))
                .unwrap_or_else(PCWSTR::null),
            &si,
            &mut pi,
        )
        .with_context(|| format!("CreateProcessW failed for {}", exe.display()))?;
    }

    struct Handles {
        process: HANDLE,
        thread: HANDLE,
    }
    impl Drop for Handles {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.thread);
                let _ = CloseHandle(self.process);
            }
        }
    }
    let handles = Handles {
        process: pi.hProcess,
        thread: pi.hThread,
    };
    let pid = pi.dwProcessId;

    if skip_overlay {
        return Ok((pid, None));
    }

    let dll_dir = require_overlay_dll_dir()?;
    let paths = overlay_dll_paths(&dll_dir);
    if let Err(err) = inject_stopped(
        handles.process,
        handles.thread,
        overlay_dll_ref(&paths),
        Some(OVERLAY_WAIT),
    ) {
        unsafe {
            let _ = TerminateProcess(handles.process, 1);
        }
        return Err(err).context("inject_stopped failed");
    }

    if let Some(achievements) = default_achievements_dll() {
        if let Err(err) =
            inject_achievements_dll_stopped(handles.process, handles.thread, &achievements)
        {
            eprintln!("[glint] achievements DLL stopped-inject failed: {err:#}");
        }
    } else {
        eprintln!(
            "[glint] glint_achievements_native.dll not found — \
             in-game achievement hooks unavailable (build -p glint-achievements-native)"
        );
    }

    let prev = unsafe { ResumeThread(handles.thread) };
    if prev == u32::MAX {
        unsafe {
            let _ = TerminateProcess(handles.process, 1);
        }
        anyhow::bail!("ResumeThread failed");
    }

    let module_handle = wait_overlay_module(handles.process, OVERLAY_WAIT)?;
    let (_child, log_file) = spawn_cef_ui_with_pipe(pid, overlay_cef_pipe(pid, module_handle))?;
    Ok((pid, Some(log_file)))
}

#[cfg(windows)]
fn wait_overlay_module(
    process: windows::Win32::Foundation::HANDLE,
    timeout: Duration,
) -> Result<u32> {
    use std::time::Instant;
    use windows::Win32::System::Threading::GetExitCodeProcess;

    let deadline = Instant::now() + timeout;
    loop {
        if let Some(handle) = find_overlay_module(process) {
            return Ok(handle);
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(process, &mut code) }.is_ok() && code != STILL_ACTIVE {
            anyhow::bail!("game process exited before overlay DLL loaded ({code})");
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for overlay DLL after ResumeThread");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
fn find_overlay_module(process: windows::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows::Win32::Foundation::{HMODULE, MAX_PATH};
    use windows::Win32::System::ProcessStatus::{
        EnumProcessModulesEx, GetModuleBaseNameA, LIST_MODULES_ALL,
    };

    let mut mods = vec![HMODULE::default(); 1024];
    let mut cb = 0u32;
    unsafe {
        EnumProcessModulesEx(
            process,
            mods.as_mut_ptr(),
            (mods.len() * std::mem::size_of::<HMODULE>()) as u32,
            &mut cb,
            LIST_MODULES_ALL,
        )
        .ok()?;
    }
    mods.truncate(cb as usize / std::mem::size_of::<HMODULE>());
    let mut name = [0u8; MAX_PATH as usize + 1];
    for module in mods {
        let len = unsafe { GetModuleBaseNameA(process, Some(module), &mut name) } as usize;
        if len == 0 {
            continue;
        }
        let Ok(base) = std::str::from_utf8(&name[..len]) else {
            continue;
        };
        if base.len() >= 14 && base[..14].eq_ignore_ascii_case("glint_overlay-") {
            return Some(module.0 as usize as u32);
        }
    }
    None
}
