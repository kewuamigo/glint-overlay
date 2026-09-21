//! Thread-local `WH_GETMESSAGE` + `WH_CALLWNDPROC` while Interactive.
//!
//! Steam: show `sub_1800A5AC0` / hide `sub_1800A55F0`; GETMESSAGE `0x1800A6260`;
//! CALLWNDPROC `sub_1800A5510`; watchdog `sub_1800A6940` (500 ms).

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use tracing::{info, warn};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        CWPSTRUCT, CallNextHookEx, HHOOK, MSG, SetWindowsHookExW, UnhookWindowsHookEx,
        WH_CALLWNDPROC, WH_GETMESSAGE, WM_IME_NOTIFY, WM_IME_REQUEST, WM_NULL,
    },
};

use crate::hook::{on_message_read, should_consume_pump_message, translate_keydown_for_char};

/// Documented per-process skip. Empty default — do not invent a blacklist.
const SKIP_EXE_NAMES: &[&str] = &[];

const GETMESSAGE_WATCHDOG_MS: u32 = 500;

static GETMESSAGE_HOOK: AtomicUsize = AtomicUsize::new(0);
static CALLWNDPROC_HOOK: AtomicUsize = AtomicUsize::new(0);
static GAME_HWND: AtomicU32 = AtomicU32::new(0);
static LAST_GETMESSAGE_TICK: AtomicU32 = AtomicU32::new(0);
static WATCHDOG_TID: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GetMessageResult {
    CallNext,
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallWndProcResult {
    CallNext,
    Block,
}

fn apply_getmessage(n_code: i32, message: u32, interactive: bool) -> GetMessageResult {
    if n_code < 0 {
        return GetMessageResult::CallNext;
    }
    if should_consume_pump_message(message, interactive) {
        GetMessageResult::Consumed
    } else {
        GetMessageResult::CallNext
    }
}

/// Emit on the live `MSG`, then `WM_NULL`. Overlay host must see the key first.
fn consume_overlay_getmessage(
    n_code: i32,
    msg: &mut MSG,
    interactive: bool,
    emit: impl FnOnce(&MSG),
) -> GetMessageResult {
    if apply_getmessage(n_code, msg.message, interactive) != GetMessageResult::Consumed {
        return GetMessageResult::CallNext;
    }
    emit(msg);
    msg.message = WM_NULL;
    GetMessageResult::Consumed
}

fn apply_callwndproc(n_code: i32, hwnd: u32, game_hwnd: u32, message: u32) -> CallWndProcResult {
    if n_code < 0 || hwnd != game_hwnd {
        return CallWndProcResult::CallNext;
    }
    if is_callwndproc_ime(message) {
        CallWndProcResult::Block
    } else {
        CallWndProcResult::CallNext
    }
}

/// Steam `sub_1800A5510`: `WM_IME_NOTIFY` / `WM_IME_REQUEST` / 269..=271.
fn is_callwndproc_ime(message: u32) -> bool {
    message == WM_IME_NOTIFY || message == WM_IME_REQUEST || (269..=271).contains(&message)
}

fn should_install_thread_hooks(interactive: bool, skip: bool) -> bool {
    interactive && !skip
}

fn watchdog_should_reinstall(getmessage_installed: bool, tick_delta_ms: u32) -> bool {
    getmessage_installed && tick_delta_ms > GETMESSAGE_WATCHDOG_MS
}

fn skip_thread_hooks_decision(env_skip: bool, exe_name: &str) -> bool {
    env_skip
        || SKIP_EXE_NAMES
            .iter()
            .any(|n| n.eq_ignore_ascii_case(exe_name))
}

fn now_ms() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0)
}

fn skip_thread_hooks() -> bool {
    let env_skip = std::env::var_os("GLINT_SKIP_THREAD_HOOKS").is_some_and(|v| v == "1");
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    skip_thread_hooks_decision(env_skip, &exe)
}

