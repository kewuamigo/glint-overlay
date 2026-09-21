//! Low-level mouse/keyboard hooks used while overlay input blocking is active.
//!
//! Raw-input games may not pump `WM_MOUSE*` to the hooked WndProc. WH_MOUSE_LL /
//! WH_KEYBOARD_LL capture global input and forward it to the overlay host.
//!
//! These hooks OBSERVE only — they must always call `CallNextHookEx` and never
//! swallow events. Swallowing here freezes the system cursor and eats every
//! keystroke OS-wide (breaking Alt+Tab and the Shift+Tab escape hatch). Hiding
//! input from the game is handled separately by the message-loop filter and the
//! GetRawInputData/GetAsyncKeyState/GetCursorPos detours.
//!
//! Hook procs run on the thread that installed them (the game GUI thread). They
//! must never lock `WindowProcData` — use the lock-free route cache below.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use glint_overlay_event::input::{
    CursorAction, CursorEvent, CursorInputState, InputPosition, Key, KeyInputState, KeyboardInput,
    ScrollAxis,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, HC_ACTION, HHOOK,
        KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSLLHOOKSTRUCT, SetWindowsHookExW, UnhookWindowsHookEx,
        WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
        WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    },
};

use crate::{backend::window::input_event, event_sink::OverlayEventSink};

windows::core::link!("user32.dll" "system" fn ScreenToClient(hwnd: HWND, lpPoint: *mut POINT) -> windows::core::BOOL);

static ROUTE_HWND: AtomicU32 = AtomicU32::new(0);
static ROUTE_POS_X: AtomicI32 = AtomicI32::new(0);
static ROUTE_POS_Y: AtomicI32 = AtomicI32::new(0);
static BLOCKING_SESSIONS: AtomicUsize = AtomicUsize::new(0);
static MOUSE_HOOK: AtomicUsize = AtomicUsize::new(0);
static KEYBOARD_HOOK: AtomicUsize = AtomicUsize::new(0);
static LAST_MOVE_TICK: AtomicU64 = AtomicU64::new(0);
/// `KBDLLHOOKSTRUCT.time` of the last key transition this hook forwarded,
/// tagged with a validity bit (0 = never forwarded). Same tick domain as
/// `MSG.time` — MSDN documents `KBDLLHOOKSTRUCT.time` as "equivalent to what
/// `GetMessageTime` would return for this message".
static LAST_KEYBOARD_EMIT: AtomicU64 = AtomicU64::new(0);
const KEYBOARD_EMIT_SEEN: u64 = 1 << 32;

const MOVE_THROTTLE_MS: u64 = 8;

/// Publish overlay input route for `hwnd` (lock-free; safe from hook procs).
pub fn activate_route(hwnd: u32, position: (i32, i32)) {
    ROUTE_POS_X.store(position.0, Ordering::Release);
    ROUTE_POS_Y.store(position.1, Ordering::Release);
    ROUTE_HWND.store(hwnd, Ordering::Release);
}

/// Clear route when input blocking ends for `hwnd`.
pub fn deactivate_route(hwnd: u32) {
    let _ = ROUTE_HWND.compare_exchange(hwnd, 0, Ordering::AcqRel, Ordering::Acquire);
}

/// Whether this hook already forwarded the keystroke carried by a pump message
/// stamped `msg_time`.
///
/// The LL hook runs before the same keystroke is dequeued by the game's
/// message loop, so when the hook is alive its last-forwarded timestamp is
/// always >= the timestamp of any key message the pump is currently reading.
/// A stale (or never-set) timestamp means the hook is starved — some games
/// (observed in Alan Wake 2) stall WH_KEYBOARD_LL delivery entirely while
/// legacy WM_KEYDOWN still reaches their pump — and the pump must emit Key
/// events itself or overlay shortcuts (Ctrl+A, Backspace, arrows) go dead.
/// Whether this hook already forwarded the keystroke carried by a pump message
/// stamped `msg_time`. Kept for tests / diagnostics; the pump always emits Keys
/// while blocking now, and browser-host dedupes LL+pump duplicates.
#[cfg_attr(not(test), allow(dead_code))]
pub fn keyboard_ll_handled(msg_time: u32) -> bool {
    let v = LAST_KEYBOARD_EMIT.load(Ordering::Acquire);
    if v & KEYBOARD_EMIT_SEEN == 0 {
        return false;
    }
    // Wrapping compare: GetTickCount wraps every ~49.7 days.
    (v as u32).wrapping_sub(msg_time) as i32 >= 0
}

