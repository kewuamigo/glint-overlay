//! XInput 1.1–1.4 isolation (Steam `sub_1800CEC10`).
//!
//! `GetModuleHandle` only — never `LoadLibrary`. Missing DLL is success.
//! Interactive = `blocking_state.is_some()`. Extra exports attach if present
//! and call original (no fake GUIDs). No pad Shift+Tab chord.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{GUID, PCSTR, s},
};

use crate::backend::Backends;

const ERROR_SUCCESS: u32 = 0;
const ERROR_EMPTY: u32 = 4306;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const GET_STATE_EX_ORDINAL: usize = 100;
const GET_CAPS_EX_ORDINAL: usize = 0x6C;

const VER_11: u32 = 11;
const VER_12: u32 = 12;
const VER_13: u32 = 13;
const VER_14: u32 = 14;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct XInputGamepad {
    w_buttons: u16,
    b_left_trigger: u8,
    b_right_trigger: u8,
    s_thumb_lx: i16,
    s_thumb_ly: i16,
    s_thumb_rx: i16,
    s_thumb_ry: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct XInputState {
    dw_packet_number: u32,
    gamepad: XInputGamepad,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputKeystroke {
    virtual_key: u16,
    unicode: u16,
    flags: u16,
    user_index: u8,
    hid_code: u8,
}

type GetStateFn = unsafe extern "system" fn(u32, *mut XInputState) -> u32;
type SetStateFn = unsafe extern "system" fn(u32, *mut core::ffi::c_void) -> u32;
type GetKeystrokeFn = unsafe extern "system" fn(u32, u32, *mut XInputKeystroke) -> u32;
type GetCapsFn = unsafe extern "system" fn(u32, u32, *mut core::ffi::c_void) -> u32;
type EnableFn = unsafe extern "system" fn(i32);
type AudioFn = unsafe extern "system" fn(u32, *mut u16, *mut u32, *mut u16, *mut u32) -> u32;
type BatteryFn = unsafe extern "system" fn(u32, u8, *mut core::ffi::c_void) -> u32;
type CapsExFn = unsafe extern "system" fn(u32, u32, u32, *mut core::ffi::c_void) -> u32;
type DsoundFn = unsafe extern "system" fn(u32, *mut GUID, *mut GUID) -> u32;

struct XInputHooks {
    get_state: Option<DetourHook<GetStateFn>>,
    get_state_ex: Option<DetourHook<GetStateFn>>,
    set_state: Option<DetourHook<SetStateFn>>,
    get_keystroke: Option<DetourHook<GetKeystrokeFn>>,
    get_caps: Option<DetourHook<GetCapsFn>>,
    enable: Option<DetourHook<EnableFn>>,
    audio: Option<DetourHook<AudioFn>>,
    battery: Option<DetourHook<BatteryFn>>,
    caps_ex: Option<DetourHook<CapsExFn>>,
    dsound: Option<DetourHook<DsoundFn>>,
}

struct AllHooks {
    v11: XInputHooks,
    v12: XInputHooks,
    v13: XInputHooks,
    v14: XInputHooks,
}

struct VersionDetours {
    get_state: GetStateFn,
    get_state_ex: GetStateFn,
    set_state: SetStateFn,
    get_keystroke: GetKeystrokeFn,
    get_caps: GetCapsFn,
    enable: EnableFn,
    audio: AudioFn,
    battery: BatteryFn,
    caps_ex: CapsExFn,
    dsound: DsoundFn,
}

static HOOKS: OnceCell<AllHooks> = OnceCell::new();

fn mask_get_state(state: &mut XInputState, interactive: bool) {
    if !interactive {
        return;
    }
    state.gamepad.w_buttons = 0;
    state.gamepad.b_left_trigger = 0;
    state.gamepad.b_right_trigger = 0;
    state.gamepad.s_thumb_lx = 0;
    state.gamepad.s_thumb_ly = 0;
    state.gamepad.s_thumb_rx = 0;
    state.gamepad.s_thumb_ry = 0;
}

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn hooks(ver: u32) -> Option<&'static XInputHooks> {
    let all = HOOKS.wait();
    match ver {
        VER_11 => Some(&all.v11),
        VER_12 => Some(&all.v12),
        VER_13 => Some(&all.v13),
        VER_14 => Some(&all.v14),
        _ => None,
    }
}

fn apply_get_state(orig: Option<GetStateFn>, user: u32, state: *mut XInputState) -> u32 {
    let Some(orig) = orig else {
        return ERROR_DEVICE_NOT_CONNECTED;
    };
    let hr = unsafe { orig(user, state) };
    if hr == ERROR_SUCCESS && !state.is_null() {
        unsafe { mask_get_state(&mut *state, is_interactive()) };
    }
    hr
}

fn apply_set_state(orig: Option<SetStateFn>, user: u32, vibration: *mut core::ffi::c_void) -> u32 {
    if is_interactive() {
        return ERROR_SUCCESS;
    }
    orig.map(|f| unsafe { f(user, vibration) })
        .unwrap_or(ERROR_DEVICE_NOT_CONNECTED)
}

fn apply_get_keystroke(
    orig: Option<GetKeystrokeFn>,
    user: u32,
    reserved: u32,
    keystroke: *mut XInputKeystroke,
) -> u32 {
    if is_interactive() {
        let _ = orig.map(|f| unsafe { f(user, reserved, keystroke) });
        if !keystroke.is_null() {
            unsafe { *keystroke = XInputKeystroke::default() };
        }
        return ERROR_EMPTY;
    }
    orig.map(|f| unsafe { f(user, reserved, keystroke) })
        .unwrap_or(ERROR_EMPTY)
}

fn orig_get_state(ver: u32) -> Option<GetStateFn> {
    hooks(ver)
        .and_then(|h| h.get_state.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_get_state_ex(ver: u32) -> Option<GetStateFn> {
    hooks(ver)
        .and_then(|h| h.get_state_ex.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_set_state(ver: u32) -> Option<SetStateFn> {
    hooks(ver)
        .and_then(|h| h.set_state.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_get_keystroke(ver: u32) -> Option<GetKeystrokeFn> {
    hooks(ver)
        .and_then(|h| h.get_keystroke.as_ref())
        .map(DetourHook::original_fn)
}

fn apply_get_caps(
    orig: Option<GetCapsFn>,
    user: u32,
    flags: u32,
    caps: *mut core::ffi::c_void,
) -> u32 {
    orig.map(|f| unsafe { f(user, flags, caps) })
        .unwrap_or(ERROR_DEVICE_NOT_CONNECTED)
}

fn apply_enable(orig: Option<EnableFn>, enable: i32) {
    if let Some(f) = orig {
        unsafe { f(enable) };
    }
}

fn orig_get_caps(ver: u32) -> Option<GetCapsFn> {
    hooks(ver)
        .and_then(|h| h.get_caps.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_enable(ver: u32) -> Option<EnableFn> {
    hooks(ver)
        .and_then(|h| h.enable.as_ref())
        .map(DetourHook::original_fn)
}

fn apply_audio(
    orig: Option<AudioFn>,
    user: u32,
    render: *mut u16,
    render_count: *mut u32,
    capture: *mut u16,
    capture_count: *mut u32,
) -> u32 {
    orig.map(|f| unsafe { f(user, render, render_count, capture, capture_count) })
        .unwrap_or(ERROR_DEVICE_NOT_CONNECTED)
}

fn apply_battery(orig: Option<BatteryFn>, user: u32, dev: u8, info: *mut core::ffi::c_void) -> u32 {
    orig.map(|f| unsafe { f(user, dev, info) })
        .unwrap_or(ERROR_DEVICE_NOT_CONNECTED)
}

fn apply_caps_ex(
    orig: Option<CapsExFn>,
    reserved: u32,
    user: u32,
    flags: u32,
    caps: *mut core::ffi::c_void,
) -> u32 {
    orig.map(|f| unsafe { f(reserved, user, flags, caps) })
        .unwrap_or(ERROR_DEVICE_NOT_CONNECTED)
}

fn apply_dsound(orig: Option<DsoundFn>, user: u32, render: *mut GUID, capture: *mut GUID) -> u32 {
    orig.map(|f| unsafe { f(user, render, capture) })
        .unwrap_or(ERROR_DEVICE_NOT_CONNECTED)
}

fn orig_audio(ver: u32) -> Option<AudioFn> {
    hooks(ver)
        .and_then(|h| h.audio.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_battery(ver: u32) -> Option<BatteryFn> {
    hooks(ver)
        .and_then(|h| h.battery.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_caps_ex(ver: u32) -> Option<CapsExFn> {
    hooks(ver)
        .and_then(|h| h.caps_ex.as_ref())
        .map(DetourHook::original_fn)
}

fn orig_dsound(ver: u32) -> Option<DsoundFn> {
    hooks(ver)
        .and_then(|h| h.dsound.as_ref())
        .map(DetourHook::original_fn)
}

unsafe extern "system" fn hooked_get_state_13(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state(VER_13), user, state)
}
unsafe extern "system" fn hooked_get_state_14(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state(VER_14), user, state)
}
unsafe extern "system" fn hooked_get_state_ex_13(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state_ex(VER_13), user, state)
}
unsafe extern "system" fn hooked_get_state_ex_14(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state_ex(VER_14), user, state)
}
unsafe extern "system" fn hooked_set_state_13(user: u32, vibration: *mut core::ffi::c_void) -> u32 {
    apply_set_state(orig_set_state(VER_13), user, vibration)
}
unsafe extern "system" fn hooked_set_state_14(user: u32, vibration: *mut core::ffi::c_void) -> u32 {
    apply_set_state(orig_set_state(VER_14), user, vibration)
}
unsafe extern "system" fn hooked_get_keystroke_13(
    user: u32,
    reserved: u32,
    keystroke: *mut XInputKeystroke,
) -> u32 {
    apply_get_keystroke(orig_get_keystroke(VER_13), user, reserved, keystroke)
}
unsafe extern "system" fn hooked_get_keystroke_14(
    user: u32,
    reserved: u32,
    keystroke: *mut XInputKeystroke,
) -> u32 {
    apply_get_keystroke(orig_get_keystroke(VER_14), user, reserved, keystroke)
}
unsafe extern "system" fn hooked_get_caps_13(
    user: u32,
    flags: u32,
    caps: *mut core::ffi::c_void,
) -> u32 {
    apply_get_caps(orig_get_caps(VER_13), user, flags, caps)
}
unsafe extern "system" fn hooked_get_caps_14(
    user: u32,
    flags: u32,
    caps: *mut core::ffi::c_void,
) -> u32 {
    apply_get_caps(orig_get_caps(VER_14), user, flags, caps)
}
unsafe extern "system" fn hooked_enable_13(enable: i32) {
    apply_enable(orig_enable(VER_13), enable);
}
unsafe extern "system" fn hooked_enable_14(enable: i32) {
    apply_enable(orig_enable(VER_14), enable);
}

unsafe extern "system" fn hooked_get_state_11(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state(VER_11), user, state)
}
unsafe extern "system" fn hooked_get_state_12(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state(VER_12), user, state)
}
unsafe extern "system" fn hooked_get_state_ex_11(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state_ex(VER_11), user, state)
}
unsafe extern "system" fn hooked_get_state_ex_12(user: u32, state: *mut XInputState) -> u32 {
    apply_get_state(orig_get_state_ex(VER_12), user, state)
}
unsafe extern "system" fn hooked_set_state_11(user: u32, vibration: *mut core::ffi::c_void) -> u32 {
    apply_set_state(orig_set_state(VER_11), user, vibration)
}
unsafe extern "system" fn hooked_set_state_12(user: u32, vibration: *mut core::ffi::c_void) -> u32 {
    apply_set_state(orig_set_state(VER_12), user, vibration)
}
unsafe extern "system" fn hooked_get_keystroke_11(
    user: u32,
    reserved: u32,
    keystroke: *mut XInputKeystroke,
) -> u32 {
    apply_get_keystroke(orig_get_keystroke(VER_11), user, reserved, keystroke)
}
unsafe extern "system" fn hooked_get_keystroke_12(
    user: u32,
    reserved: u32,
    keystroke: *mut XInputKeystroke,
) -> u32 {
    apply_get_keystroke(orig_get_keystroke(VER_12), user, reserved, keystroke)
}
unsafe extern "system" fn hooked_get_caps_11(
    user: u32,
    flags: u32,
    caps: *mut core::ffi::c_void,
) -> u32 {
    apply_get_caps(orig_get_caps(VER_11), user, flags, caps)
}
unsafe extern "system" fn hooked_get_caps_12(
    user: u32,
    flags: u32,
    caps: *mut core::ffi::c_void,
) -> u32 {
    apply_get_caps(orig_get_caps(VER_12), user, flags, caps)
}
unsafe extern "system" fn hooked_enable_11(enable: i32) {
    apply_enable(orig_enable(VER_11), enable);
}
unsafe extern "system" fn hooked_enable_12(enable: i32) {
    apply_enable(orig_enable(VER_12), enable);
}

macro_rules! passthrough_audio {
    ($name:ident, $ver:expr) => {
        unsafe extern "system" fn $name(
            user: u32,
            render: *mut u16,
            render_count: *mut u32,
            capture: *mut u16,
            capture_count: *mut u32,
        ) -> u32 {
            apply_audio(
                orig_audio($ver),
                user,
                render,
                render_count,
                capture,
                capture_count,
            )
        }
    };
}
macro_rules! passthrough_battery {
    ($name:ident, $ver:expr) => {
        unsafe extern "system" fn $name(user: u32, dev: u8, info: *mut core::ffi::c_void) -> u32 {
            apply_battery(orig_battery($ver), user, dev, info)
        }
    };
}
macro_rules! passthrough_caps_ex {
    ($name:ident, $ver:expr) => {
        unsafe extern "system" fn $name(
            reserved: u32,
            user: u32,
            flags: u32,
            caps: *mut core::ffi::c_void,
        ) -> u32 {
            apply_caps_ex(orig_caps_ex($ver), reserved, user, flags, caps)
        }
    };
}
macro_rules! passthrough_dsound {
    ($name:ident, $ver:expr) => {
        unsafe extern "system" fn $name(user: u32, render: *mut GUID, capture: *mut GUID) -> u32 {
            apply_dsound(orig_dsound($ver), user, render, capture)
        }
    };
}

passthrough_audio!(hooked_audio_11, VER_11);
passthrough_audio!(hooked_audio_12, VER_12);
passthrough_audio!(hooked_audio_13, VER_13);
passthrough_audio!(hooked_audio_14, VER_14);
passthrough_battery!(hooked_battery_11, VER_11);
passthrough_battery!(hooked_battery_12, VER_12);
passthrough_battery!(hooked_battery_13, VER_13);
passthrough_battery!(hooked_battery_14, VER_14);
passthrough_caps_ex!(hooked_caps_ex_11, VER_11);
passthrough_caps_ex!(hooked_caps_ex_12, VER_12);
passthrough_caps_ex!(hooked_caps_ex_13, VER_13);
passthrough_caps_ex!(hooked_caps_ex_14, VER_14);
passthrough_dsound!(hooked_dsound_11, VER_11);
passthrough_dsound!(hooked_dsound_12, VER_12);
passthrough_dsound!(hooked_dsound_13, VER_13);
passthrough_dsound!(hooked_dsound_14, VER_14);

fn load_xinput(dll: PCSTR) -> Option<HMODULE> {
    unsafe { GetModuleHandleA(dll) }.ok()
}

fn attach_export<F: Copy + std::fmt::Debug>(
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

fn attach_get_state_ex(
    module: HMODULE,
    detour: GetStateFn,
    label: &str,
) -> Option<DetourHook<GetStateFn>> {
    let proc = unsafe {
        GetProcAddress(module, s!("XInputGetStateEx"))
            .or_else(|| GetProcAddress(module, PCSTR(GET_STATE_EX_ORDINAL as *const u8)))
    }?;
    let func: GetStateFn = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => Some(h),
        Err(err) => {
            warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

fn attach_ordinal<F: Copy + std::fmt::Debug>(
    module: HMODULE,
    ordinal: usize,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let proc = unsafe { GetProcAddress(module, PCSTR(ordinal as *const u8)) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => Some(h),
        Err(err) => {
            warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

fn empty_hooks() -> XInputHooks {
    XInputHooks {
        get_state: None,
        get_state_ex: None,
        set_state: None,
        get_keystroke: None,
        get_caps: None,
        enable: None,
        audio: None,
        battery: None,
        caps_ex: None,
        dsound: None,
    }
}

fn hook_version(dll: PCSTR, ver: u32, detours: VersionDetours) -> XInputHooks {
    let Some(module) = load_xinput(dll) else {
        return empty_hooks();
    };
    let get_state = attach_export(
        module,
        s!("XInputGetState"),
        detours.get_state,
        "XInputGetState",
    );
    if get_state.is_some() {
        info!("XInput Hooked XInputGetState Version {ver}");
    }
    XInputHooks {
        get_state,
        get_state_ex: attach_get_state_ex(module, detours.get_state_ex, "XInputGetStateEx"),
        set_state: attach_export(
            module,
            s!("XInputSetState"),
            detours.set_state,
            "XInputSetState",
        ),
        get_keystroke: attach_export(
            module,
            s!("XInputGetKeystroke"),
            detours.get_keystroke,
            "XInputGetKeystroke",
        ),
        get_caps: attach_export(
            module,
            s!("XInputGetCapabilities"),
            detours.get_caps,
            "XInputGetCapabilities",
        ),
        enable: attach_export(module, s!("XInputEnable"), detours.enable, "XInputEnable"),
        audio: attach_export(
            module,
            s!("XInputGetAudioDeviceIds"),
            detours.audio,
            "XInputGetAudioDeviceIds",
        ),
        battery: attach_export(
            module,
            s!("XInputGetBatteryInformation"),
            detours.battery,
            "XInputGetBatteryInformation",
        ),
        caps_ex: attach_ordinal(
            module,
            GET_CAPS_EX_ORDINAL,
            detours.caps_ex,
            "XInputGetCapabilitiesEx",
        ),
        dsound: attach_export(
            module,
            s!("XInputGetDSoundAudioDeviceGuids"),
            detours.dsound,
            "XInputGetDSoundAudioDeviceGuids",
        ),
    }
}

pub(super) fn hook() {
    let _ = HOOKS.set(AllHooks {
        v11: hook_version(
            s!("xinput1_1.dll"),
            VER_11,
            VersionDetours {
                get_state: hooked_get_state_11,
                get_state_ex: hooked_get_state_ex_11,
                set_state: hooked_set_state_11,
                get_keystroke: hooked_get_keystroke_11,
                get_caps: hooked_get_caps_11,
                enable: hooked_enable_11,
                audio: hooked_audio_11,
                battery: hooked_battery_11,
                caps_ex: hooked_caps_ex_11,
                dsound: hooked_dsound_11,
            },
        ),
        v12: hook_version(
            s!("xinput1_2.dll"),
            VER_12,
            VersionDetours {
                get_state: hooked_get_state_12,
                get_state_ex: hooked_get_state_ex_12,
                set_state: hooked_set_state_12,
                get_keystroke: hooked_get_keystroke_12,
                get_caps: hooked_get_caps_12,
                enable: hooked_enable_12,
                audio: hooked_audio_12,
                battery: hooked_battery_12,
                caps_ex: hooked_caps_ex_12,
                dsound: hooked_dsound_12,
            },
        ),
        v13: hook_version(
            s!("xinput1_3.dll"),
            VER_13,
            VersionDetours {
                get_state: hooked_get_state_13,
                get_state_ex: hooked_get_state_ex_13,
                set_state: hooked_set_state_13,
                get_keystroke: hooked_get_keystroke_13,
                get_caps: hooked_get_caps_13,
                enable: hooked_enable_13,
                audio: hooked_audio_13,
                battery: hooked_battery_13,
                caps_ex: hooked_caps_ex_13,
                dsound: hooked_dsound_13,
            },
        ),
        v14: hook_version(
            s!("xinput1_4.dll"),
            VER_14,
            VersionDetours {
                get_state: hooked_get_state_14,
                get_state_ex: hooked_get_state_ex_14,
                set_state: hooked_set_state_14,
                get_keystroke: hooked_get_keystroke_14,
                get_caps: hooked_get_caps_14,
                enable: hooked_enable_14,
                audio: hooked_audio_14,
                battery: hooked_battery_14,
                caps_ex: hooked_caps_ex_14,
                dsound: hooked_dsound_14,
            },
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed_state() -> XInputState {
        XInputState {
            dw_packet_number: 42,
            gamepad: XInputGamepad {
                w_buttons: 0x1001,
                b_left_trigger: 200,
                b_right_trigger: 180,
                s_thumb_lx: -12000,
                s_thumb_ly: 8000,
                s_thumb_rx: 4000,
                s_thumb_ry: -3000,
            },
        }
    }

    #[test]
    fn interactive_zeros_buttons_triggers_thumbs_keeps_packet() {
        let mut state = pressed_state();
        mask_get_state(&mut state, true);
        assert_eq!(state.dw_packet_number, 42);
        assert_eq!(state.gamepad, XInputGamepad::default());
    }

    #[test]
    fn not_interactive_leaves_state_unchanged() {
        let mut state = pressed_state();
        let before = state;
        mask_get_state(&mut state, false);
        assert_eq!(state, before);
    }

    #[test]
    fn missing_xinput_11_12_is_success() {
        assert!(load_xinput(s!("xinput1_1.dll")).is_none());
        assert!(load_xinput(s!("xinput1_2.dll")).is_none());
    }
}