/// Install on the game GUI thread (`GetCurrentThreadId` is the tid).
pub(crate) fn install(hwnd: u32) {
    if !should_install_thread_hooks(true, skip_thread_hooks()) {
        info!("Not using SetWindowsHookExW for this process");
        return;
    }
    if GETMESSAGE_HOOK.load(Ordering::Acquire) != 0 || CALLWNDPROC_HOOK.load(Ordering::Acquire) != 0
    {
        return;
    }

    GAME_HWND.store(hwnd, Ordering::Release);
    let tid = unsafe { GetCurrentThreadId() };
    WATCHDOG_TID.store(tid, Ordering::Release);
    LAST_GETMESSAGE_TICK.store(now_ms(), Ordering::Release);

    unsafe {
        match SetWindowsHookExW(WH_GETMESSAGE, Some(getmessage_proc), None, tid) {
            Ok(hook) => GETMESSAGE_HOOK.store(hook.0 as usize, Ordering::Release),
            Err(err) => warn!("Failed SetWindowsHookExW(WH_GETMESSAGE): {err:?}"),
        }
        match SetWindowsHookExW(WH_CALLWNDPROC, Some(callwndproc_proc), None, tid) {
            Ok(hook) => CALLWNDPROC_HOOK.store(hook.0 as usize, Ordering::Release),
            Err(err) => warn!("Failed SetWindowsHookExW(WH_CALLWNDPROC): {err:?}"),
        }
    }
}

pub(crate) fn uninstall() {
    GAME_HWND.store(0, Ordering::Release);
    unhook(&GETMESSAGE_HOOK);
    unhook(&CALLWNDPROC_HOOK);
    WATCHDOG_TID.store(0, Ordering::Release);
    LAST_GETMESSAGE_TICK.store(0, Ordering::Release);
}

fn unhook(slot: &AtomicUsize) {
    let h = slot.swap(0, Ordering::AcqRel);
    if h != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
        }
    }
}

/// Tick from the Get/Peek detour so Peek-only loops still reinstall within 500 ms.
pub(crate) fn tick_watchdog() {
    let installed = GETMESSAGE_HOOK.load(Ordering::Acquire) != 0;
    let delta = now_ms().wrapping_sub(LAST_GETMESSAGE_TICK.load(Ordering::Acquire));
    if watchdog_should_reinstall(installed, delta) {
        warn!("Reseting get message hook because we think it may have died");
        reinstall_getmessage();
    }
}

fn reinstall_getmessage() {
    unhook(&GETMESSAGE_HOOK);
    let tid = WATCHDOG_TID.load(Ordering::Acquire);
    if tid == 0 {
        return;
    }
    LAST_GETMESSAGE_TICK.store(now_ms(), Ordering::Release);
    unsafe {
        match SetWindowsHookExW(WH_GETMESSAGE, Some(getmessage_proc), None, tid) {
            Ok(hook) => GETMESSAGE_HOOK.store(hook.0 as usize, Ordering::Release),
            Err(err) => warn!("Failed SetWindowsHookExW(WH_GETMESSAGE) reinstall: {err:?}"),
        }
    }
}

fn call_next(slot: &AtomicUsize, code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let hook = slot.load(Ordering::Acquire);
    let next = if hook != 0 {
        Some(HHOOK(hook as *mut _))
    } else {
        None
    };
    unsafe { CallNextHookEx(next, code, wparam, lparam) }
}

unsafe extern "system" fn getmessage_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    LAST_GETMESSAGE_TICK.store(now_ms(), Ordering::Release);

    if lparam.0 != 0 {
        let msg = unsafe { &mut *(lparam.0 as *mut MSG) };
        let interactive = GAME_HWND.load(Ordering::Acquire) != 0;
        let removed = wparam.0 != 0;
        if consume_overlay_getmessage(code, msg, interactive, |msg| {
            if removed {
                on_message_read(msg);
                translate_keydown_for_char(msg);
            }
        }) == GetMessageResult::Consumed
        {
            return LRESULT(0);
        }
    }

    call_next(&GETMESSAGE_HOOK, code, wparam, lparam)
}

