//! DirectInput 7/8 create + device consume / Shift+Tab (Steam D7).
//!
//! Always create the real interface first. Never a fake IDirectInput.

use std::{
    mem,
    sync::atomic::{AtomicBool, Ordering},
};

use dashmap::DashMap;
use glint_overlay_event::input::{Key, KeyInputState, KeyboardInput};
use glint_overlay_hook::DetourHook;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::{
    Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
    core::{GUID, PCSTR, s},
};

use crate::{backend::Backends, backend::window::input_event, event_sink::OverlayEventSink};

/// DIK_TAB
const DIK_TAB: u8 = 0x0F;
/// DIK_LSHIFT
const DIK_LSHIFT: u8 = 0x2A;
/// DIK_RSHIFT
const DIK_RSHIFT: u8 = 0x36;

const GUID_SYS_MOUSE: GUID = GUID::from_u128(0x6F1D2B60_D5A0_11CF_BFC7_444553540000);
const GUID_SYS_KEYBOARD: GUID = GUID::from_u128(0x6F1D2B61_D5A0_11CF_BFC7_444553540000);

const VT_CREATE_DEVICE: usize = 3;
const VT_GET_CAPS: usize = 3;
const VT_GET_STATE: usize = 9;
const VT_GET_DATA: usize = 10;

type DirectInput8CreateFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    u32,
    *const GUID,
    *mut *mut core::ffi::c_void,
    *mut core::ffi::c_void,
) -> i32;
type DirectInputCreateFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    u32,
    *mut *mut core::ffi::c_void,
    *mut core::ffi::c_void,
) -> i32;
type CreateDeviceFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *const GUID,
    *mut *mut core::ffi::c_void,
    *mut core::ffi::c_void,
) -> i32;
type GetDeviceStateFn =
    unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut core::ffi::c_void) -> i32;
type GetDeviceDataFn =
    unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut u8, *mut u32, u32) -> i32;
type GetCapsFn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut DiDevCaps) -> i32;

