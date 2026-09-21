//! Injector module for injecting overlay DLL into target process.
//!
//! Uses the most typical DLL injection method of creating a remote thread that requires least permissions.

use core::{mem, time::Duration};
use std::{
    ffi::OsStr,
    fs,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use goblin::pe::PE;
use ntapi::{
    ntapi_base::CLIENT_ID,
    ntmmapi::{NtAllocateVirtualMemory, NtFreeVirtualMemory, NtWriteVirtualMemory},
    ntpsapi::NtOpenProcess,
    ntrtl::{PUSER_THREAD_START_ROUTINE, RtlCreateUserThread},
};
use scopeguard::defer;
use windows::{
    Wdk::Foundation::OBJECT_ATTRIBUTES,
    Win32::{
        Foundation::{
            CloseHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS, HMODULE,
            MAX_PATH, NTSTATUS, SetHandleInformation, WAIT_TIMEOUT,
        },
        System::{
            Diagnostics::Debug::{
                CONTEXT, CONTEXT_CONTROL_AMD64, FlushInstructionCache, GetThreadContext,
                SetThreadContext, WriteProcessMemory,
            },
            Memory::{
                MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
                PAGE_PROTECTION_FLAGS, PAGE_READONLY, PAGE_READWRITE, VirtualAllocEx,
                VirtualProtectEx,
            },
            ProcessStatus::{EnumProcessModulesEx, GetModuleBaseNameA, LIST_MODULES_ALL},
            SystemInformation::{
                GetSystemWow64DirectoryA, IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64,
                IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_UNKNOWN,
            },
            Threading::{
                CreateProcessW, GetCurrentProcess, GetExitCodeThread, IsWow64Process2,
                PROCESS_CREATE_THREAD, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
                PROCESS_VM_WRITE, STARTUPINFOW, WaitForSingleObject,
            },
        },
    },
    core::{PCSTR, PCWSTR, PWSTR},
};

windows::core::link!(
    "kernel32.dll" "system" fn LoadLibraryW(lplibfilename: PCSTR) -> HMODULE
);

use crate::{OverlayDll, overlay_dll_paths};

type CreateProcessWFn = unsafe extern "system" fn(
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

static CREATE_PROCESS_W_ORIGINAL: std::sync::atomic::AtomicPtr<()> =
    std::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Steam `qword_18017D290` analog. Overlay-dll registers the CreateProcessW trampoline.
pub fn set_create_process_w_original(f: Option<CreateProcessWFn>) {
    let ptr = f.map(|f| f as *mut ()).unwrap_or(core::ptr::null_mut());
    CREATE_PROCESS_W_ORIGINAL.store(ptr, std::sync::atomic::Ordering::Release);
}

fn create_process_w_dispatch() -> CreateProcessWFn {
    let ptr = CREATE_PROCESS_W_ORIGINAL.load(std::sync::atomic::Ordering::Acquire);
    if ptr.is_null() {
        create_process_w_kernel32
    } else {
        unsafe { mem::transmute(ptr) }
    }
}

unsafe extern "system" fn create_process_w_kernel32(
    app: *const u16,
    cmd: *mut u16,
    _proc_attr: *const core::ffi::c_void,
    _thread_attr: *const core::ffi::c_void,
    inherit: i32,
    flags: u32,
    _env: *const core::ffi::c_void,
    dir: *const u16,
    startup: *const core::ffi::c_void,
    pi: *mut PROCESS_INFORMATION,
) -> i32 {
    let ok = unsafe {
        CreateProcessW(
            PCWSTR(app),
            if cmd.is_null() {
                None
            } else {
                Some(PWSTR(cmd))
            },
            None,
            None,
            inherit != 0,
            PROCESS_CREATION_FLAGS(flags),
            None,
            PCWSTR(dir),
            &*startup.cast(),
            &mut *pi,
        )
    };
    i32::from(ok.is_ok())
}

/// Steam `sub_180095250` 0x65-byte LoadLibraryW stub (path / LoadLibraryW / saved Rip at +0x4D).
const STEAM_X64_STUB_PREFIX: [u8; 0x4D] = [
    0x9C, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41,
    0x53, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0x0D,
    0x29, 0x00, 0x00, 0x00, 0xFF, 0x15, 0x2B, 0x00, 0x00, 0x00, 0x48, 0x83, 0xC4, 0x20, 0x41, 0x5F,
    0x41, 0x5E, 0x41, 0x5D, 0x41, 0x5C, 0x41, 0x5B, 0x41, 0x5A, 0x41, 0x59, 0x41, 0x58, 0x5F, 0x5E,
    0x5D, 0x5C, 0x5B, 0x5A, 0x59, 0x58, 0x9D, 0xFF, 0x25, 0x10, 0x00, 0x00, 0x00,
];

fn steam_x64_inject_stub(path_ptr: u64, load_library_w: u64, saved_rip: u64) -> [u8; 0x65] {
    let mut stub = [0u8; 0x65];
    stub[..0x4D].copy_from_slice(&STEAM_X64_STUB_PREFIX);
    stub[0x4D..0x55].copy_from_slice(&path_ptr.to_le_bytes());
    stub[0x55..0x5D].copy_from_slice(&load_library_w.to_le_bytes());
    stub[0x5D..0x65].copy_from_slice(&saved_rip.to_le_bytes());
    stub
}

/// Steam `sub_1800AB210`: helper is `bin\x86launcher.exe` next to the overlay DLL.
fn steam_wow64_helper_path(overlay_dll: &Path) -> PathBuf {
    overlay_dll
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("bin")
        .join("x86launcher.exe")
}

/// Steam cmdline `L"\"%ls\" -hproc %x -hthread %x -baseoverlayname %ls"` at `0x1801252c0`.
fn steam_wow64_helper_cmdline(
    helper: &Path,
    hproc: u32,
    hthread: u32,
    overlay_dll: &Path,
) -> String {
    format!(
        "\"{}\" -hproc {:x} -hthread {:x} -baseoverlayname {}",
        helper.display(),
        hproc,
        hthread,
        overlay_dll.display()
    )
}

/// Prefer `glint_overlay-x86.dll` beside the helper. Do not strip `*64.dll`.
fn steam_wow64_baseoverlay(overlay_dll: &Path) -> PathBuf {
    let dir = overlay_dll.parent().unwrap_or_else(|| Path::new("."));
    let x86 = overlay_dll_paths(dir).x86;
    if x86.is_file() { x86 } else { overlay_dll.to_path_buf() }
}

/// Inject overlay DLL into target process and returns the module handle of the injected DLL.
///
/// Note that returned module handle is truncated to u32 and may not point to the actual module handle.
pub fn inject(pid: u32, dll: OverlayDll, timeout: Option<Duration>) -> anyhow::Result<u32> {
    let mut handle = HANDLE(0 as _);
    unsafe {
        let mut attr = OBJECT_ATTRIBUTES {
            Length: mem::size_of::<OBJECT_ATTRIBUTES>() as _,
            ..Default::default()
        };

        // NtOpenProcess is more permissive
        NTSTATUS(NtOpenProcess(
            &mut handle as *mut _ as _,
            (PROCESS_QUERY_LIMITED_INFORMATION
                | PROCESS_CREATE_THREAD
                | PROCESS_VM_OPERATION
                | PROCESS_VM_READ
                | PROCESS_VM_WRITE)
                .0,
            &mut attr as *mut _ as _,
            &mut CLIENT_ID {
                UniqueProcess: pid as _,
                UniqueThread: 0 as _,
            },
        ))
        .ok()
        .context("cannot open process")?;
    };
    defer!(unsafe {
        _ = CloseHandle(handle);
    });

    let target_arch = get_process_arch(handle);
    let current_arch = get_process_arch(unsafe { GetCurrentProcess() });

    let path = match target_arch {
        IMAGE_FILE_MACHINE_AMD64 => dll.x64.context("x64 dll path is not provided")?,
        IMAGE_FILE_MACHINE_I386 => dll.x86.context("x86 dll path is not provided")?,
        IMAGE_FILE_MACHINE_ARM64 => dll.arm64.context("arm64 dll path is not provided")?,
        arch => bail!("Unsupported arch: {}", arch.0),
    };

    execute_remote_fn(
        handle,
        load_library_w_for(handle, target_arch, current_arch)
            .context("cannot find LoadLibraryW")?,
        path.as_os_str(),
        timeout,
    )
}

/// Inject while the primary thread is stopped. x64 uses Steam `sub_180095250`
/// (`VirtualAllocEx` + `Get/SetThreadContext` stub). Wow64 uses `sub_1800AB210`
/// (`bin\x86launcher.exe`). Does not `ResumeThread`.
pub fn inject_stopped(
    process: HANDLE,
    thread: HANDLE,
    dll: OverlayDll<'_>,
    timeout: Option<Duration>,
) -> anyhow::Result<()> {
    if process.is_invalid() || thread.is_invalid() {
        bail!("stopped inject needs process and thread handles");
    }

    let target_arch = get_process_arch(process);
    let current_arch = get_process_arch(unsafe { GetCurrentProcess() });
    let path = match target_arch {
        IMAGE_FILE_MACHINE_AMD64 => dll.x64.context("x64 dll path is not provided")?,
        IMAGE_FILE_MACHINE_I386 => dll.x86.context("x86 dll path is not provided")?,
        IMAGE_FILE_MACHINE_ARM64 => dll.arm64.context("arm64 dll path is not provided")?,
        arch => bail!("Unsupported arch: {}", arch.0),
    };

    if target_arch == IMAGE_FILE_MACHINE_AMD64 {
        return inject_x64_stopped(process, thread, path.as_os_str(), current_arch);
    }

    if target_arch == IMAGE_FILE_MACHINE_I386 {
        return inject_wow64_stopped(process, thread, path);
    }

    execute_remote_fn(
        process,
        load_library_w_for(process, target_arch, current_arch)
            .context("cannot find LoadLibraryW")?,
        path.as_os_str(),
        timeout,
    )
    .map(|_| ())
}

/// Get the architecture of the target process handle.
fn get_process_arch(handle: HANDLE) -> IMAGE_FILE_MACHINE {
    let mut native_output = IMAGE_FILE_MACHINE_UNKNOWN;
    let mut wow64_output = IMAGE_FILE_MACHINE_UNKNOWN;
    unsafe {
        _ = IsWow64Process2(handle, &mut wow64_output, Some(&mut native_output));
    }

    if wow64_output != IMAGE_FILE_MACHINE_UNKNOWN {
        wow64_output
    } else {
        native_output
    }
}

/// Get the address of LoadLibraryW in the target process.
fn load_library_w_for(
    process: HANDLE,
    target_arch: IMAGE_FILE_MACHINE,
    process_arch: IMAGE_FILE_MACHINE,
) -> anyhow::Result<usize> {
    if target_arch == process_arch {
        Ok(LoadLibraryW as *const () as usize)
    } else {
        match (process_arch, target_arch) {
            (IMAGE_FILE_MACHINE_I386, IMAGE_FILE_MACHINE_AMD64) => {
                bail!("cannot inject to x64 process from x86 process")
            }

            // wow64 x86
            (_, IMAGE_FILE_MACHINE_I386) => {
                let mut kernel32_path = unsafe {
                    let size = GetSystemWow64DirectoryA(None);
                    let mut buf = vec![0u8; size as _];
                    GetSystemWow64DirectoryA(Some(&mut buf));
                    // pop nul
                    buf.pop();
                    PathBuf::from(str::from_utf8(&buf)?)
                };
                kernel32_path.push("kernel32.dll");

                let data = fs::read(&kernel32_path)?;
                let pe = PE::parse(&data)?;
                let ex = pe
                    .exports
                    .iter()
                    .find(|ex| matches!(ex.name, Some("LoadLibraryW")))
                    .context("cannot find LoadLibraryW exports")?;

                let mut mod_list = vec![HMODULE::default(); 1024];
                let mut cb_size = 0;
                unsafe {
                    EnumProcessModulesEx(
                        process,
                        mod_list.as_mut_ptr(),
                        (mod_list.len() * mem::size_of::<HMODULE>()) as u32,
                        &mut cb_size,
                        LIST_MODULES_ALL,
                    )?;
                };
                mod_list.truncate(cb_size as usize / mem::size_of::<HMODULE>());

                let target_kernel32_base = {
                    let mut buf = [0_u8; MAX_PATH as usize + 1];

                    mod_list
                        .into_iter()
                        .find({
                            |module| unsafe {
                                let len = GetModuleBaseNameA(process, Some(*module), &mut buf);
                                str::from_utf8(&buf[..len as usize])
                                    .map(|path| path.eq_ignore_ascii_case("kernel32.dll"))
                                    .unwrap_or(false)
                            }
                        })
                        .context("cannot find kernel32.dll in target process")?
                };

                Ok(ex.rva + target_kernel32_base.0 as usize)
            }

            // x64 on arm64
            (IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_AMD64) => {
                Ok(LoadLibraryW as *const () as usize)
            }

            (current_arch, target_arch) => {
                bail!(
                    "Unsupported target arch: {}, current arch: {}",
                    target_arch.0,
                    current_arch.0
                );
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[repr(C, align(16))]
struct AlignedContext(CONTEXT);

#[cfg(target_arch = "x86_64")]
impl AlignedContext {
    fn new() -> Self {
        Self(CONTEXT::default())
    }
}

#[cfg(target_arch = "x86_64")]
fn inject_x64_stopped(
    process: HANDLE,
    thread: HANDLE,
    path: &OsStr,
    current_arch: IMAGE_FILE_MACHINE,
) -> anyhow::Result<()> {
    let encoded = path.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let encoded = bytemuck::cast_slice::<_, u8>(&encoded);

    unsafe {
        let path_remote = VirtualAllocEx(process, None, encoded.len(), MEM_COMMIT, PAGE_READWRITE);
        if path_remote.is_null() {
            bail!("VirtualAllocEx path failed");
        }
        WriteProcessMemory(
            process,
            path_remote,
            encoded.as_ptr().cast(),
            encoded.len(),
            None,
        )
        .context("WriteProcessMemory path")?;

        let stub_remote = VirtualAllocEx(
            process,
            None,
            0x65,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        );
        if stub_remote.is_null() {
            bail!("VirtualAllocEx stub failed");
        }

        let load_library_w = load_library_w_for(process, IMAGE_FILE_MACHINE_AMD64, current_arch)
            .context("cannot find LoadLibraryW")?;

        let mut ctx = AlignedContext::new();
        ctx.0.ContextFlags = CONTEXT_CONTROL_AMD64;
        GetThreadContext(thread, &mut ctx.0).context("GetThreadContext")?;

        let stub = steam_x64_inject_stub(path_remote as u64, load_library_w as u64, ctx.0.Rip);
        WriteProcessMemory(process, stub_remote, stub.as_ptr().cast(), stub.len(), None)
            .context("WriteProcessMemory stub")?;

        let mut old = PAGE_PROTECTION_FLAGS::default();
        VirtualProtectEx(process, path_remote, encoded.len(), PAGE_READONLY, &mut old)?;
        VirtualProtectEx(process, stub_remote, 0x65, PAGE_EXECUTE_READ, &mut old)?;
        FlushInstructionCache(process, Some(stub_remote), 0x65)?;

        ctx.0.Rip = stub_remote as u64;
        ctx.0.ContextFlags = CONTEXT_CONTROL_AMD64;
        SetThreadContext(thread, &ctx.0).context("SetThreadContext")?;
    }
    Ok(())
}

#[cfg(not(target_arch = "x86_64"))]
fn inject_x64_stopped(
    _process: HANDLE,
    _thread: HANDLE,
    _path: &OsStr,
    _current_arch: IMAGE_FILE_MACHINE,
) -> anyhow::Result<()> {
    bail!("x64 Get/SetThreadContext inject requires an x64 host");
}

/// Steam `sub_1800AB210` Wow64 path: spawn `bin\x86launcher.exe`, inherit handles, wait.
fn inject_wow64_stopped(process: HANDLE, thread: HANDLE, overlay_dll: &Path) -> anyhow::Result<()> {
    let helper = steam_wow64_helper_path(overlay_dll);
    if !helper.is_file() {
        bail!("Wow64 inject helper missing: {}", helper.display());
    }

    let baseoverlay = steam_wow64_baseoverlay(overlay_dll);
    let mut cmdline: Vec<u16> = steam_wow64_helper_cmdline(
        &helper,
        process.0 as usize as u32,
        thread.0 as usize as u32,
        &baseoverlay,
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();

    unsafe {
        let mut proc_flags = 0u32;
        let mut thread_flags = 0u32;
        _ = GetHandleInformation(process, &mut proc_flags);
        _ = GetHandleInformation(thread, &mut thread_flags);
        _ = SetHandleInformation(process, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT);
        _ = SetHandleInformation(thread, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT);
        defer!({
            let restore_proc = HANDLE_FLAGS(proc_flags & HANDLE_FLAG_INHERIT.0);
            let restore_thread = HANDLE_FLAGS(thread_flags & HANDLE_FLAG_INHERIT.0);
            _ = SetHandleInformation(process, HANDLE_FLAG_INHERIT.0, restore_proc);
            _ = SetHandleInformation(thread, HANDLE_FLAG_INHERIT.0, restore_thread);
        });

        let si = STARTUPINFOW {
            cb: mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let ok = create_process_w_dispatch()(
            core::ptr::null(),
            cmdline.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            core::ptr::null(),
            core::ptr::null(),
            (&raw const si).cast(),
            &mut pi,
        );
        if ok == 0 {
            bail!("CreateProcessW {}", helper.display());
        }

        WaitForSingleObject(pi.hProcess, u32::MAX);
        _ = CloseHandle(pi.hThread);
        _ = CloseHandle(pi.hProcess);
    }
    Ok(())
}

/// Execute a function to the target process by creating a remote thread.
fn execute_remote_fn(
    process: HANDLE,
    f: usize,
    param: &OsStr,
    timeout: Option<Duration>,
) -> anyhow::Result<u32> {
    let param_encoded = param.encode_wide().collect::<Vec<u16>>();
    let param_encoded = bytemuck::cast_slice::<_, u8>(&param_encoded);

    unsafe {
        let mut base_addr = 0_usize;
        let mut region_size = param_encoded.len();
        // allocate rw page
        NTSTATUS(NtAllocateVirtualMemory(
            process.0 as _,
            &raw mut base_addr as _,
            0,
            &mut region_size,
            MEM_COMMIT.0,
            PAGE_EXECUTE_READWRITE.0,
        ))
        .ok()?;
        // free memory on exit
        defer!({
            let mut base_addr = base_addr;
            _ = NtFreeVirtualMemory(
                process.0 as _,
                &raw mut base_addr as _,
                &mut 0_usize as *mut _,
                MEM_RELEASE.0,
            );
        });

        // write dll path
        NTSTATUS(NtWriteVirtualMemory(
            process.0 as _,
            base_addr as _,
            param_encoded.as_ptr() as _,
            param_encoded.len(),
            0 as _,
        ))
        .ok()?;

        let mut thread_handle: HANDLE = HANDLE::default();
        // create a user thread in the process and execute LoadLibraryW
        NTSTATUS(RtlCreateUserThread(
            process.0 as _,
            0 as _,
            0,
            0,
            0,
            0,
            mem::transmute::<usize, PUSER_THREAD_START_ROUTINE>(f),
            base_addr as _,
            &mut thread_handle as *mut _ as _,
            0 as _,
        ))
        .ok()?;
        // cleanup thread handle
        defer!({
            _ = CloseHandle(thread_handle);
        });

        // wait for overlay dll to start
        let res = WaitForSingleObject(
            thread_handle,
            timeout
                .map(|duration| duration.as_millis() as u32)
                .unwrap_or(u32::MAX),
        );
        if res == WAIT_TIMEOUT {
            bail!("remote thread wait timeout");
        }

        let mut module_handle = 0_u32;
        // Get loaded module handle
        GetExitCodeThread(thread_handle, &mut module_handle)?;
        if module_handle == 0 {
            bail!("failed to load overlay DLL");
        }

        Ok(module_handle)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        create_process_w_dispatch, create_process_w_kernel32, set_create_process_w_original,
        steam_wow64_baseoverlay, steam_wow64_helper_cmdline, steam_wow64_helper_path,
        steam_x64_inject_stub, CreateProcessWFn,
    };
    #[cfg(target_arch = "x86_64")]
    use super::AlignedContext;

    /// IDA `sub_180095250` 0x65-byte template prefix (qwords at +0x4D are runtime).
    const IDA_STUB_PREFIX: &[u8] = &[
        0x9C, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52,
        0x41, 0x53, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xEC, 0x20, 0x48,
        0x8B, 0x0D, 0x29, 0x00, 0x00, 0x00, 0xFF, 0x15, 0x2B, 0x00, 0x00, 0x00, 0x48, 0x83, 0xC4,
        0x20, 0x41, 0x5F, 0x41, 0x5E, 0x41, 0x5D, 0x41, 0x5C, 0x41, 0x5B, 0x41, 0x5A, 0x41, 0x59,
        0x41, 0x58, 0x5F, 0x5E, 0x5D, 0x5C, 0x5B, 0x5A, 0x59, 0x58, 0x9D, 0xFF, 0x25, 0x10, 0x00,
        0x00, 0x00,
    ];

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn aligned_context_is_16_byte_aligned() {
        let ctx = AlignedContext::new();
        assert_eq!(core::mem::align_of_val(&ctx) % 16, 0);
        assert_eq!((&raw const ctx as usize) % 16, 0);
    }

    #[test]
    fn steam_x64_inject_stub_matches_ida_prefix() {
        let stub = steam_x64_inject_stub(0, 0, 0);
        assert_eq!(stub.len(), 0x65);
        assert_eq!(&stub[..0x4D], IDA_STUB_PREFIX);
        assert_eq!(&stub[0x4D..], &[0u8; 24]);
    }

    #[test]
    fn steam_x64_inject_stub_qwords_little_endian() {
        let stub =
            steam_x64_inject_stub(0x0102030405060708, 0x1112131415161718, 0x2122232425262728);
        assert_eq!(&stub[0x4D..0x55], &0x0102030405060708u64.to_le_bytes());
        assert_eq!(&stub[0x55..0x5D], &0x1112131415161718u64.to_le_bytes());
        assert_eq!(&stub[0x5D..0x65], &0x2122232425262728u64.to_le_bytes());
    }

    #[test]
    fn steam_wow64_helper_path_joins_bin_x86launcher() {
        let overlay = Path::new(r"C:\overlay\glint_overlay-x86.dll");
        assert_eq!(
            steam_wow64_helper_path(overlay),
            PathBuf::from(r"C:\overlay\bin\x86launcher.exe")
        );
    }

    #[test]
    fn steam_wow64_helper_cmdline_matches_ida_format() {
        let helper = Path::new(r"C:\overlay\bin\x86launcher.exe");
        let overlay = Path::new(r"C:\overlay\glint_overlay-x86.dll");
        assert_eq!(
            steam_wow64_helper_cmdline(helper, 0x5c, 0x60, overlay),
            r#""C:\overlay\bin\x86launcher.exe" -hproc 5c -hthread 60 -baseoverlayname C:\overlay\glint_overlay-x86.dll"#
        );
    }

    unsafe extern "system" fn trampoline_stub(
        _app: *const u16,
        _cmd: *mut u16,
        _proc_attr: *const core::ffi::c_void,
        _thread_attr: *const core::ffi::c_void,
        _inherit: i32,
        _flags: u32,
        _env: *const core::ffi::c_void,
        _dir: *const u16,
        _startup: *const core::ffi::c_void,
        _pi: *mut windows::Win32::System::Threading::PROCESS_INFORMATION,
    ) -> i32 {
        7
    }

    #[test]
    fn wow64_create_process_w_uses_registered_trampoline() {
        set_create_process_w_original(Some(trampoline_stub));
        let f: CreateProcessWFn = create_process_w_dispatch();
        assert_eq!(f as *const () as usize, trampoline_stub as *const () as usize);
        set_create_process_w_original(None);
        let fallback: CreateProcessWFn = create_process_w_dispatch();
        assert_eq!(
            fallback as *const () as usize,
            create_process_w_kernel32 as *const () as usize
        );
    }

    #[test]
    fn steam_wow64_baseoverlay_uses_x86_when_present() {
        let dir = std::env::temp_dir().join("glint-wow64-baseoverlay-test");
        let _ = std::fs::create_dir_all(&dir);
        let x64 = dir.join("glint_overlay-x64.dll");
        let x86 = dir.join("glint_overlay-x86.dll");
        std::fs::write(&x86, []).unwrap();
        assert_eq!(steam_wow64_baseoverlay(&x64), x86);
        let _ = std::fs::remove_file(&x86);
        assert_eq!(steam_wow64_baseoverlay(&x64), x64);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
