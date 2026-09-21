//! GameInput v1.0 / v1.1 / v2.0 / v3.0 isolation + Shift+Tab (Steam D7).
//!
//! Always create the real IGameInput first. Never a fake interface.

use std::{
    mem,
    sync::atomic::{AtomicBool, Ordering},
};

use glint_overlay_event::input::{Key, KeyInputState, KeyboardInput};
use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::{
    Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
    core::{GUID, PCSTR, s},
};

use crate::{backend::Backends, backend::window::input_event, event_sink::OverlayEventSink};

const KIND_KEYBOARD: u32 = 0x00000010;
const KIND_MOUSE: u32 = 0x00000020;
const KIND_CONTROLLER: u32 = 0x0000000E;
const KIND_GAMEPAD: u32 = 0x00040000;

const VK_TAB: u8 = 0x09;
const VK_SHIFT: u8 = 0x10;
const VK_LSHIFT: u8 = 0xA0;
const VK_RSHIFT: u8 = 0xA1;

const E_FAIL: i32 = 0x8000_4005u32 as i32;

const VT_QI: usize = 0;
const VT_RELEASE: usize = 2;
const VT_GET_CURRENT: usize = 4;
const VT_GET_NEXT: usize = 5;
const VT_GET_PREV: usize = 6;

const IID_V10: GUID = GUID::from_u128(0x11BE2A7E_4254_445A_9C09_FFC40F006918);
const IID_V11: GUID = GUID::from_u128(0x40FFB7E4_6150_407A_B439_132BADC08D2D);
const IID_V20: GUID = GUID::from_u128(0xBBAA66D2_837A_40F7_A303_917D500955F4);
const IID_V30: GUID = GUID::from_u128(0x20EFC1C7_5D9A_43BA_B26F_B807FA48609C);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GiKind {
    Keyboard,
    Mouse,
    Gamepad,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GiHotkey {
    None,
    Chord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GiOutcome {
    hotkey: GiHotkey,
    consume: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyLayout {
    /// v1.0 / v0 reading: GetKeyCount=14, GetKeyState=15
    V0,
    /// v1.1 / v2 / v3 reading: GetKeyCount=12, GetKeyState=13
    V1,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GiKeyState {
    scan_code: u32,
    code_point: u32,
    virtual_key: u8,
    is_dead_key: u8,
    _pad: [u8; 2],
}

type GameInputCreateFn = unsafe extern "system" fn(*mut *mut core::ffi::c_void) -> i32;
type GameInputInitializeFn =
    unsafe extern "system" fn(*const GUID, *mut *mut core::ffi::c_void) -> i32;
type CoCreateInstanceFn = unsafe extern "system" fn(
    *const GUID,
    *mut core::ffi::c_void,
    u32,
    *const GUID,
    *mut *mut core::ffi::c_void,
) -> i32;
type GetCurrentReadingFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    u32,
    *mut core::ffi::c_void,
    *mut *mut core::ffi::c_void,
) -> i32;
type GetNextReadingFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
    *mut core::ffi::c_void,
    *mut *mut core::ffi::c_void,
) -> i32;
type QiFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *const GUID,
    *mut *mut core::ffi::c_void,
) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
type GetKeyCountFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
type GetKeyStateFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32, *mut GiKeyState) -> u32;

struct CreateHooks {
    create: Option<DetourHook<GameInputCreateFn>>,
    init: Option<DetourHook<GameInputInitializeFn>>,
    cocreate: Option<DetourHook<CoCreateInstanceFn>>,
}

struct ReadingHookSet {
    current: Vec<(usize, KeyLayout, DetourHook<GetCurrentReadingFn>)>,
    next: Vec<(usize, KeyLayout, DetourHook<GetNextReadingFn>)>,
    prev: Vec<(usize, KeyLayout, DetourHook<GetNextReadingFn>)>,
}

static CREATE_HOOKS: OnceCell<CreateHooks> = OnceCell::new();
static READING_HOOKS: Mutex<ReadingHookSet> = Mutex::new(ReadingHookSet {
    current: Vec::new(),
    next: Vec::new(),
    prev: Vec::new(),
});
static CHORD: AtomicBool = AtomicBool::new(false);