#[repr(C)]
struct DiDevCaps {
    dw_size: u32,
    dw_flags: u32,
    dw_dev_type: u32,
    _rest: [u32; 8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiDeviceKind {
    Keyboard,
    Mouse,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiHotkey {
    None,
    Unbuffered,
    Buffered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiObjectData {
    dw_ofs: u32,
    dw_data: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiKbOutcome {
    hotkey: DiHotkey,
}

struct DeviceRec {
    kind: DiDeviceKind,
    get_state: GetDeviceStateFn,
    get_data: GetDeviceDataFn,
}

struct CreateHooks {
    di8: Option<DetourHook<DirectInput8CreateFn>>,
    di_a: Option<DetourHook<DirectInputCreateFn>>,
    di_w: Option<DetourHook<DirectInputCreateFn>>,
}

struct DeviceHookSet {
    create: Vec<(usize, DetourHook<CreateDeviceFn>)>,
    state: Vec<(usize, DetourHook<GetDeviceStateFn>)>,
    data: Vec<(usize, DetourHook<GetDeviceDataFn>)>,
}

static CREATE_HOOKS: OnceCell<CreateHooks> = OnceCell::new();
static DEVICE_HOOKS: Mutex<DeviceHookSet> = Mutex::new(DeviceHookSet {
    create: Vec::new(),
    state: Vec::new(),
    data: Vec::new(),
});
static DEVICES: Lazy<DashMap<usize, DeviceRec>> = Lazy::new(DashMap::new);
static KB_SHADOW: Mutex<[u8; 256]> = Mutex::new([0; 256]);
static CHORD: AtomicBool = AtomicBool::new(false);

fn key_down(buf: &[u8], dik: u8) -> bool {
    buf.get(dik as usize).is_some_and(|&b| b & 0x80 != 0)
}

fn shift_down(buf: &[u8]) -> bool {
    key_down(buf, DIK_LSHIFT) || key_down(buf, DIK_RSHIFT)
}

fn chord(buf: &[u8]) -> bool {
    shift_down(buf) && key_down(buf, DIK_TAB)
}

fn apply_kb_state(buf: &mut [u8], interactive: bool) -> DiKbOutcome {
    let hotkey = if chord(buf) {
        DiHotkey::Unbuffered
    } else {
        DiHotkey::None
    };
    if shift_down(buf) {
        if let Some(tab) = buf.get_mut(DIK_TAB as usize) {
            *tab = 0;
        }
    }
    if interactive {
        buf.fill(0);
    }
    DiKbOutcome { hotkey }
}

fn apply_kb_data(
    shadow: &mut [u8; 256],
    data: &mut [DiObjectData],
    interactive: bool,
) -> DiKbOutcome {
    for ev in data.iter() {
        if let Some(slot) = shadow.get_mut(ev.dw_ofs as usize) {
            *slot = if ev.dw_data & 0x80 != 0 { 0x80 } else { 0 };
        }
    }
    let hotkey = if chord(shadow) {
        DiHotkey::Buffered
    } else {
        DiHotkey::None
    };
    if shift_down(shadow) {
        for ev in data.iter_mut() {
            if ev.dw_ofs == DIK_TAB as u32 {
                ev.dw_data = 0;
            }
        }
        shadow[DIK_TAB as usize] = 0;
    }
    if interactive {
        for ev in data.iter_mut() {
            ev.dw_data = 0;
        }
    }
    DiKbOutcome { hotkey }
}

fn apply_device_state(kind: DiDeviceKind, buf: &mut [u8], interactive: bool) -> DiKbOutcome {
    match kind {
        DiDeviceKind::Keyboard => apply_kb_state(buf, interactive),
        DiDeviceKind::Mouse => {
            if interactive {
                buf.fill(0);
            }
            DiKbOutcome {
                hotkey: DiHotkey::None,
            }
        }
        DiDeviceKind::Other => DiKbOutcome {
            hotkey: DiHotkey::None,
        },
    }
}

fn apply_device_data(
    kind: DiDeviceKind,
    shadow: &mut [u8; 256],
    data: &mut [DiObjectData],
    interactive: bool,
) -> DiKbOutcome {
    match kind {
        DiDeviceKind::Keyboard => apply_kb_data(shadow, data, interactive),
        DiDeviceKind::Mouse => {
            if interactive {
                for ev in data.iter_mut() {
                    ev.dw_data = 0;
                }
            }
            DiKbOutcome {
                hotkey: DiHotkey::None,
            }
        }
        DiDeviceKind::Other => DiKbOutcome {
            hotkey: DiHotkey::None,
        },
    }
}

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn note_hotkey(kind: DiHotkey) {
    let held = kind != DiHotkey::None;
    let was = CHORD.swap(held, Ordering::Relaxed);
    if !held || was {
        return;
    }
    match kind {
        DiHotkey::Unbuffered => info!("Detected hot-key via dinput8 (unbuffered)"),
        DiHotkey::Buffered => info!("Detected hot-key via dinput8 (buffered)"),
        DiHotkey::None => {}
    }
    emit_shift_tab();
}

fn emit_shift_tab() {
    let hwnd = Backends::iter().next().map(|b| b.id).unwrap_or(0);
    for (vk, state) in [
        (0x10u8, KeyInputState::Pressed),
        (0x09, KeyInputState::Pressed),
        (0x09, KeyInputState::Released),
        (0x10, KeyInputState::Released),
    ] {
        if let Some(key) = Key::new(vk, false) {
            OverlayEventSink::emit(input_event::keyboard_overlay_event(
                hwnd,
                KeyboardInput::Key { key, state },
            ));
        }
    }
}

fn fn_addr<F: Copy>(func: F) -> usize {
    unsafe { mem::transmute_copy(&func) }
}

fn vtable_fn(this: *mut core::ffi::c_void, idx: usize) -> Option<*const core::ffi::c_void> {
    if this.is_null() {
        return None;
    }
    let vt = unsafe { *(this as *const *const *const core::ffi::c_void) };
    if vt.is_null() {
        return None;
    }
    Some(unsafe { *vt.add(idx) })
}

fn attach_slot<F: Copy + std::fmt::Debug>(
    hooks: &mut Vec<(usize, DetourHook<F>)>,
    func: F,
    detour: F,
) -> Option<F> {
    let addr = fn_addr(func);
    if let Some((_, h)) = hooks.iter().find(|(a, _)| *a == addr) {
        return Some(h.original_fn());
    }
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => {
            let orig = h.original_fn();
            hooks.push((addr, h));
            Some(orig)
        }
        Err(err) => {
            warn!("Failed hooking dinput vtable method: {err:?}");
            None
        }
    }
}

fn hook_interface_create_device(iface: *mut core::ffi::c_void) {
    let Some(ptr) = vtable_fn(iface, VT_CREATE_DEVICE) else {
        return;
    };
    let func: CreateDeviceFn = unsafe { mem::transmute(ptr) };
    let mut set = DEVICE_HOOKS.lock();
    attach_slot(&mut set.create, func, hooked_create_device);
}

fn classify(device: *mut core::ffi::c_void, guid: *const GUID) -> DiDeviceKind {
    if !guid.is_null() {
        let g = unsafe { *guid };
        if g == GUID_SYS_KEYBOARD {
            return DiDeviceKind::Keyboard;
        }
        if g == GUID_SYS_MOUSE {
            return DiDeviceKind::Mouse;
        }
    }
    caps_kind(device).unwrap_or(DiDeviceKind::Other)
}

fn caps_kind(device: *mut core::ffi::c_void) -> Option<DiDeviceKind> {
    let ptr = vtable_fn(device, VT_GET_CAPS)?;
    let get_caps: GetCapsFn = unsafe { mem::transmute(ptr) };
    let mut caps = DiDevCaps {
        dw_size: mem::size_of::<DiDevCaps>() as u32,
        dw_flags: 0,
        dw_dev_type: 0,
        _rest: [0; 8],
    };
    if unsafe { get_caps(device, &mut caps) } < 0 {
        return None;
    }
    Some(match caps.dw_dev_type & 0xFF {
        2 | 0x12 => DiDeviceKind::Mouse,
        3 | 0x13 => DiDeviceKind::Keyboard,
        _ => DiDeviceKind::Other,
    })
}

fn hook_device(device: *mut core::ffi::c_void, kind: DiDeviceKind) {
    let Some(state_ptr) = vtable_fn(device, VT_GET_STATE) else {
        return;
    };
    let Some(data_ptr) = vtable_fn(device, VT_GET_DATA) else {
        return;
    };
    let state_fn: GetDeviceStateFn = unsafe { mem::transmute(state_ptr) };
    let data_fn: GetDeviceDataFn = unsafe { mem::transmute(data_ptr) };
    let mut set = DEVICE_HOOKS.lock();
    let get_state = attach_slot(&mut set.state, state_fn, hooked_get_device_state);
    let get_data = attach_slot(&mut set.data, data_fn, hooked_get_device_data);
    drop(set);
    let (Some(get_state), Some(get_data)) = (get_state, get_data) else {
        return;
    };
    DEVICES.insert(
        device as usize,
        DeviceRec {
            kind,
            get_state,
            get_data,
        },
    );
}

fn infer_state_kind(this: *mut core::ffi::c_void, cb: u32) -> DiDeviceKind {
    if let Some(rec) = DEVICES.get(&(this as usize)) {
        return rec.kind;
    }
    match cb {
        256 => DiDeviceKind::Keyboard,
        16 | 20 => DiDeviceKind::Mouse,
        _ => DiDeviceKind::Other,
    }
}

fn call_original_state(this: *mut core::ffi::c_void, cb: u32, data: *mut core::ffi::c_void) -> i32 {
    if let Some(rec) = DEVICES.get(&(this as usize)) {
        return unsafe { (rec.get_state)(this, cb, data) };
    }
    let set = DEVICE_HOOKS.lock();
    if let Some((_, h)) = set.state.first() {
        return unsafe { h.original_fn()(this, cb, data) };
    }
    0
}

fn call_original_data(
    this: *mut core::ffi::c_void,
    cb: u32,
    data: *mut u8,
    inout: *mut u32,
    flags: u32,
) -> i32 {
    if let Some(rec) = DEVICES.get(&(this as usize)) {
        return unsafe { (rec.get_data)(this, cb, data, inout, flags) };
    }
    let set = DEVICE_HOOKS.lock();
    if let Some((_, h)) = set.data.first() {
        return unsafe { h.original_fn()(this, cb, data, inout, flags) };
    }
    0
}

unsafe extern "system" fn hooked_create_device(
    this: *mut core::ffi::c_void,
    guid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
    outer: *mut core::ffi::c_void,
) -> i32 {
    let orig = {
        let addr = vtable_fn(this, VT_CREATE_DEVICE).map(|p| p as usize);
        let set = DEVICE_HOOKS.lock();
        addr.and_then(|addr| {
            set.create
                .iter()
                .find(|(a, _)| *a == addr)
                .map(|(_, h)| h.original_fn())
        })
        .or_else(|| set.create.first().map(|(_, h)| h.original_fn()))
    };
    let Some(orig) = orig else {
        return 0x80004001u32 as i32;
    };
    let hr = unsafe { orig(this, guid, ppv, outer) };
    if hr < 0 || ppv.is_null() {
        return hr;
    }
    let device = unsafe { *ppv };
    if device.is_null() {
        return hr;
    }
    let kind = classify(device, guid);
    hook_device(device, kind);
    hr
}

unsafe extern "system" fn hooked_get_device_state(
    this: *mut core::ffi::c_void,
    cb: u32,
    data: *mut core::ffi::c_void,
) -> i32 {
    let hr = call_original_state(this, cb, data);
    if hr < 0 || data.is_null() || cb == 0 {
        return hr;
    }
    let kind = infer_state_kind(this, cb);
    let buf = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, cb as usize) };
    let out = apply_device_state(kind, buf, is_interactive());
    note_hotkey(out.hotkey);
    hr
}

unsafe extern "system" fn hooked_get_device_data(
    this: *mut core::ffi::c_void,
    cb: u32,
    data: *mut u8,
    inout: *mut u32,
    flags: u32,
) -> i32 {
    let hr = call_original_data(this, cb, data, inout, flags);
    if hr < 0 || inout.is_null() {
        return hr;
    }
    let count = unsafe { *inout };
    let kind = DEVICES
        .get(&(this as usize))
        .map(|r| r.kind)
        .unwrap_or(DiDeviceKind::Other);
    let interactive = is_interactive();
    if kind == DiDeviceKind::Other {
        return hr;
    }
    if data.is_null() || cb < 8 || count == 0 {
        if interactive && kind == DiDeviceKind::Mouse {
            unsafe { *inout = 0 };
        }
        return hr;
    }
    let mut events = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let p = unsafe { data.add(i * cb as usize) };
        events.push(DiObjectData {
            dw_ofs: unsafe { *(p as *const u32) },
            dw_data: unsafe { *(p.add(4) as *const u32) },
        });
    }
    let out = {
        let mut shadow = KB_SHADOW.lock();
        apply_device_data(kind, &mut shadow, &mut events, interactive)
    };
    for (i, ev) in events.iter().enumerate() {
        let p = unsafe { data.add(i * cb as usize) };
        unsafe { *(p.add(4) as *mut u32) = ev.dw_data };
    }
    if interactive {
        unsafe { *inout = 0 };
    }
    note_hotkey(out.hotkey);
    hr
}

unsafe extern "system" fn hooked_direct_input8_create(
    hinst: *mut core::ffi::c_void,
    version: u32,
    riid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
    outer: *mut core::ffi::c_void,
) -> i32 {
    let orig = CREATE_HOOKS.wait().di8.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(hinst, version, riid, ppv, outer) };
    if hr < 0 {
        warn!(
            "Failed creating real dinput8 interface during hooked DirectInput8Create call, game will probably crash"
        );
        return hr;
    }
    info!("DirectInput8Create hook called");
    if !ppv.is_null() {
        let iface = unsafe { *ppv };
        if !iface.is_null() {
            hook_interface_create_device(iface);
        }
    }
    hr
}