/// Update surface offset while blocking (called from layout recompute).
pub fn update_route_position(position: (i32, i32)) {
    if ROUTE_HWND.load(Ordering::Acquire) != 0 {
        ROUTE_POS_X.store(position.0, Ordering::Release);
        ROUTE_POS_Y.store(position.1, Ordering::Release);
    }
}

/// Begin routing global input through low-level hooks.
pub fn acquire() {
    if BLOCKING_SESSIONS.fetch_add(1, Ordering::SeqCst) == 0 {
        install();
    }
}

/// Stop low-level routing when no window is blocking input.
pub fn release() {
    if BLOCKING_SESSIONS.fetch_sub(1, Ordering::SeqCst) == 1 {
        uninstall();
    }
}

fn install() {
    unsafe {
        if let Ok(hook) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) {
            MOUSE_HOOK.store(hook.0 as usize, Ordering::Release);
        }
        if let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) {
            KEYBOARD_HOOK.store(hook.0 as usize, Ordering::Release);
        }
    }
}

fn uninstall() {
    unsafe {
        let h = MOUSE_HOOK.swap(0, Ordering::AcqRel);
        if h != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
        }
        let h = KEYBOARD_HOOK.swap(0, Ordering::AcqRel);
        if h != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
        }
    }
}

fn blocking_target() -> Option<(u32, (i32, i32))> {
    let id = ROUTE_HWND.load(Ordering::Acquire);
    if id == 0 {
        return None;
    }

    // Route input while the game is in the foreground. Match by process, not
    // by exact window handle: after Alt+Tab (or when a game uses separate
    // owner/render windows) the foreground handle often differs from the
    // routed one, and an exact-handle gate would silently drop all overlay
    // input until the original handle regains foreground.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return None;
        }
        if fg.0 as u32 != id {
            let mut fg_pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut fg_pid));
            if fg_pid != GetCurrentProcessId() {
                return None;
            }
        }
    }

    let pos = (
        ROUTE_POS_X.load(Ordering::Acquire),
        ROUTE_POS_Y.load(Ordering::Acquire),
    );
    Some((id, pos))
}

fn screen_to_window(hwnd: u32, screen_x: i32, screen_y: i32) -> InputPosition {
    unsafe {
        let mut pt = POINT {
            x: screen_x,
            y: screen_y,
        };
        let _ = ScreenToClient(HWND(hwnd as _), &mut pt);
        InputPosition { x: pt.x, y: pt.y }
    }
}

fn emit_cursor(id: u32, position: (i32, i32), window: InputPosition, event: CursorEvent) {
    OverlayEventSink::emit(input_event::cursor_overlay_event(
        id, position, window, event,
    ));
}

/// LL keyboard hooks report side-specific modifier codes (VK_LSHIFT, ...),
/// while the WndProc path reports the generic ones (VK_SHIFT, ...). Normalize
/// so consumers (e.g. the Shift+Tab toggle in the host) see one encoding.
fn normalize_vk(vk: u32) -> u8 {
    match vk {
        0xA0 | 0xA1 => 0x10, // VK_LSHIFT / VK_RSHIFT -> VK_SHIFT
        0xA2 | 0xA3 => 0x11, // VK_LCONTROL / VK_RCONTROL -> VK_CONTROL
        0xA4 | 0xA5 => 0x12, // VK_LMENU / VK_RMENU -> VK_MENU
        v => v as u8,
    }
}

