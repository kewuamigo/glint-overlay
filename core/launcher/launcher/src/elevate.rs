use anyhow::{Context, Result};

#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut return_length = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// Re-launch this executable elevated with the given args (Windows UAC prompt).
/// Uses ShellExecuteEx runas — never powershell.exe / cmd.exe.
#[cfg(windows)]
pub fn run_elevated(args: &[String]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_NORMAL;

    let exe = std::env::current_exe().context("current_exe failed")?;
    let exe_wide: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let params = args
        .iter()
        .map(|a| {
            if a.is_empty() {
                "\"\"".to_string()
            } else if a.contains([' ', '\t', '"']) {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let params_wide: Vec<u16> = params.encode_utf16().chain(std::iter::once(0)).collect();

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(exe_wide.as_ptr()),
        lpParameters: if params.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(params_wide.as_ptr())
        },
        nShow: SW_NORMAL.0 as i32,
        ..Default::default()
    };

    unsafe {
        ShellExecuteExW(&mut info).context("ShellExecuteEx runas failed")?;
        if !info.hProcess.is_invalid() {
            let wait = WaitForSingleObject(info.hProcess, u32::MAX);
            let mut exit_code = 0u32;
            let got_exit = GetExitCodeProcess(info.hProcess, &mut exit_code).is_ok();
            let _ = CloseHandle(info.hProcess);
            if wait != WAIT_OBJECT_0 {
                anyhow::bail!("elevated launcher wait failed ({wait:?})");
            }
            if !got_exit {
                anyhow::bail!("elevated launcher exit code unavailable");
            }
            if exit_code != 0 {
                anyhow::bail!("elevated launcher exited with {exit_code}");
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run_elevated(_args: &[String]) -> Result<()> {
    anyhow::bail!("elevation is only supported on Windows");
}