unsafe extern "system" fn hooked_direct_input_create_a(
    hinst: *mut core::ffi::c_void,
    version: u32,
    ppv: *mut *mut core::ffi::c_void,
    outer: *mut core::ffi::c_void,
) -> i32 {
    let orig = CREATE_HOOKS.wait().di_a.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(hinst, version, ppv, outer) };
    if hr < 0 {
        warn!(
            "Failed creating real dinput interface during hooked DirectInputCreateA call, game will probably crash"
        );
        return hr;
    }
    info!("DirectInputCreateA hook called");
    if !ppv.is_null() {
        let iface = unsafe { *ppv };
        if !iface.is_null() {
            hook_interface_create_device(iface);
        }
    }
    hr
}

unsafe extern "system" fn hooked_direct_input_create_w(
    hinst: *mut core::ffi::c_void,
    version: u32,
    ppv: *mut *mut core::ffi::c_void,
    outer: *mut core::ffi::c_void,
) -> i32 {
    let orig = CREATE_HOOKS.wait().di_w.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(hinst, version, ppv, outer) };
    if hr < 0 {
        warn!(
            "Failed creating real dinput interface during hooked DirectInputCreateW call, game will probably crash"
        );
        return hr;
    }
    info!("DirectInputCreateW hook called");
    if !ppv.is_null() {
        let iface = unsafe { *ppv };
        if !iface.is_null() {
            hook_interface_create_device(iface);
        }
    }
    hr
}

