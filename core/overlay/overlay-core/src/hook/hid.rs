//! hid.dll consume (Steam `sub_1800BA5C0`).
//!
//! `GetModuleHandle` only. Missing DLL is success. Do **not** clone Steam’s
//! `pcbData==10462` virtual HID. Interactive: `HidP_GetData` / `GetUsages` /
//! `GetUsageValue` succeed with empty/zero count. Other listed HidD/HidP →
//! original.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::warn;
use windows::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{PCSTR, s},
};

use crate::backend::Backends;

const HIDP_STATUS_SUCCESS: i32 = 0x0011_0000;

type HidD1Fn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> i32;
type HidDFreeFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> i32;
type HidDStrFn =
    unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, u32) -> i32;
type HidPCapsFn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> i32;
type HidPButtonCapsFn =
    unsafe extern "system" fn(i32, *mut core::ffi::c_void, *mut u16, *mut core::ffi::c_void) -> i32;
type HidPMaxFn = unsafe extern "system" fn(i32, *mut core::ffi::c_void) -> u32;
type HidPGetDataFn = unsafe extern "system" fn(
    i32,
    *mut core::ffi::c_void,
    *mut u32,
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
) -> i32;
type HidPGetUsagesFn = unsafe extern "system" fn(
    i32,
    u16,
    u16,
    *mut u16,
    *mut u32,
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
) -> i32;
type HidPGetUsageValueFn = unsafe extern "system" fn(
    i32,
    u16,
    u16,
    u16,
    *mut u32,
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
) -> i32;

struct Hooks {
    attrs: Option<DetourHook<HidD1Fn>>,
    preparsed: Option<DetourHook<HidD1Fn>>,
    free: Option<DetourHook<HidDFreeFn>>,
    product: Option<DetourHook<HidDStrFn>>,
    caps: Option<DetourHook<HidPCapsFn>>,
    button_caps: Option<DetourHook<HidPButtonCapsFn>>,
    value_caps: Option<DetourHook<HidPButtonCapsFn>>,
    max_data: Option<DetourHook<HidPMaxFn>>,
    get_data: Option<DetourHook<HidPGetDataFn>>,
    get_usages: Option<DetourHook<HidPGetUsagesFn>>,
    get_usage_value: Option<DetourHook<HidPGetUsageValueFn>>,
}

impl Hooks {
    fn empty() -> Self {
        Self {
            attrs: None,
            preparsed: None,
            free: None,
            product: None,
            caps: None,
            button_caps: None,
            value_caps: None,
            max_data: None,
            get_data: None,
            get_usages: None,
            get_usage_value: None,
        }
    }

    fn any_attached(&self) -> bool {
        self.get_data.is_some() || self.get_usages.is_some() || self.get_usage_value.is_some()
    }
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("hid.dll")) }.ok()
}

fn empty_count(len: *mut u32) {
    if !len.is_null() {
        unsafe { *len = 0 };
    }
}

