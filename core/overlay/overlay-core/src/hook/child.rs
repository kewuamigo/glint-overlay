//! Child attach via CreateProcess / ShellExecuteEx (Steam `sub_1800AE490` / `sub_1800AB510`).
//!
//! CreateProcess: original first (`flags | 4`), inject, resume unless caller suspended.
//! ShellExecute `.exe` + empty/`open`: rewrite to CreateProcess flags=0 (inject via wrap).

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::{CloseHandle, GetLastError, HANDLE, HINSTANCE, SetLastError},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
            Threading::{
                CreateProcessA, CreateProcessW, GetCurrentProcessId, PROCESS_CREATION_FLAGS,
                PROCESS_INFORMATION, ResumeThread, STARTUPINFOA, STARTUPINFOW,
            },
        },
        UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOA, SHELLEXECUTEINFOW},
    },
    core::{PCSTR, PCWSTR, PSTR, PWSTR, s},
};

type CreateProcessAFn = unsafe extern "system" fn(
    *const u8,
    *mut u8,
    *const core::ffi::c_void,
    *const core::ffi::c_void,
    i32,
    u32,
    *const core::ffi::c_void,
    *const u8,
    *const core::ffi::c_void,
    *mut PROCESS_INFORMATION,
) -> i32;
pub type CreateProcessWFn = unsafe extern "system" fn(
    *const u16,
    *mut u16,
    *const core::ffi::c_void,
    *const core::ffi::c_void,
    i32,
    u32,
    *const core::ffi::c_void,
    *const u16,
    *const core::ffi::c_void,
    *mut PROCESS_INFORMATION,
) -> i32;
type ShellExecuteExAFn = unsafe extern "system" fn(*mut SHELLEXECUTEINFOA) -> i32;
type ShellExecuteExWFn = unsafe extern "system" fn(*mut SHELLEXECUTEINFOW) -> i32;

struct Hooks {
    create_process_a: Option<DetourHook<CreateProcessAFn>>,
    create_process_w: Option<DetourHook<CreateProcessWFn>>,
    shell_execute_ex_a: Option<DetourHook<ShellExecuteExAFn>>,
    shell_execute_ex_w: Option<DetourHook<ShellExecuteExWFn>>,
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();
static INJECT: OnceCell<fn(u32, HANDLE, HANDLE)> = OnceCell::new();

/// Register the overlay-dll inject callback. No-op if already set.
pub fn set_child_inject(inject: fn(u32, HANDLE, HANDLE)) {
    let _ = INJECT.set(inject);
}

/// Steam `qword_18017D290` analog — original CreateProcessW after the detour.
pub fn create_process_w_original() -> Option<CreateProcessWFn> {
    HOOKS
        .get()
        .and_then(|h| h.create_process_w.as_ref())
        .map(DetourHook::original_fn)
}

const STEAM_SHELL_HINSTANCE: isize = 1238;

fn steam_shell_verb_open_or_empty(verb: Option<&[u16]>) -> bool {
    match verb {
        None | Some([]) => true,
        Some(v) => v == [b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16],
    }
}

fn steam_shell_file_is_exe(file: &[u16]) -> bool {
    file.len() > 4 && file.ends_with(&[b'.' as u16, b'e' as u16, b'x' as u16, b'e' as u16])
}

fn steam_shell_should_create(verb: Option<&[u16]>, file: Option<&[u16]>) -> bool {
    steam_shell_verb_open_or_empty(verb) && file.is_some_and(steam_shell_file_is_exe)
}

fn steam_shell_needs_quotes(file: &[u16]) -> bool {
    file.iter().any(|&c| c == b' ' as u16 || c == b'\t' as u16)
}

fn steam_shell_cmdline(file: &[u16], params: Option<&[u16]>) -> Vec<u16> {
    let mut out = Vec::new();
    if steam_shell_needs_quotes(file) {
        out.push(b'"' as u16);
        out.extend_from_slice(file);
        out.push(b'"' as u16);
    } else {
        out.extend_from_slice(file);
    }
    if let Some(params) = params.filter(|p| !p.is_empty()) {
        out.push(b' ' as u16);
        out.extend_from_slice(params);
    }
    out.push(0);
    out
}

fn steam_shell_cmdline_a(file: &[u8], params: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    if file.iter().any(|&c| c == b' ' || c == b'\t') {
        out.push(b'"');
        out.extend_from_slice(file);
        out.push(b'"');
    } else {
        out.extend_from_slice(file);
    }
    if let Some(params) = params.filter(|p| !p.is_empty()) {
        out.push(b' ');
        out.extend_from_slice(params);
    }
    out.push(0);
    out
}

unsafe fn widez<'a>(p: *const u16) -> Option<&'a [u16]> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    Some(unsafe { std::slice::from_raw_parts(p, len) })
}