fn attach_named<F: Copy + std::fmt::Debug>(
    dll: PCSTR,
    name: PCSTR,
    detour: F,
) -> Option<DetourHook<F>> {
    let module = unsafe { GetModuleHandleA(dll) }
        .ok()
        .or_else(|| unsafe { LoadLibraryA(dll) }.ok())?;
    let proc = unsafe { GetProcAddress(module, name) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => Some(h),
        Err(err) => {
            warn!("Failed hooking dinput create: {err:?}");
            None
        }
    }
}

pub(super) fn hook() {
    let _ = CREATE_HOOKS.set(CreateHooks {
        di8: attach_named(
            s!("dinput8.dll"),
            s!("DirectInput8Create"),
            hooked_direct_input8_create,
        ),
        di_a: attach_named(
            s!("dinput.dll"),
            s!("DirectInputCreateA"),
            hooked_direct_input_create_a,
        ),
        di_w: attach_named(
            s!("dinput.dll"),
            s!("DirectInputCreateW"),
            hooked_direct_input_create_w,
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(pressed: &[u8]) -> [u8; 256] {
        let mut buf = [0u8; 256];
        for &dik in pressed {
            buf[dik as usize] = 0x80;
        }
        buf
    }

    #[test]
    fn unbuffered_lshift_tab_is_hotkey_and_masks_tab() {
        let mut buf = kb(&[DIK_LSHIFT, DIK_TAB]);
        let out = apply_kb_state(&mut buf, false);
        assert_eq!(out.hotkey, DiHotkey::Unbuffered);
        assert_eq!(buf[DIK_TAB as usize], 0);
        assert_eq!(buf[DIK_LSHIFT as usize], 0x80);
    }

    #[test]
    fn unbuffered_rshift_tab_is_hotkey_and_masks_tab() {
        let mut buf = kb(&[DIK_RSHIFT, DIK_TAB]);
        let out = apply_kb_state(&mut buf, false);
        assert_eq!(out.hotkey, DiHotkey::Unbuffered);
        assert_eq!(buf[DIK_TAB as usize], 0);
        assert_eq!(buf[DIK_RSHIFT as usize], 0x80);
    }

    #[test]
    fn unbuffered_tab_alone_is_not_hotkey() {
        let mut buf = kb(&[DIK_TAB]);
        let out = apply_kb_state(&mut buf, false);
        assert_eq!(out.hotkey, DiHotkey::None);
        assert_eq!(buf[DIK_TAB as usize], 0x80);
    }

    #[test]
    fn unbuffered_interactive_zeros_after_hotkey_scan() {
        let mut buf = kb(&[DIK_LSHIFT, DIK_TAB, 0x1E]);
        let out = apply_kb_state(&mut buf, true);
        assert_eq!(out.hotkey, DiHotkey::Unbuffered);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn unbuffered_interactive_zeros_without_hotkey() {
        let mut buf = kb(&[0x1E]);
        let out = apply_kb_state(&mut buf, true);
        assert_eq!(out.hotkey, DiHotkey::None);
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn buffered_shift_tab_is_hotkey_and_masks_tab() {
        let mut shadow = [0u8; 256];
        let mut data = [
            DiObjectData {
                dw_ofs: DIK_LSHIFT as u32,
                dw_data: 0x80,
            },
            DiObjectData {
                dw_ofs: DIK_TAB as u32,
                dw_data: 0x80,
            },
        ];
        let out = apply_kb_data(&mut shadow, &mut data, false);
        assert_eq!(out.hotkey, DiHotkey::Buffered);
        assert_eq!(data[1].dw_data, 0);
        assert_eq!(data[0].dw_data, 0x80);
    }

    #[test]
    fn buffered_tab_with_prior_shift_is_hotkey() {
        let mut shadow = kb(&[DIK_RSHIFT]);
        let mut data = [DiObjectData {
            dw_ofs: DIK_TAB as u32,
            dw_data: 0x80,
        }];
        let out = apply_kb_data(&mut shadow, &mut data, false);
        assert_eq!(out.hotkey, DiHotkey::Buffered);
        assert_eq!(data[0].dw_data, 0);
    }

    #[test]
    fn buffered_interactive_zeros() {
        let mut shadow = [0u8; 256];
        let mut data = [DiObjectData {
            dw_ofs: 0x1E,
            dw_data: 0x80,
        }];
        let out = apply_kb_data(&mut shadow, &mut data, true);
        assert_eq!(out.hotkey, DiHotkey::None);
        assert_eq!(data[0].dw_data, 0);
    }

    #[test]
    fn mouse_interactive_zeros_keyboard_passthrough_not_applied() {
        let mut buf = [1u8, 2, 3, 4];
        let out = apply_device_state(DiDeviceKind::Mouse, &mut buf, true);
        assert_eq!(out.hotkey, DiHotkey::None);
        assert_eq!(buf, [0, 0, 0, 0]);
    }

    #[test]
    fn joystick_passthrough_even_when_interactive() {
        let mut buf = [1u8, 2, 3, 4];
        let out = apply_device_state(DiDeviceKind::Other, &mut buf, true);
        assert_eq!(out.hotkey, DiHotkey::None);
        assert_eq!(buf, [1, 2, 3, 4]);

        let mut shadow = [0u8; 256];
        let mut data = [DiObjectData {
            dw_ofs: 0,
            dw_data: 0x80,
        }];
        let out = apply_device_data(DiDeviceKind::Other, &mut shadow, &mut data, true);
        assert_eq!(out.hotkey, DiHotkey::None);
        assert_eq!(data[0].dw_data, 0x80);
    }
}
