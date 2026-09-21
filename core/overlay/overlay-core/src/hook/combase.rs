//! `RoGetActivationFactory` (Steam `sub_1800CE480`).
//!
//! `GetModuleHandle("combase.dll")` only. Call original. On Steam’s three
//! IIDs, log and `hook_known` the real factory (do not invent a wrapper
//! object). Interactive readings empty on confirmed slots:
//! IGamepad `GetCurrentReading` vtable+64 (slot 8, `sub_1800CCFA0`);
//! IRawGameController `GetCurrentReading` vtable+104 (slot 13, `sub_1800CD040`).
//! FromGameController: IRaw slot 11 (original+88); IGamepadStatics2 slot 6
//! (original+48). IGamepadStatics slot 6 is `add_GamepadAdded` — do not hook.
//! Do not detour Windows `IVectorView.GetAt`. Do not hook
//! `WindowsCreateString` / `DeleteString` / `GetStringRawBuffer`.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{GUID, s},
};

use crate::backend::Backends;

/// IGamepadStatics2 `FromGameController` (original+48).
const VT_IGAMEPAD2_FROM: usize = 6;
/// IRawGameControllerStatics `FromGameController` (original+88).
const VT_IRAW_FROM: usize = 11;
const VT_IGAMEPAD_READING: usize = 8;
const VT_IRAW_READING: usize = 13;

const QWORD_RAW: (u64, u64) = (0x4B19_E95A_EB8D_0792, 0x9E75_BFF8_590A_C7AF);
const QWORD_GAMEPAD: (u64, u64) = (0x39E9_D49C_8BBC_E529, 0xC8B7_96DE_7DE4_6095);
const QWORD_GAMEPAD2: (u64, u64) = (0x47C4_0856_4267_6DC5, 0x3C3A_4C50_95B3_1392);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FactoryKind {
    Raw,
    Gamepad,
    Gamepad2,
}

type RoGetFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *const GUID,
    *mut *mut core::ffi::c_void,
) -> i32;
type FromCtrlFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    *mut *mut core::ffi::c_void,
) -> i32;
type GamepadReadingFn = unsafe extern "system" fn(*mut core::ffi::c_void, *mut u8) -> i32;
type RawReadingFn = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    u32,
    *mut u8,
    u32,
    *mut u32,
    u32,
    *mut f64,
) -> i32;

struct Hooks {
    ro_get: Option<DetourHook<RoGetFn>>,
}

struct ReadingHooks {
    from_ctrl: Vec<(usize, FactoryKind, DetourHook<FromCtrlFn>)>,
    gamepad: Vec<DetourHook<GamepadReadingFn>>,
    raw: Vec<DetourHook<RawReadingFn>>,
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();
static READING: Mutex<ReadingHooks> = Mutex::new(ReadingHooks {
    from_ctrl: Vec::new(),
    gamepad: Vec::new(),
    raw: Vec::new(),
});

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("combase.dll")) }.ok()
}

fn iid_qwords(iid: *const GUID) -> Option<(u64, u64)> {
    if iid.is_null() {
        return None;
    }
    let mut buf = [0u8; 16];
    unsafe { std::ptr::copy_nonoverlapping(iid as *const u8, buf.as_mut_ptr(), 16) };
    Some((
        u64::from_le_bytes(buf[0..8].try_into().unwrap()),
        u64::from_le_bytes(buf[8..16].try_into().unwrap()),
    ))
}