unsafe fn ascz<'a>(p: *const u8) -> Option<&'a [u8]> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    Some(unsafe { std::slice::from_raw_parts(p, len) })
}

fn u8_as_u16(bytes: &[u8]) -> Vec<u16> {
    bytes.iter().copied().map(u16::from).collect()
}

unsafe fn try_shell_create_w(info: *mut SHELLEXECUTEINFOW) -> bool {
    if info.is_null() {
        return false;
    }
    let file = unsafe { (*info).lpFile.0 };
    let verb = unsafe { widez((*info).lpVerb.0) };
    let file_s = unsafe { widez(file) };
    if !steam_shell_should_create(verb, file_s) {
        return false;
    }
    let Some(file_s) = file_s else {
        return false;
    };
    let params = unsafe { widez((*info).lpParameters.0) };
    let mut cmdline = steam_shell_cmdline(file_s, params);
    let dir = unsafe { widez((*info).lpDirectory.0) }.filter(|d| !d.is_empty());
    let dir_z = dir.map(|d| {
        let mut v = d.to_vec();
        v.push(0);
        v
    });
    let si = STARTUPINFOW {
        cb: mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessW(
            PCWSTR(file),
            Some(PWSTR(cmdline.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            dir_z
                .as_ref()
                .map(|d| PCWSTR(d.as_ptr()))
                .unwrap_or_else(PCWSTR::null),
            &si,
            &mut pi,
        )
    }
    .is_ok();
    if !ok {
        return false;
    }
    unsafe {
        _ = CloseHandle(pi.hThread);
        (*info).hInstApp = HINSTANCE(STEAM_SHELL_HINSTANCE as *mut _);
        if (*info).fMask & SEE_MASK_NOCLOSEPROCESS == Default::default() {
            _ = CloseHandle(pi.hProcess);
            (*info).hProcess = HANDLE::default();
        } else {
            (*info).hProcess = pi.hProcess;
        }
    }
    true
}

unsafe fn try_shell_create_a(info: *mut SHELLEXECUTEINFOA) -> bool {
    if info.is_null() {
        return false;
    }
    let file = unsafe { (*info).lpFile.0 };
    let verb_b = unsafe { ascz((*info).lpVerb.0) };
    let file_b = unsafe { ascz(file) };
    let verb = verb_b.as_ref().map(|v| u8_as_u16(v));
    let file_s = file_b.as_ref().map(|f| u8_as_u16(f));
    if !steam_shell_should_create(verb.as_deref(), file_s.as_deref()) {
        return false;
    }
    let params_b = unsafe { ascz((*info).lpParameters.0) };
    let mut cmdline_a = steam_shell_cmdline_a(file_b.unwrap_or(&[]), params_b);
    let dir_b = unsafe { ascz((*info).lpDirectory.0) }.filter(|d| !d.is_empty());
    let dir_z = dir_b.map(|d| {
        let mut v = d.to_vec();
        v.push(0);
        v
    });
    let si = STARTUPINFOA {
        cb: mem::size_of::<STARTUPINFOA>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessA(
            PCSTR(file),
            Some(PSTR(cmdline_a.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            dir_z
                .as_ref()
                .map(|d| PCSTR(d.as_ptr()))
                .unwrap_or_else(PCSTR::null),
            &si,
            &mut pi,
        )
    }
    .is_ok();
    if !ok {
        return false;
    }
    unsafe {
        _ = CloseHandle(pi.hThread);
        (*info).hInstApp = HINSTANCE(STEAM_SHELL_HINSTANCE as *mut _);
        if (*info).fMask & SEE_MASK_NOCLOSEPROCESS == Default::default() {
            _ = CloseHandle(pi.hProcess);
            (*info).hProcess = HANDLE::default();
        } else {
            (*info).hProcess = pi.hProcess;
        }
    }
    true
}

/// After original CreateProcess / ShellExecuteEx. `None` = do not inject.
fn observed_child_pid(create_ok: bool, child_pid: u32, self_pid: u32) -> Option<u32> {
    if create_ok && child_pid != 0 && child_pid != self_pid {
        Some(child_pid)
    } else {
        None
    }
}

fn child_from_pi(pi: *mut PROCESS_INFORMATION) -> (u32, HANDLE, HANDLE) {
    if pi.is_null() {
        (0, HANDLE::default(), HANDLE::default())
    } else {
        unsafe { ((*pi).dwProcessId, (*pi).hProcess, (*pi).hThread) }
    }
}

fn request_child_inject(create_ok: bool, child_pid: u32, process: HANDLE, thread: HANDLE) {
    let self_pid = unsafe { GetCurrentProcessId() };
    let Some(pid) = observed_child_pid(create_ok, child_pid, self_pid) else {
        return;
    };
    let Some(inject) = INJECT.get() else {
        return;
    };
    let last = unsafe { GetLastError() };
    inject(pid, process, thread);
    unsafe { SetLastError(last) };
}

/// Steam `a6 | 4` — `CREATE_SUSPENDED`.
const CREATE_SUSPENDED: u32 = 0x4;

fn steam_create_flags(original: u32) -> u32 {
    original | CREATE_SUSPENDED
}

fn should_resume_primary(original: u32) -> bool {
    original & CREATE_SUSPENDED == 0
}

fn maybe_resume_primary(create_ok: bool, original_flags: u32, pi: *mut PROCESS_INFORMATION) {
    if !create_ok || !should_resume_primary(original_flags) || pi.is_null() {
        return;
    }
    let thread = unsafe { (*pi).hThread };
    if thread.is_invalid() {
        return;
    }
    let last = unsafe { GetLastError() };
    let _ = unsafe { ResumeThread(thread) };
    unsafe { SetLastError(last) };
}

unsafe extern "system" fn hooked_create_process_a(
    app: *const u8,
    cmd: *mut u8,
    proc_attr: *const core::ffi::c_void,
    thread_attr: *const core::ffi::c_void,
    inherit: i32,
    flags: u32,
    env: *const core::ffi::c_void,
    dir: *const u8,
    startup: *const core::ffi::c_void,
    pi: *mut PROCESS_INFORMATION,
) -> i32 {
    let Some(hook) = HOOKS.wait().create_process_a.as_ref() else {
        return 0;
    };
    let ok = unsafe {
        hook.original_fn()(
            app,
            cmd,
            proc_attr,
            thread_attr,
            inherit,
            steam_create_flags(flags),
            env,
            dir,
            startup,
            pi,
        )
    };
    let (pid, process, thread) = child_from_pi(pi);
    request_child_inject(ok != 0, pid, process, thread);
    maybe_resume_primary(ok != 0, flags, pi);
    ok
}

unsafe extern "system" fn hooked_create_process_w(
    app: *const u16,
    cmd: *mut u16,
    proc_attr: *const core::ffi::c_void,
    thread_attr: *const core::ffi::c_void,
    inherit: i32,
    flags: u32,
    env: *const core::ffi::c_void,
    dir: *const u16,
    startup: *const core::ffi::c_void,
    pi: *mut PROCESS_INFORMATION,
) -> i32 {
    let Some(hook) = HOOKS.wait().create_process_w.as_ref() else {
        return 0;
    };
    let ok = unsafe {
        hook.original_fn()(
            app,
            cmd,
            proc_attr,
            thread_attr,
            inherit,
            steam_create_flags(flags),
            env,
            dir,
            startup,
            pi,
        )
    };
    let (pid, process, thread) = child_from_pi(pi);
    request_child_inject(ok != 0, pid, process, thread);
    maybe_resume_primary(ok != 0, flags, pi);
    ok
}

unsafe extern "system" fn hooked_shell_execute_ex_a(info: *mut SHELLEXECUTEINFOA) -> i32 {
    if unsafe { try_shell_create_a(info) } {
        return 1;
    }
    let Some(hook) = HOOKS.wait().shell_execute_ex_a.as_ref() else {
        return 0;
    };
    unsafe { hook.original_fn()(info) }
}

unsafe extern "system" fn hooked_shell_execute_ex_w(info: *mut SHELLEXECUTEINFOW) -> i32 {
    if unsafe { try_shell_create_w(info) } {
        return 1;
    }
    let Some(hook) = HOOKS.wait().shell_execute_ex_w.as_ref() else {
        return 0;
    };
    unsafe { hook.original_fn()(info) }
}

fn attach<F: Copy + std::fmt::Debug>(
    dll: PCSTR,
    name: PCSTR,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let module = unsafe { GetModuleHandleA(dll) }
        .ok()
        .or_else(|| unsafe { LoadLibraryA(dll) }.ok())?;
    let proc = unsafe { GetProcAddress(module, name) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => {
            info!("hooked {label}");
            Some(h)
        }
        Err(err) => {
            warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

pub(super) fn hook() {
    let _ = HOOKS.set(Hooks {
        create_process_a: attach(
            s!("kernel32.dll"),
            s!("CreateProcessA"),
            hooked_create_process_a,
            "CreateProcessA",
        ),
        create_process_w: attach(
            s!("kernel32.dll"),
            s!("CreateProcessW"),
            hooked_create_process_w,
            "CreateProcessW",
        ),
        shell_execute_ex_a: attach(
            s!("shell32.dll"),
            s!("ShellExecuteExA"),
            hooked_shell_execute_ex_a,
            "ShellExecuteExA",
        ),
        shell_execute_ex_w: attach(
            s!("shell32.dll"),
            s!("ShellExecuteExW"),
            hooked_shell_execute_ex_w,
            "ShellExecuteExW",
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_success_observes_child_pid() {
        assert_eq!(observed_child_pid(true, 4242, 7), Some(4242));
    }

    #[test]
    fn create_failure_does_not_inject() {
        assert_eq!(observed_child_pid(false, 4242, 7), None);
    }

    #[test]
    fn skip_self_pid_does_not_inject() {
        assert_eq!(observed_child_pid(true, 7, 7), None);
    }

    #[test]
    fn shell_success_observes_child_pid() {
        assert_eq!(observed_child_pid(true, 99, 1), Some(99));
    }

    #[test]
    fn shell_failure_does_not_inject() {
        assert_eq!(observed_child_pid(false, 99, 1), None);
    }

    #[test]
    fn zero_pid_does_not_inject() {
        assert_eq!(observed_child_pid(true, 0, 7), None);
    }

    #[test]
    fn steam_create_flags_ors_create_suspended() {
        assert_eq!(steam_create_flags(0), 4);
    }

    #[test]
    fn steam_create_flags_already_suspended_stays() {
        assert_eq!(steam_create_flags(4), 4);
    }

    #[test]
    fn steam_create_flags_keeps_other_bits() {
        assert_eq!(steam_create_flags(0x8), 0x8 | 4);
    }

    #[test]
    fn should_resume_primary_when_caller_did_not_suspend() {
        assert!(should_resume_primary(0));
    }

    #[test]
    fn should_resume_primary_false_when_caller_suspended() {
        assert!(!should_resume_primary(4));
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn steam_shell_open_exe_rewrites_to_create() {
        let verb = wide("open");
        let file = wide(r"C:\game\game.exe");
        assert!(steam_shell_should_create(Some(&verb), Some(&file)));
        assert!(steam_shell_should_create(None, Some(&file)));
        assert!(steam_shell_should_create(Some(&[]), Some(&file)));
    }

    #[test]
    fn steam_shell_non_exe_or_other_verb_stays_original() {
        let run = wide("runas");
        let file = wide(r"C:\game\game.exe");
        let txt = wide(r"C:\game\readme.txt");
        assert!(!steam_shell_should_create(Some(&run), Some(&file)));
        assert!(!steam_shell_should_create(None, Some(&txt)));
    }

    #[test]
    fn steam_shell_cmdline_quotes_spaces() {
        let file = wide(r"C:\Program Files\game.exe");
        let params = wide("-foo");
        let cmd = steam_shell_cmdline(&file, Some(&params));
        let s: String = String::from_utf16_lossy(&cmd[..cmd.len() - 1]);
        assert_eq!(s, r#""C:\Program Files\game.exe" -foo"#);
    }
}