fn classify_kind(kind_bits: u32) -> GiKind {
    if kind_bits & KIND_KEYBOARD != 0 {
        GiKind::Keyboard
    } else if kind_bits & KIND_MOUSE != 0 {
        GiKind::Mouse
    } else if kind_bits & KIND_GAMEPAD != 0 || kind_bits & KIND_CONTROLLER != 0 {
        GiKind::Gamepad
    } else {
        GiKind::Other
    }
}

fn shift_down(keys: &[u8]) -> bool {
    keys.iter()
        .any(|&k| k == VK_SHIFT || k == VK_LSHIFT || k == VK_RSHIFT)
}

fn apply_reading(kind: GiKind, keys: &[u8], interactive: bool) -> GiOutcome {
    let hotkey = if kind == GiKind::Keyboard && shift_down(keys) && keys.contains(&VK_TAB) {
        GiHotkey::Chord
    } else {
        GiHotkey::None
    };
    let consume = match kind {
        GiKind::Other => false,
        GiKind::Keyboard => interactive || hotkey == GiHotkey::Chord,
        GiKind::Mouse | GiKind::Gamepad => interactive,
    };
    GiOutcome { hotkey, consume }
}

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn note_hotkey_kind(kind: GiKind, hotkey: GiHotkey, chord: &AtomicBool) -> bool {
    if kind != GiKind::Keyboard {
        return false;
    }
    let held = hotkey != GiHotkey::None;
    let was = chord.swap(held, Ordering::Relaxed);
    held && !was
}

