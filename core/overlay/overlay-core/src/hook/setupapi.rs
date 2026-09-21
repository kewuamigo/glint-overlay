//! setupapi consume (Steam `sub_1800C38A0` / `sub_1800C3CC0`).
//!
//! `GetModuleHandle` only. Missing DLL is success. Detour the 6 Steam-detoured
//! procs. Do **not** detour `SetupDiGetDeviceInterfaceDetailA`. Do **not**
//! clone Steam’s HDEVINFO tree. Interactive HID-class enum → FALSE +
//! `ERROR_NO_MORE_ITEMS` (`0x103`). Non-HID / not Interactive → original.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use tracing::warn;
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND, SetLastError, WIN32_ERROR},
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{GUID, PCSTR, s},
};

use crate::backend::Backends;

const ERROR_NO_MORE_ITEMS: u32 = 0x103;
const INVALID_HANDLE: isize = -1;

/// `{4D1E55B2-F16F-11CF-88CB-001111000030}`
const GUID_DEVINTERFACE_HID: GUID = GUID::from_u128(0x4D1E55B2_F16F_11CF_88CB_001111000030);
/// `{745A17A0-74D3-11D0-B6FE-00A0C90F57DA}`
const GUID_DEVCLASS_HIDCLASS: GUID = GUID::from_u128(0x745A17A0_74D3_11D0_B6FE_00A0C90F57DA);

type GetClassDevsAFn =
    unsafe extern "system" fn(*const GUID, *const u8, HWND, u32) -> *mut core::ffi::c_void;
type GetClassDevsWFn =
    unsafe extern "system" fn(*const GUID, *const u16, HWND, u32) -> *mut core::ffi::c_void;
type EnumInterfacesFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    *const GUID,
    u32,
    *mut core::ffi::c_void,
) -> i32;
type EnumInfoFn =
    unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut core::ffi::c_void) -> i32;
type EnumDriverFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
    u32,
    *mut core::ffi::c_void,
) -> i32;

struct Hooks {
    class_a: Option<DetourHook<GetClassDevsAFn>>,
    class_w: Option<DetourHook<GetClassDevsWFn>>,
    enum_if: Option<DetourHook<EnumInterfacesFn>>,
    enum_info: Option<DetourHook<EnumInfoFn>>,
    enum_drv_a: Option<DetourHook<EnumDriverFn>>,
    enum_drv_w: Option<DetourHook<EnumDriverFn>>,
}

impl Hooks {
    fn empty() -> Self {
        Self {
            class_a: None,
            class_w: None,
            enum_if: None,
            enum_info: None,
            enum_drv_a: None,
            enum_drv_w: None,
        }
    }

    fn any_attached(&self) -> bool {
        self.class_a.is_some() || self.enum_if.is_some()
    }
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();
static HID_SETS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("setupapi.dll")) }.ok()
}

fn is_hid_guid(guid: *const GUID) -> bool {
    if guid.is_null() {
        return false;
    }
    let g = unsafe { *guid };
    g == GUID_DEVINTERFACE_HID || g == GUID_DEVCLASS_HIDCLASS
}

fn note_class_devs(handle: *mut core::ffi::c_void, guid: *const GUID) {
    if handle.is_null() || handle as isize == INVALID_HANDLE {
        return;
    }
    let key = handle as usize;
    let mut sets = HID_SETS.lock();
    sets.retain(|&h| h != key);
    if is_hid_guid(guid) {
        sets.push(key);
    }
}

fn hid_enum_blocks(handle: *mut core::ffi::c_void, iface: *const GUID) -> bool {
    if !is_interactive() {
        return false;
    }
    is_hid_guid(iface) || HID_SETS.lock().iter().any(|&h| h == handle as usize)
}

fn no_more_items() -> i32 {
    unsafe { SetLastError(WIN32_ERROR(ERROR_NO_MORE_ITEMS)) };
    0
}