fn empty_value(val: *mut u32) {
    if !val.is_null() {
        unsafe { *val = 0 };
    }
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

unsafe extern "system" fn hooked_attrs(
    h: *mut core::ffi::c_void,
    attrs: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().attrs.as_ref().unwrap().original_fn();
    unsafe { orig(h, attrs) }
}

unsafe extern "system" fn hooked_preparsed(
    h: *mut core::ffi::c_void,
    data: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().preparsed.as_ref().unwrap().original_fn();
    unsafe { orig(h, data) }
}

unsafe extern "system" fn hooked_free(data: *mut core::ffi::c_void) -> i32 {
    let orig = HOOKS.wait().free.as_ref().unwrap().original_fn();
    unsafe { orig(data) }
}

unsafe extern "system" fn hooked_product(
    h: *mut core::ffi::c_void,
    buf: *mut core::ffi::c_void,
    len: u32,
) -> i32 {
    let orig = HOOKS.wait().product.as_ref().unwrap().original_fn();
    unsafe { orig(h, buf, len) }
}

unsafe extern "system" fn hooked_caps(
    data: *mut core::ffi::c_void,
    caps: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().caps.as_ref().unwrap().original_fn();
    unsafe { orig(data, caps) }
}

unsafe extern "system" fn hooked_button_caps(
    ty: i32,
    caps: *mut core::ffi::c_void,
    len: *mut u16,
    data: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().button_caps.as_ref().unwrap().original_fn();
    unsafe { orig(ty, caps, len, data) }
}

unsafe extern "system" fn hooked_value_caps(
    ty: i32,
    caps: *mut core::ffi::c_void,
    len: *mut u16,
    data: *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().value_caps.as_ref().unwrap().original_fn();
    unsafe { orig(ty, caps, len, data) }
}

unsafe extern "system" fn hooked_max_data(ty: i32, data: *mut core::ffi::c_void) -> u32 {
    let orig = HOOKS.wait().max_data.as_ref().unwrap().original_fn();
    unsafe { orig(ty, data) }
}

unsafe extern "system" fn hooked_get_data(
    ty: i32,
    list: *mut core::ffi::c_void,
    len: *mut u32,
    data: *mut core::ffi::c_void,
    report: *mut core::ffi::c_void,
    report_len: u32,
) -> i32 {
    if is_interactive() {
        empty_count(len);
        return HIDP_STATUS_SUCCESS;
    }
    let orig = HOOKS.wait().get_data.as_ref().unwrap().original_fn();
    unsafe { orig(ty, list, len, data, report, report_len) }
}

unsafe extern "system" fn hooked_get_usages(
    ty: i32,
    page: u16,
    link: u16,
    list: *mut u16,
    len: *mut u32,
    data: *mut core::ffi::c_void,
    report: *mut core::ffi::c_void,
    report_len: u32,
) -> i32 {
    if is_interactive() {
        empty_count(len);
        return HIDP_STATUS_SUCCESS;
    }
    let orig = HOOKS.wait().get_usages.as_ref().unwrap().original_fn();
    unsafe { orig(ty, page, link, list, len, data, report, report_len) }
}

unsafe extern "system" fn hooked_get_usage_value(
    ty: i32,
    page: u16,
    link: u16,
    usage: u16,
    value: *mut u32,
    data: *mut core::ffi::c_void,
    report: *mut core::ffi::c_void,
    report_len: u32,
) -> i32 {
    if is_interactive() {
        empty_value(value);
        return HIDP_STATUS_SUCCESS;
    }
    let orig = HOOKS.wait().get_usage_value.as_ref().unwrap().original_fn();
    unsafe { orig(ty, page, link, usage, value, data, report, report_len) }
}

pub(super) fn hook() {
    let Some(module) = module() else {
        let _ = HOOKS.set(Hooks::empty());
        return;
    };
    let _ = HOOKS.set(Hooks {
        attrs: attach(
            module,
            s!("HidD_GetAttributes"),
            hooked_attrs,
            "HidD_GetAttributes",
        ),
        preparsed: attach(
            module,
            s!("HidD_GetPreparsedData"),
            hooked_preparsed,
            "HidD_GetPreparsedData",
        ),
        free: attach(
            module,
            s!("HidD_FreePreparsedData"),
            hooked_free,
            "HidD_FreePreparsedData",
        ),
        product: attach(
            module,
            s!("HidD_GetProductString"),
            hooked_product,
            "HidD_GetProductString",
        ),
        caps: attach(module, s!("HidP_GetCaps"), hooked_caps, "HidP_GetCaps"),
        button_caps: attach(
            module,
            s!("HidP_GetButtonCaps"),
            hooked_button_caps,
            "HidP_GetButtonCaps",
        ),
        value_caps: attach(
            module,
            s!("HidP_GetValueCaps"),
            hooked_value_caps,
            "HidP_GetValueCaps",
        ),
        max_data: attach(
            module,
            s!("HidP_MaxDataListLength"),
            hooked_max_data,
            "HidP_MaxDataListLength",
        ),
        get_data: attach(module, s!("HidP_GetData"), hooked_get_data, "HidP_GetData"),
        get_usages: attach(
            module,
            s!("HidP_GetUsages"),
            hooked_get_usages,
            "HidP_GetUsages",
        ),
        get_usage_value: attach(
            module,
            s!("HidP_GetUsageValue"),
            hooked_get_usage_value,
            "HidP_GetUsageValue",
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_hid_is_ok() {
        if module().is_none() {
            hook();
            assert!(!HOOKS.get().expect("empty set").any_attached());
        }
    }

    #[test]
    fn interactive_readings_are_empty_success() {
        let mut n = 7u32;
        empty_count(&mut n);
        assert_eq!(n, 0);
        let mut v = 99u32;
        empty_value(&mut v);
        assert_eq!(v, 0);
        assert_eq!(HIDP_STATUS_SUCCESS, 0x0011_0000);
    }
}