fn note_hotkey(kind: GiKind, hotkey: GiHotkey) {
    if note_hotkey_kind(kind, hotkey, &CHORD) {
        emit_shift_tab();
    }
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

fn versions() -> [(&'static str, GUID, KeyLayout); 4] {
    [
        ("v3.0", IID_V30, KeyLayout::V1),
        ("v2.0", IID_V20, KeyLayout::V1),
        ("v1.1", IID_V11, KeyLayout::V1),
        ("v1.0", IID_V10, KeyLayout::V0),
    ]
}

fn version_of(iid: &GUID) -> Option<(&'static str, KeyLayout)> {
    versions()
        .into_iter()
        .find(|(_, id, _)| *id == *iid)
        .map(|(n, _, l)| (n, l))
}

fn query_interface(this: *mut core::ffi::c_void, iid: &GUID) -> Option<*mut core::ffi::c_void> {
    let ptr = vtable_fn(this, VT_QI)?;
    let qi: QiFn = unsafe { mem::transmute(ptr) };
    let mut out = std::ptr::null_mut();
    if unsafe { qi(this, iid, &mut out) } < 0 || out.is_null() {
        return None;
    }
    Some(out)
}

fn release(this: *mut core::ffi::c_void) {
    if let Some(ptr) = vtable_fn(this, VT_RELEASE) {
        let rel: ReleaseFn = unsafe { mem::transmute(ptr) };
        unsafe { rel(this) };
    }
}

fn keys_from_reading(reading: *mut core::ffi::c_void, layout: KeyLayout) -> Vec<u8> {
    let (count_idx, state_idx) = match layout {
        KeyLayout::V0 => (14, 15),
        KeyLayout::V1 => (12, 13),
    };
    let Some(count_ptr) = vtable_fn(reading, count_idx) else {
        return Vec::new();
    };
    let Some(state_ptr) = vtable_fn(reading, state_idx) else {
        return Vec::new();
    };
    let get_count: GetKeyCountFn = unsafe { mem::transmute(count_ptr) };
    let get_state: GetKeyStateFn = unsafe { mem::transmute(state_ptr) };
    let count = unsafe { get_count(reading) }.min(16);
    if count == 0 {
        return Vec::new();
    }
    let mut buf = vec![
        GiKeyState {
            scan_code: 0,
            code_point: 0,
            virtual_key: 0,
            is_dead_key: 0,
            _pad: [0; 2],
        };
        count as usize
    ];
    let got = unsafe { get_state(reading, count, buf.as_mut_ptr()) }.min(count);
    buf.truncate(got as usize);
    buf.into_iter().map(|k| k.virtual_key).collect()
}

fn finish_reading(
    kind_bits: u32,
    reading: *mut *mut core::ffi::c_void,
    layout: KeyLayout,
    hr: i32,
) -> i32 {
    if hr < 0 || reading.is_null() {
        return hr;
    }
    let obj = unsafe { *reading };
    if obj.is_null() {
        return hr;
    }
    let kind = classify_kind(kind_bits);
    let keys = if kind == GiKind::Keyboard {
        keys_from_reading(obj, layout)
    } else {
        Vec::new()
    };
    let out = apply_reading(kind, &keys, is_interactive());
    note_hotkey(kind, out.hotkey);
    if out.consume {
        release(obj);
        unsafe { *reading = std::ptr::null_mut() };
        return E_FAIL;
    }
    hr
}

fn attach_current(
    hooks: &mut Vec<(usize, KeyLayout, DetourHook<GetCurrentReadingFn>)>,
    func: GetCurrentReadingFn,
    layout: KeyLayout,
) {
    let addr = fn_addr(func);
    if hooks.iter().any(|(a, _, _)| *a == addr) {
        return;
    }
    match unsafe { DetourHook::attach(func, hooked_get_current) } {
        Ok(h) => hooks.push((addr, layout, h)),
        Err(err) => warn!("Failed hooking GameInput GetCurrentReading: {err:?}"),
    }
}

fn attach_walk(
    hooks: &mut Vec<(usize, KeyLayout, DetourHook<GetNextReadingFn>)>,
    func: GetNextReadingFn,
    detour: GetNextReadingFn,
    layout: KeyLayout,
    label: &str,
) {
    let addr = fn_addr(func);
    if hooks.iter().any(|(a, _, _)| *a == addr) {
        return;
    }
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => hooks.push((addr, layout, h)),
        Err(err) => warn!("Failed hooking GameInput {label}: {err:?}"),
    }
}

fn attach_readings(iface: *mut core::ffi::c_void, layout: KeyLayout) {
    let Some(cur) = vtable_fn(iface, VT_GET_CURRENT) else {
        return;
    };
    let Some(next) = vtable_fn(iface, VT_GET_NEXT) else {
        return;
    };
    let Some(prev) = vtable_fn(iface, VT_GET_PREV) else {
        return;
    };
    let cur: GetCurrentReadingFn = unsafe { mem::transmute(cur) };
    let next: GetNextReadingFn = unsafe { mem::transmute(next) };
    let prev: GetNextReadingFn = unsafe { mem::transmute(prev) };
    let mut set = READING_HOOKS.lock();
    attach_current(&mut set.current, cur, layout);
    attach_walk(
        &mut set.next,
        next,
        hooked_get_next,
        layout,
        "GetNextReading",
    );
    attach_walk(
        &mut set.prev,
        prev,
        hooked_get_prev,
        layout,
        "GetPreviousReading",
    );
}

fn hook_iface(iface: *mut core::ffi::c_void) {
    if iface.is_null() {
        return;
    }
    let mut any = false;
    for (label, iid, layout) in versions() {
        if let Some(qi) = query_interface(iface, &iid) {
            info!("Hooking GameInput {label}");
            attach_readings(qi, layout);
            release(qi);
            any = true;
        }
    }
    if !any {
        attach_readings(iface, KeyLayout::V1);
    }
}

fn hook_known(iface: *mut core::ffi::c_void, iid: &GUID) {
    if iface.is_null() {
        return;
    }
    if let Some((label, layout)) = version_of(iid) {
        info!("Hooking GameInput {label}");
        attach_readings(iface, layout);
    } else {
        hook_iface(iface);
    }
}

fn lookup_current(this: *mut core::ffi::c_void) -> Option<(GetCurrentReadingFn, KeyLayout)> {
    let addr = vtable_fn(this, VT_GET_CURRENT).map(|p| p as usize);
    let set = READING_HOOKS.lock();
    addr.and_then(|addr| {
        set.current
            .iter()
            .find(|(a, _, _)| *a == addr)
            .map(|(_, l, h)| (h.original_fn(), *l))
    })
    .or_else(|| set.current.first().map(|(_, l, h)| (h.original_fn(), *l)))
}

fn lookup_walk(
    this: *mut core::ffi::c_void,
    idx: usize,
    hooks: &[(usize, KeyLayout, DetourHook<GetNextReadingFn>)],
) -> Option<(GetNextReadingFn, KeyLayout)> {
    let addr = vtable_fn(this, idx).map(|p| p as usize);
    addr.and_then(|addr| {
        hooks
            .iter()
            .find(|(a, _, _)| *a == addr)
            .map(|(_, l, h)| (h.original_fn(), *l))
    })
    .or_else(|| hooks.first().map(|(_, l, h)| (h.original_fn(), *l)))
}

unsafe extern "system" fn hooked_get_current(
    this: *mut core::ffi::c_void,
    kind: u32,
    device: *mut core::ffi::c_void,
    reading: *mut *mut core::ffi::c_void,
) -> i32 {
    let Some((orig, layout)) = lookup_current(this) else {
        return E_FAIL;
    };
    let hr = unsafe { orig(this, kind, device, reading) };
    finish_reading(kind, reading, layout, hr)
}

unsafe extern "system" fn hooked_get_next(
    this: *mut core::ffi::c_void,
    reference: *mut core::ffi::c_void,
    kind: u32,
    device: *mut core::ffi::c_void,
    reading: *mut *mut core::ffi::c_void,
) -> i32 {
    let orig = {
        let set = READING_HOOKS.lock();
        lookup_walk(this, VT_GET_NEXT, &set.next)
    };
    let Some((orig, layout)) = orig else {
        return E_FAIL;
    };
    let hr = unsafe { orig(this, reference, kind, device, reading) };
    finish_reading(kind, reading, layout, hr)
}

unsafe extern "system" fn hooked_get_prev(
    this: *mut core::ffi::c_void,
    reference: *mut core::ffi::c_void,
    kind: u32,
    device: *mut core::ffi::c_void,
    reading: *mut *mut core::ffi::c_void,
) -> i32 {
    let orig = {
        let set = READING_HOOKS.lock();
        lookup_walk(this, VT_GET_PREV, &set.prev)
    };
    let Some((orig, layout)) = orig else {
        return E_FAIL;
    };
    let hr = unsafe { orig(this, reference, kind, device, reading) };
    finish_reading(kind, reading, layout, hr)
}

unsafe extern "system" fn hooked_game_input_create(ppv: *mut *mut core::ffi::c_void) -> i32 {
    let orig = CREATE_HOOKS.wait().create.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(ppv) };
    if hr < 0 {
        warn!("Failed creating real GameInput interface, game will probably crash");
        return hr;
    }
    info!("hookGameInputCreate called");
    if !ppv.is_null() {
        hook_iface(unsafe { *ppv });
    }
    hr
}