fn factory_kind(iid: *const GUID) -> Option<FactoryKind> {
    match iid_qwords(iid)? {
        QWORD_RAW => Some(FactoryKind::Raw),
        QWORD_GAMEPAD => Some(FactoryKind::Gamepad),
        QWORD_GAMEPAD2 => Some(FactoryKind::Gamepad2),
        _ => None,
    }
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

fn fn_addr<F: Copy>(func: F) -> usize {
    unsafe { mem::transmute_copy(&func) }
}

fn empty_gamepad_reading(buf: *mut u8) {
    if buf.is_null() {
        return;
    }
    // Steam `sub_1800CE380`: keep Timestamp at +0; zero Buttons (+8) and analogs.
    unsafe { std::ptr::write_bytes(buf.add(8), 0, 56) };
}

fn empty_raw_reading(buttons: *mut u8, button_n: u32, switches: *mut u32, switch_n: u32) {
    if !buttons.is_null() && button_n > 0 {
        unsafe { std::ptr::write_bytes(buttons, 0, button_n as usize) };
    }
    if !switches.is_null() && switch_n > 0 {
        unsafe { std::ptr::write_bytes(switches, 0, switch_n as usize) };
    }
}

fn attach_reading(obj: *mut core::ffi::c_void, kind: FactoryKind) {
    if obj.is_null() {
        return;
    }
    match kind {
        FactoryKind::Gamepad | FactoryKind::Gamepad2 => {
            let Some(ptr) = vtable_fn(obj, VT_IGAMEPAD_READING) else {
                return;
            };
            let func: GamepadReadingFn = unsafe { mem::transmute(ptr) };
            let mut set = READING.lock();
            if set
                .gamepad
                .iter()
                .any(|h| fn_addr(h.original_fn()) == fn_addr(func))
            {
                return;
            }
            match unsafe { DetourHook::attach(func, hooked_gamepad_reading) } {
                Ok(h) => set.gamepad.push(h),
                Err(err) => warn!("Failed hooking IGamepad GetCurrentReading: {err:?}"),
            }
        }
        FactoryKind::Raw => {
            let Some(ptr) = vtable_fn(obj, VT_IRAW_READING) else {
                return;
            };
            let func: RawReadingFn = unsafe { mem::transmute(ptr) };
            let mut set = READING.lock();
            if set
                .raw
                .iter()
                .any(|h| fn_addr(h.original_fn()) == fn_addr(func))
            {
                return;
            }
            match unsafe { DetourHook::attach(func, hooked_raw_reading) } {
                Ok(h) => set.raw.push(h),
                Err(err) => warn!("Failed hooking IRawGameController GetCurrentReading: {err:?}"),
            }
        }
    }
}

fn attach_from_ctrl(factory: *mut core::ffi::c_void, kind: FactoryKind, slot: usize) {
    let Some(ptr) = vtable_fn(factory, slot) else {
        return;
    };
    let func: FromCtrlFn = unsafe { mem::transmute(ptr) };
    let addr = fn_addr(func);
    let mut set = READING.lock();
    if set.from_ctrl.iter().any(|(a, _, _)| *a == addr) {
        return;
    }
    match unsafe { DetourHook::attach(func, hooked_from_ctrl) } {
        Ok(h) => set.from_ctrl.push((addr, kind, h)),
        Err(err) => warn!("Failed hooking FromGameController: {err:?}"),
    }
}

/// IRaw: FromGameController is slot 11 (+88), not 6.
/// IGamepadStatics2: FromGameController is slot 6 (+48).
/// IGamepadStatics: no FromGameController — do not touch slot 6 (GamepadAdded).
fn hook_known(factory: *mut core::ffi::c_void, kind: FactoryKind) {
    if factory.is_null() {
        return;
    }
    match kind {
        FactoryKind::Raw => attach_from_ctrl(factory, kind, VT_IRAW_FROM),
        FactoryKind::Gamepad2 => attach_from_ctrl(factory, kind, VT_IGAMEPAD2_FROM),
        FactoryKind::Gamepad => {}
    }
}

fn lookup_from(this: *mut core::ffi::c_void) -> Option<(FromCtrlFn, FactoryKind)> {
    let candidates = [
        vtable_fn(this, VT_IGAMEPAD2_FROM),
        vtable_fn(this, VT_IRAW_FROM),
    ];
    let set = READING.lock();
    for ptr in candidates.into_iter().flatten() {
        let addr = ptr as usize;
        if let Some((_, k, h)) = set.from_ctrl.iter().find(|(a, _, _)| *a == addr) {
            return Some((h.original_fn(), *k));
        }
    }
    set.from_ctrl.first().map(|(_, k, h)| (h.original_fn(), *k))
}

unsafe extern "system" fn hooked_from_ctrl(
    this: *mut core::ffi::c_void,
    controller: *mut core::ffi::c_void,
    out: *mut *mut core::ffi::c_void,
) -> i32 {
    let Some((orig, kind)) = lookup_from(this) else {
        return 0x8000_4005u32 as i32;
    };
    let hr = unsafe { orig(this, controller, out) };
    if hr >= 0 && !out.is_null() {
        attach_reading(unsafe { *out }, kind);
    }
    hr
}

unsafe extern "system" fn hooked_gamepad_reading(
    this: *mut core::ffi::c_void,
    reading: *mut u8,
) -> i32 {
    let orig = {
        let set = READING.lock();
        set.gamepad.first().map(|h| h.original_fn())
    };
    let Some(orig) = orig else {
        return 0x8000_4005u32 as i32;
    };
    let hr = unsafe { orig(this, reading) };
    if hr >= 0 && is_interactive() {
        empty_gamepad_reading(reading);
    }
    hr
}

unsafe extern "system" fn hooked_raw_reading(
    this: *mut core::ffi::c_void,
    button_n: u32,
    buttons: *mut u8,
    switch_n: u32,
    switches: *mut u32,
    axis_n: u32,
    axes: *mut f64,
) -> i32 {
    let orig = {
        let set = READING.lock();
        set.raw.first().map(|h| h.original_fn())
    };
    let Some(orig) = orig else {
        return 0x8000_4005u32 as i32;
    };
    let hr = unsafe { orig(this, button_n, buttons, switch_n, switches, axis_n, axes) };
    if hr >= 0 && is_interactive() {
        empty_raw_reading(buttons, button_n, switches, switch_n);
    }
    hr
}

unsafe extern "system" fn hooked_ro_get(
    class_id: *mut core::ffi::c_void,
    iid: *const GUID,
    factory: *mut *mut core::ffi::c_void,
) -> i32 {
    let orig = HOOKS.wait().ro_get.as_ref().unwrap().original_fn();
    let hr = unsafe { orig(class_id, iid, factory) };
    if hr < 0 {
        return hr;
    }
    let Some(kind) = factory_kind(iid) else {
        return hr;
    };
    match kind {
        FactoryKind::Raw => {
            info!("hookRoGetActivationFactory called for IID_IRawGameControllerStatics");
        }
        FactoryKind::Gamepad => {
            info!("hookRoGetActivationFactory called for IID_IGamepadStatics");
        }
        FactoryKind::Gamepad2 => {
            info!("hookRoGetActivationFactory called for IID_IGamepadStatics2");
        }
    }
    if !factory.is_null() {
        hook_known(unsafe { *factory }, kind);
    }
    hr
}

pub(super) fn hook() {
    let Some(module) = module() else {
        let _ = HOOKS.set(Hooks { ro_get: None });
        return;
    };
    let ro_get = unsafe { GetProcAddress(module, s!("RoGetActivationFactory")) }.and_then(|proc| {
        let func: RoGetFn = unsafe { mem::transmute_copy(&proc) };
        match unsafe { DetourHook::attach(func, hooked_ro_get) } {
            Ok(h) => Some(h),
            Err(err) => {
                warn!("Failed hooking RoGetActivationFactory: {err:?}");
                None
            }
        }
    });
    let _ = HOOKS.set(Hooks { ro_get });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_iids_match_qword_pairs() {
        fn from_qwords(q: (u64, u64)) -> GUID {
            let mut buf = [0u8; 16];
            buf[..8].copy_from_slice(&q.0.to_le_bytes());
            buf[8..].copy_from_slice(&q.1.to_le_bytes());
            unsafe { std::ptr::read(buf.as_ptr() as *const GUID) }
        }
        assert_eq!(
            factory_kind(&from_qwords(QWORD_RAW)),
            Some(FactoryKind::Raw)
        );
        assert_eq!(
            factory_kind(&from_qwords(QWORD_GAMEPAD)),
            Some(FactoryKind::Gamepad)
        );
        assert_eq!(
            factory_kind(&from_qwords(QWORD_GAMEPAD2)),
            Some(FactoryKind::Gamepad2)
        );
    }

    #[test]
    fn confirmed_from_and_reading_slots() {
        assert_eq!(VT_IRAW_FROM, 11);
        assert_eq!(VT_IGAMEPAD2_FROM, 6);
        assert_eq!(VT_IGAMEPAD_READING, 8);
        assert_eq!(VT_IRAW_READING, 13);
        assert_eq!(8 * VT_IGAMEPAD2_FROM, 48);
        assert_eq!(8 * VT_IRAW_FROM, 88);
        assert_eq!(8 * VT_IGAMEPAD_READING, 64);
        assert_eq!(8 * VT_IRAW_READING, 104);
    }

    #[test]
    fn interactive_readings_zero_buttons_keep_timestamp() {
        let mut buf = [1u8; 64];
        empty_gamepad_reading(buf.as_mut_ptr());
        assert_eq!(&buf[..8], &[1u8; 8]);
        assert!(buf[8..].iter().all(|&b| b == 0));
        let mut buttons = [1u8, 1, 1];
        let mut switches = [3u32, 4];
        empty_raw_reading(buttons.as_mut_ptr(), 3, switches.as_mut_ptr(), 2);
        assert_eq!(buttons, [0, 0, 0]);
        assert_eq!(switches, [0, 0]);
    }

    #[test]
    fn unknown_iid_is_not_wrapped() {
        let g = GUID::from_u128(0);
        assert!(factory_kind(&g).is_none());
    }
}