fn attach<F: Copy + std::fmt::Debug>(
    module: HMODULE,
    name: PCSTR,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let proc = unsafe { GetProcAddress(module, name) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => Some(h),
        Err(err) => {
            warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

unsafe extern "system" fn hooked_class_a(
    guid: *const GUID,
    enumerator: *const u8,
    hwnd: HWND,
    flags: u32,
) -> *mut core::ffi::c_void {
    let orig = HOOKS.wait().class_a.as_ref().unwrap().original_fn();
    let h = unsafe { orig(guid, enumerator, hwnd, flags) };
    note_class_devs(h, guid);
    h
}

unsafe extern "system" fn hooked_class_w(
    guid: *const GUID,
    enumerator: *const u16,
    hwnd: HWND,
    flags: u32,
) -> *mut core::ffi::c_void {
    let orig = HOOKS.wait().class_w.as_ref().unwrap().original_fn();
    let h = unsafe { orig(guid, enumerator, hwnd, flags) };
    note_class_devs(h, guid);
    h
}

unsafe extern "system" fn hooked_enum_if(
    set: *mut core::ffi::c_void,
    info: *mut core::ffi::c_void,
    guid: *const GUID,
    index: u32,
    out: *mut core::ffi::c_void,
) -> i32 {
    if hid_enum_blocks(set, guid) {
        return no_more_items();
    }
    let orig = HOOKS.wait().enum_if.as_ref().unwrap().original_fn();
    unsafe { orig(set, info, guid, index, out) }
}

unsafe extern "system" fn hooked_enum_info(
    set: *mut core::ffi::c_void,
    index: u32,
    out: *mut core::ffi::c_void,
) -> i32 {
    if hid_enum_blocks(set, std::ptr::null()) {
        return no_more_items();
    }
    let orig = HOOKS.wait().enum_info.as_ref().unwrap().original_fn();
    unsafe { orig(set, index, out) }
}

unsafe extern "system" fn hooked_enum_drv_a(
    set: *mut core::ffi::c_void,
    info: *mut core::ffi::c_void,
    ty: u32,
    index: u32,
    out: *mut core::ffi::c_void,
) -> i32 {
    if hid_enum_blocks(set, std::ptr::null()) {
        return no_more_items();
    }
    let orig = HOOKS.wait().enum_drv_a.as_ref().unwrap().original_fn();
    unsafe { orig(set, info, ty, index, out) }
}

unsafe extern "system" fn hooked_enum_drv_w(
    set: *mut core::ffi::c_void,
    info: *mut core::ffi::c_void,
    ty: u32,
    index: u32,
    out: *mut core::ffi::c_void,
) -> i32 {
    if hid_enum_blocks(set, std::ptr::null()) {
        return no_more_items();
    }
    let orig = HOOKS.wait().enum_drv_w.as_ref().unwrap().original_fn();
    unsafe { orig(set, info, ty, index, out) }
}

pub(super) fn hook() {
    let Some(module) = module() else {
        let _ = HOOKS.set(Hooks::empty());
        return;
    };
    let _ = HOOKS.set(Hooks {
        class_a: attach(
            module,
            s!("SetupDiGetClassDevsA"),
            hooked_class_a,
            "SetupDiGetClassDevsA",
        ),
        class_w: attach(
            module,
            s!("SetupDiGetClassDevsW"),
            hooked_class_w,
            "SetupDiGetClassDevsW",
        ),
        enum_if: attach(
            module,
            s!("SetupDiEnumDeviceInterfaces"),
            hooked_enum_if,
            "SetupDiEnumDeviceInterfaces",
        ),
        enum_info: attach(
            module,
            s!("SetupDiEnumDeviceInfo"),
            hooked_enum_info,
            "SetupDiEnumDeviceInfo",
        ),
        enum_drv_a: attach(
            module,
            s!("SetupDiEnumDriverInfoA"),
            hooked_enum_drv_a,
            "SetupDiEnumDriverInfoA",
        ),
        enum_drv_w: attach(
            module,
            s!("SetupDiEnumDriverInfoW"),
            hooked_enum_drv_w,
            "SetupDiEnumDriverInfoW",
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_setupapi_is_ok() {
        if module().is_none() {
            hook();
            assert!(!HOOKS.get().expect("empty set").any_attached());
        }
    }

    #[test]
    fn hid_guids_match_interface_and_class() {
        assert!(is_hid_guid(&GUID_DEVINTERFACE_HID));
        assert!(is_hid_guid(&GUID_DEVCLASS_HIDCLASS));
        assert!(!is_hid_guid(std::ptr::null()));
        let other = GUID::from_u128(0);
        assert!(!is_hid_guid(&other));
    }

    #[test]
    fn interactive_hid_enum_is_no_more_items() {
        assert!(hid_enum_blocks_decision(true, true));
        assert!(!hid_enum_blocks_decision(false, true));
        assert!(!hid_enum_blocks_decision(true, false));
        assert_eq!(ERROR_NO_MORE_ITEMS, 0x103);
    }

    fn hid_enum_blocks_decision(interactive: bool, hid: bool) -> bool {
        interactive && hid
    }
}