unsafe extern "system" fn hooked_game_input_initialize(
    iid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> i32 {
    let orig = CREATE_HOOKS.wait().init.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(iid, ppv) };
    if hr < 0 {
        warn!("Failed creating real GameInput interface, game will probably crash");
        return hr;
    }
    if !ppv.is_null() && !iid.is_null() {
        hook_known(unsafe { *ppv }, unsafe { &*iid });
    }
    hr
}

unsafe extern "system" fn hooked_co_create(
    clsid: *const GUID,
    outer: *mut core::ffi::c_void,
    ctx: u32,
    iid: *const GUID,
    ppv: *mut *mut core::ffi::c_void,
) -> i32 {
    let orig = CREATE_HOOKS.wait().cocreate.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(clsid, outer, ctx, iid, ppv) };
    if hr < 0 || iid.is_null() || ppv.is_null() {
        return hr;
    }
    if version_of(unsafe { &*iid }).is_none() {
        return hr;
    }
    info!("hookGameInputCreate called");
    hook_known(unsafe { *ppv }, unsafe { &*iid });
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
            warn!("Failed hooking GameInput create: {err:?}");
            None
        }
    }
}

pub(super) fn hook() {
    let _ = CREATE_HOOKS.set(CreateHooks {
        create: attach_named(
            s!("gameinput.dll"),
            s!("GameInputCreate"),
            hooked_game_input_create,
        ),
        init: attach_named(
            s!("gameinputredist.dll"),
            s!("GameInputInitialize"),
            hooked_game_input_initialize,
        ),
        cocreate: attach_named(s!("ole32.dll"), s!("CoCreateInstance"), hooked_co_create),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_mouse_is_consumed() {
        let out = apply_reading(GiKind::Mouse, &[], true);
        assert!(out.consume);
        assert_eq!(out.hotkey, GiHotkey::None);
    }

    #[test]
    fn interactive_keyboard_is_consumed() {
        let out = apply_reading(GiKind::Keyboard, &[0x41], true);
        assert!(out.consume);
        assert_eq!(out.hotkey, GiHotkey::None);
    }

    #[test]
    fn interactive_gamepad_is_consumed() {
        let out = apply_reading(GiKind::Gamepad, &[], true);
        assert!(out.consume);
        assert_eq!(out.hotkey, GiHotkey::None);
    }

    #[test]
    fn interactive_other_passes_through() {
        let out = apply_reading(GiKind::Other, &[], true);
        assert!(!out.consume);
        assert_eq!(out.hotkey, GiHotkey::None);
    }

    #[test]
    fn not_interactive_keyboard_passes_through() {
        let out = apply_reading(GiKind::Keyboard, &[0x41], false);
        assert!(!out.consume);
        assert_eq!(out.hotkey, GiHotkey::None);
    }

    #[test]
    fn not_interactive_mouse_passes_through() {
        let out = apply_reading(GiKind::Mouse, &[], false);
        assert!(!out.consume);
    }

    #[test]
    fn lshift_tab_is_hotkey_and_consumed() {
        let out = apply_reading(GiKind::Keyboard, &[VK_LSHIFT, VK_TAB], false);
        assert_eq!(out.hotkey, GiHotkey::Chord);
        assert!(out.consume);
    }

    #[test]
    fn rshift_tab_is_hotkey_and_consumed() {
        let out = apply_reading(GiKind::Keyboard, &[VK_RSHIFT, VK_TAB], false);
        assert_eq!(out.hotkey, GiHotkey::Chord);
        assert!(out.consume);
    }

    #[test]
    fn generic_shift_tab_is_hotkey_and_consumed() {
        let out = apply_reading(GiKind::Keyboard, &[VK_SHIFT, VK_TAB], false);
        assert_eq!(out.hotkey, GiHotkey::Chord);
        assert!(out.consume);
    }

    #[test]
    fn tab_alone_is_not_hotkey() {
        let out = apply_reading(GiKind::Keyboard, &[VK_TAB], false);
        assert_eq!(out.hotkey, GiHotkey::None);
        assert!(!out.consume);
    }

    #[test]
    fn interactive_shift_tab_is_hotkey_and_consumed() {
        let out = apply_reading(GiKind::Keyboard, &[VK_LSHIFT, VK_TAB], true);
        assert_eq!(out.hotkey, GiHotkey::Chord);
        assert!(out.consume);
    }

    #[test]
    fn classify_keyboard_mouse_gamepad_bits() {
        assert_eq!(classify_kind(KIND_KEYBOARD), GiKind::Keyboard);
        assert_eq!(classify_kind(KIND_MOUSE), GiKind::Mouse);
        assert_eq!(classify_kind(KIND_GAMEPAD), GiKind::Gamepad);
        assert_eq!(classify_kind(KIND_CONTROLLER), GiKind::Gamepad);
        assert_eq!(classify_kind(0), GiKind::Other);
        assert_eq!(classify_kind(KIND_KEYBOARD | KIND_MOUSE), GiKind::Keyboard);
    }

    #[test]
    fn interactive_controller_kind_is_consumed() {
        let kind = classify_kind(KIND_CONTROLLER);
        let out = apply_reading(kind, &[], true);
        assert_eq!(kind, GiKind::Gamepad);
        assert!(out.consume);
    }

    #[test]
    fn mouse_or_pad_poll_does_not_clear_shift_tab_latch() {
        let chord = AtomicBool::new(false);
        assert!(note_hotkey_kind(GiKind::Keyboard, GiHotkey::Chord, &chord));
        assert!(chord.load(Ordering::Relaxed));
        assert!(!note_hotkey_kind(GiKind::Mouse, GiHotkey::None, &chord));
        assert!(!note_hotkey_kind(GiKind::Gamepad, GiHotkey::None, &chord));
        assert!(!note_hotkey_kind(GiKind::Other, GiHotkey::None, &chord));
        assert!(chord.load(Ordering::Relaxed));
        assert!(!note_hotkey_kind(GiKind::Keyboard, GiHotkey::Chord, &chord));
    }
}