fn should_emit_move() -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev = LAST_MOVE_TICK.load(Ordering::Acquire);
    if now.saturating_sub(prev) < MOVE_THROTTLE_MS {
        return false;
    }
    LAST_MOVE_TICK.store(now, Ordering::Release);
    true
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some((id, position)) = blocking_target() {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            let window = screen_to_window(id, info.pt.x, info.pt.y);
            let msg = wparam.0 as u32;

            match msg {
                WM_MOUSEMOVE if should_emit_move() => {
                    emit_cursor(id, position, window, CursorEvent::Move);
                }
                WM_LBUTTONDOWN => {
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Action {
                            action: CursorAction::Left,
                            state: CursorInputState::Pressed {
                                double_click: false,
                            },
                        },
                    );
                }
                WM_LBUTTONUP => {
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Action {
                            action: CursorAction::Left,
                            state: CursorInputState::Released,
                        },
                    );
                }
                WM_RBUTTONDOWN => {
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Action {
                            action: CursorAction::Right,
                            state: CursorInputState::Pressed {
                                double_click: false,
                            },
                        },
                    );
                }
                WM_RBUTTONUP => {
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Action {
                            action: CursorAction::Right,
                            state: CursorInputState::Released,
                        },
                    );
                }
                WM_MBUTTONDOWN => {
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Action {
                            action: CursorAction::Middle,
                            state: CursorInputState::Pressed {
                                double_click: false,
                            },
                        },
                    );
                }
                WM_MBUTTONUP => {
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Action {
                            action: CursorAction::Middle,
                            state: CursorInputState::Released,
                        },
                    );
                }
                WM_MOUSEWHEEL => {
                    let delta = ((info.mouseData >> 16) & 0xffff) as i16;
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Scroll {
                            axis: ScrollAxis::Y,
                            delta,
                        },
                    );
                }
                WM_MOUSEHWHEEL => {
                    let delta = ((info.mouseData >> 16) & 0xffff) as i16;
                    emit_cursor(
                        id,
                        position,
                        window,
                        CursorEvent::Scroll {
                            axis: ScrollAxis::X,
                            delta,
                        },
                    );
                }
                _ => {}
            }
        }
    }

    let hook = MOUSE_HOOK.load(Ordering::Acquire);
    let next = if hook != 0 {
        Some(HHOOK(hook as *mut _))
    } else {
        None
    };
    unsafe { CallNextHookEx(next, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some((id, _)) = blocking_target() {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let pressed = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let released = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);

            if pressed || released {
                if let Some(key) = Key::new(
                    normalize_vk(info.vkCode),
                    info.flags.contains(LLKHF_EXTENDED),
                ) {
                    // Stamp for diagnostics / future pump coordination. Pump
                    // always emits Keys while blocking; browser-host dedupes.
                    LAST_KEYBOARD_EMIT
                        .store(KEYBOARD_EMIT_SEEN | info.time as u64, Ordering::Release);
                    OverlayEventSink::emit(input_event::keyboard_overlay_event(
                        id,
                        KeyboardInput::Key {
                            key,
                            state: if pressed {
                                KeyInputState::Pressed
                            } else {
                                KeyInputState::Released
                            },
                        },
                    ));
                }
            }
        }
    }

    let hook = KEYBOARD_HOOK.load(Ordering::Acquire);
    let next = if hook != 0 {
        Some(HHOOK(hook as *mut _))
    } else {
        None
    };
    unsafe { CallNextHookEx(next, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: Alan Wake 2 starves WH_KEYBOARD_LL, so the pump must emit
    /// Key events whenever the hook has not already handled the keystroke.
    /// (The full pipeline needs a live game pump + OS hook and has no test
    /// seam; this locks down the dedupe predicate the fallback relies on.)
    #[test]
    fn keyboard_ll_handled_predicate() {
        // Hook never fired (LAST_KEYBOARD_EMIT starts at 0): pump must emit.
        assert!(!keyboard_ll_handled(12345));

        // Hook forwarded a keystroke at t=1000: the same keystroke (equal
        // stamp) and older ones are handled; a newer keystroke is not.
        LAST_KEYBOARD_EMIT.store(KEYBOARD_EMIT_SEEN | 1000, Ordering::Release);
        assert!(keyboard_ll_handled(1000));
        assert!(keyboard_ll_handled(999));
        assert!(!keyboard_ll_handled(1001));

        // GetTickCount wraparound: hook stamp just past the wrap still covers
        // a pump message stamped just before it.
        LAST_KEYBOARD_EMIT.store(KEYBOARD_EMIT_SEEN | 5, Ordering::Release);
        assert!(keyboard_ll_handled(u32::MAX - 5));
        assert!(!keyboard_ll_handled(500));
    }
}