unsafe extern "system" fn callwndproc_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if lparam.0 != 0 {
        let cwp = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        let game = GAME_HWND.load(Ordering::Acquire);
        if apply_callwndproc(code, cwp.hwnd.0 as u32, game, cwp.message) == CallWndProcResult::Block
        {
            return LRESULT(1);
        }
    }

    call_next(&CALLWNDPROC_HOOK, code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, POINT, WPARAM},
        UI::WindowsAndMessaging::{
            WM_IME_COMPOSITION, WM_IME_NOTIFY, WM_IME_REQUEST, WM_IME_STARTCOMPOSITION, WM_KEYDOWN,
            WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NULL, WM_PAINT,
        },
    };

    const GAME: u32 = 0x100;
    const OTHER: u32 = 0x200;

    fn fake_msg(message: u32) -> MSG {
        MSG {
            hwnd: HWND(core::ptr::null_mut()),
            message,
            wParam: WPARAM(0),
            lParam: LPARAM(0),
            time: 0,
            pt: POINT { x: 0, y: 0 },
        }
    }

    #[test]
    fn getmessage_consume_keydown_is_wm_null_return_0() {
        let mut msg = fake_msg(WM_KEYDOWN);
        assert_eq!(
            consume_overlay_getmessage(0, &mut msg, true, |_| {}),
            GetMessageResult::Consumed
        );
        assert_eq!(msg.message, WM_NULL);
    }

    #[test]
    fn getmessage_consume_lbutton_is_wm_null_return_0() {
        let mut msg = fake_msg(WM_LBUTTONDOWN);
        assert_eq!(
            consume_overlay_getmessage(0, &mut msg, true, |_| {}),
            GetMessageResult::Consumed
        );
        assert_eq!(msg.message, WM_NULL);
    }

    #[test]
    fn getmessage_consume_emits_before_null() {
        let mut msg = fake_msg(WM_KEYDOWN);
        let mut seen = None;
        assert_eq!(
            consume_overlay_getmessage(0, &mut msg, true, |m| seen = Some(m.message)),
            GetMessageResult::Consumed
        );
        assert_eq!(seen, Some(WM_KEYDOWN));
        assert_eq!(msg.message, WM_NULL);
    }

    #[test]
    fn getmessage_unrelated_or_inactive_calls_next() {
        assert_eq!(
            apply_getmessage(0, WM_PAINT, true),
            GetMessageResult::CallNext
        );
        assert_eq!(
            apply_getmessage(0, WM_KEYDOWN, false),
            GetMessageResult::CallNext
        );
        assert_eq!(
            apply_getmessage(-1, WM_KEYDOWN, true),
            GetMessageResult::CallNext
        );
    }

    #[test]
    fn callwndproc_ime_on_game_hwnd_blocks() {
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_IME_NOTIFY),
            CallWndProcResult::Block
        );
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_IME_REQUEST),
            CallWndProcResult::Block
        );
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_IME_STARTCOMPOSITION),
            CallWndProcResult::Block
        );
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_IME_COMPOSITION),
            CallWndProcResult::Block
        );
    }

    #[test]
    fn callwndproc_mouse_key_paint_pass_through() {
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_KEYDOWN),
            CallWndProcResult::CallNext
        );
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_LBUTTONDOWN),
            CallWndProcResult::CallNext
        );
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_MOUSEMOVE),
            CallWndProcResult::CallNext
        );
        assert_eq!(
            apply_callwndproc(0, GAME, GAME, WM_PAINT),
            CallWndProcResult::CallNext
        );
        assert_eq!(
            apply_callwndproc(0, OTHER, GAME, WM_IME_NOTIFY),
            CallWndProcResult::CallNext
        );
        assert_eq!(
            apply_callwndproc(-1, GAME, GAME, WM_IME_NOTIFY),
            CallWndProcResult::CallNext
        );
    }

    #[test]
    fn hooks_off_when_not_interactive() {
        assert!(!should_install_thread_hooks(false, false));
        assert!(should_install_thread_hooks(true, false));
        assert!(!should_install_thread_hooks(true, true));
    }

    #[test]
    fn watchdog_reinstalls_when_delta_over_500() {
        assert!(watchdog_should_reinstall(true, 501));
        assert!(!watchdog_should_reinstall(true, 500));
        assert!(!watchdog_should_reinstall(true, 0));
        assert!(!watchdog_should_reinstall(false, 1000));
    }

    #[test]
    fn skip_only_env_or_documented_exe() {
        assert!(!skip_thread_hooks_decision(false, "game.exe"));
        assert!(skip_thread_hooks_decision(true, "game.exe"));
        assert!(
            !SKIP_EXE_NAMES
                .iter()
                .any(|n| n.eq_ignore_ascii_case("game.exe"))
        );
    }
}
