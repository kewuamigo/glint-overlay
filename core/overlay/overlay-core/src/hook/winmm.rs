//! winmm joystick consume (Steam `sub_1800CEAC0` / `sub_1800CF890` / `sub_1800CF8B0`).
//!
//! `GetModuleHandle` only. Missing DLL is success. Interactive: `joyGetNumDevs`
//! → 4; `joyGetPosEx` → centered `0x7FFF`, buttons 0, POV `0xFFFF` when
//! requested. Caps → original. No JOYINFOEX-from-overlay-pad.

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

const JOY_RETURNX: u32 = 0x1;
const JOY_RETURNY: u32 = 0x2;
const JOY_RETURNZ: u32 = 0x4;
const JOY_RETURNR: u32 = 0x8;
const JOY_RETURNU: u32 = 0x10;
const JOY_RETURNV: u32 = 0x20;
const JOY_RETURNPOV: u32 = 0x40;
const JOY_RETURNPOVCTS: u32 = 0x200;
const JOY_RETURNCENTERED: u32 = 0x400;
const AXIS_CENTER: u32 = 0x7FFF;
const POV_CENTERED: u32 = 0xFFFF;
const JOYERR_NOERROR: u32 = 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct JoyInfoEx {
    size: u32,
    flags: u32,
    x: u32,
    y: u32,
    z: u32,
    r: u32,
    u: u32,
    v: u32,
    buttons: u32,
    button_number: u32,
    pov: u32,
    reserved1: u32,
    reserved2: u32,
}

type NumDevsFn = unsafe extern "system" fn() -> u32;
type CapsFn = unsafe extern "system" fn(usize, *mut core::ffi::c_void, u32) -> u32;
type PosExFn = unsafe extern "system" fn(u32, *mut JoyInfoEx) -> u32;

struct Hooks {
    num_devs: Option<DetourHook<NumDevsFn>>,
    caps_a: Option<DetourHook<CapsFn>>,
    caps_w: Option<DetourHook<CapsFn>>,
    pos_ex: Option<DetourHook<PosExFn>>,
}

impl Hooks {
    fn empty() -> Self {
        Self {
            num_devs: None,
            caps_a: None,
            caps_w: None,
            pos_ex: None,
        }
    }

    fn any_attached(&self) -> bool {
        self.num_devs.is_some()
            || self.caps_a.is_some()
            || self.caps_w.is_some()
            || self.pos_ex.is_some()
    }
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("winmm.dll")) }.ok()
}

fn fill_centered(flags: u32, out: &mut JoyInfoEx) {
    let all = flags == JOY_RETURNCENTERED;
    if all || flags & JOY_RETURNX != 0 {
        out.x = AXIS_CENTER;
    }
    if all || flags & JOY_RETURNY != 0 {
        out.y = AXIS_CENTER;
    }
    if all || flags & JOY_RETURNZ != 0 {
        out.z = AXIS_CENTER;
    }
    if all || flags & JOY_RETURNR != 0 {
        out.r = AXIS_CENTER;
    }
    if all || flags & JOY_RETURNU != 0 {
        out.u = AXIS_CENTER;
    }
    if all || flags & JOY_RETURNV != 0 {
        out.v = AXIS_CENTER;
    }
    out.buttons = 0;
    out.button_number = 0;
    if all || flags & (JOY_RETURNPOV | JOY_RETURNPOVCTS) != 0 {
        out.pov = POV_CENTERED;
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

unsafe extern "system" fn hooked_num_devs() -> u32 {
    if is_interactive() {
        return 4;
    }
    HOOKS
        .wait()
        .num_devs
        .as_ref()
        .map(|h| unsafe { h.original_fn()() })
        .unwrap_or(0)
}

unsafe extern "system" fn hooked_caps_a(id: usize, caps: *mut core::ffi::c_void, size: u32) -> u32 {
    let orig = HOOKS.wait().caps_a.as_ref().unwrap().original_fn();
    unsafe { orig(id, caps, size) }
}

unsafe extern "system" fn hooked_caps_w(id: usize, caps: *mut core::ffi::c_void, size: u32) -> u32 {
    let orig = HOOKS.wait().caps_w.as_ref().unwrap().original_fn();
    unsafe { orig(id, caps, size) }
}

unsafe extern "system" fn hooked_pos_ex(id: u32, info: *mut JoyInfoEx) -> u32 {
    let orig = HOOKS.wait().pos_ex.as_ref().unwrap().original_fn();
    if is_interactive() {
        if info.is_null() {
            return 11;
        }
        let out = unsafe { &mut *info };
        fill_centered(out.flags, out);
        return JOYERR_NOERROR;
    }
    unsafe { orig(id, info) }
}

pub(super) fn hook() {
    let Some(module) = module() else {
        let _ = HOOKS.set(Hooks::empty());
        return;
    };
    let _ = HOOKS.set(Hooks {
        num_devs: attach(
            module,
            s!("joyGetNumDevs"),
            hooked_num_devs,
            "joyGetNumDevs",
        ),
        caps_a: attach(
            module,
            s!("joyGetDevCapsA"),
            hooked_caps_a,
            "joyGetDevCapsA",
        ),
        caps_w: attach(
            module,
            s!("joyGetDevCapsW"),
            hooked_caps_w,
            "joyGetDevCapsW",
        ),
        pos_ex: attach(module, s!("joyGetPosEx"), hooked_pos_ex, "joyGetPosEx"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_winmm_is_ok() {
        if module().is_none() {
            hook();
            assert!(!HOOKS.get().expect("empty set").any_attached());
        }
    }

    #[test]
    fn interactive_pos_ex_is_centered_empty_buttons() {
        let mut info = JoyInfoEx {
            size: 52,
            flags: JOY_RETURNX | JOY_RETURNY | JOY_RETURNZ | JOY_RETURNPOV | 0x80,
            x: 1,
            y: 2,
            z: 3,
            buttons: 0xF,
            button_number: 2,
            pov: 0,
            ..JoyInfoEx::default()
        };
        fill_centered(info.flags, &mut info);
        assert_eq!(info.x, AXIS_CENTER);
        assert_eq!(info.y, AXIS_CENTER);
        assert_eq!(info.z, AXIS_CENTER);
        assert_eq!(info.buttons, 0);
        assert_eq!(info.button_number, 0);
        assert_eq!(info.pov, POV_CENTERED);
    }

    #[test]
    fn centered_flag_fills_all_axes_and_pov() {
        let mut info = JoyInfoEx {
            flags: JOY_RETURNCENTERED,
            ..JoyInfoEx::default()
        };
        fill_centered(info.flags, &mut info);
        assert_eq!(info.x, AXIS_CENTER);
        assert_eq!(info.r, AXIS_CENTER);
        assert_eq!(info.pov, POV_CENTERED);
        assert_eq!(info.buttons, 0);
    }
}
