mod input;

pub(crate) use input::with_cursor_passthrough;

use core::cell::Cell;
use glint_overlay_event::{
    OverlayEvent, WindowEvent,
    input::{CursorAction, CursorInput, InputEvent, Key, KeyInputState, KeyboardInput, ScrollAxis},
};
use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use scopeguard::defer;
use tracing::{debug, trace, warn};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK, MapVirtualKeyA},
            WindowsAndMessaging::{
                self as msg, CallWindowProcA, CallWindowProcW, GA_ROOT, GetAncestor, MSG,
                PEEK_MESSAGE_REMOVE_TYPE, PM_REMOVE, TranslateMessage,
            },
        },
    },
    core::BOOL,
};

use crate::{
    backend::{Backends, WindowBackend, window::WindowProcData},
    event_sink::OverlayEventSink,
};

windows::core::link!("user32.dll" "system" fn GetMessageA(lpmsg: *mut MSG, hwnd: HWND, wmsgfiltermin: u32, wmsgfiltermax: u32) -> BOOL);
windows::core::link!("user32.dll" "system" fn GetMessageW(lpmsg: *mut MSG, hwnd: HWND, wmsgfiltermin: u32, wmsgfiltermax: u32) -> BOOL);

windows::core::link!("user32.dll" "system" fn PeekMessageA(
    lpmsg: *mut MSG,
    hwnd: HWND,
    wmsgfiltermin: u32,
    wmsgfiltermax: u32,
    wremovemsg: PEEK_MESSAGE_REMOVE_TYPE,
) -> BOOL);
windows::core::link!("user32.dll" "system" fn PeekMessageW(
    lpmsg: *mut MSG,
    hwnd: HWND,
    wmsgfiltermin: u32,
    wmsgfiltermax: u32,
    wremovemsg: PEEK_MESSAGE_REMOVE_TYPE,
) -> BOOL);
windows::core::link!("user32.dll" "system" fn DefWindowProcA(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT);
windows::core::link!("user32.dll" "system" fn DefWindowProcW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT);
windows::core::link!("user32.dll" "system" fn DispatchMessageA(lpmsg: *const MSG) -> LRESULT);
windows::core::link!("user32.dll" "system" fn DispatchMessageW(lpmsg: *const MSG) -> LRESULT);

struct Hook {
    get_message_a: DetourHook<GetMessageFn>,
    get_message_w: DetourHook<GetMessageFn>,

    peek_message_a: DetourHook<PeekMessageFn>,
    peek_message_w: DetourHook<PeekMessageFn>,
    dispatch_message_a: Option<DetourHook<DispatchMessageFn>>,
    dispatch_message_w: Option<DetourHook<DispatchMessageFn>>,
}

static HOOK: OnceCell<Hook> = OnceCell::new();

type GetMessageFn = unsafe extern "system" fn(*mut MSG, HWND, u32, u32) -> BOOL;
type PeekMessageFn =
    unsafe extern "system" fn(*mut MSG, HWND, u32, u32, PEEK_MESSAGE_REMOVE_TYPE) -> BOOL;
type DispatchMessageFn = unsafe extern "system" fn(*const MSG) -> LRESULT;

fn attach_soft<F: Copy + std::fmt::Debug>(
    name: &'static str,
    func: F,
    detour: F,
) -> Option<DetourHook<F>> {
    debug!("hooking {name}");
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(hook) => Some(hook),
        Err(err) => {
            warn!("Failed hooking {name}(): {err:?}");
            None
        }
    }
}

pub fn hook() -> anyhow::Result<()> {
    input::hook()?;

    HOOK.get_or_try_init(|| unsafe {
        debug!("hooking GetMessageA");
        let get_message_a = DetourHook::attach(GetMessageA as _, hooked_get_message_a as _)?;

        debug!("hooking GetMessageW");
        let get_message_w = DetourHook::attach(GetMessageW as _, hooked_get_message_w as _)?;

        debug!("hooking PeekMessageA");
        let peek_message_a = DetourHook::attach(PeekMessageA as _, hooked_peek_message_a as _)?;

        debug!("hooking PeekMessageW");
        let peek_message_w = DetourHook::attach(PeekMessageW as _, hooked_peek_message_w as _)?;

        let dispatch_message_a = attach_soft(
            "DispatchMessageA",
            DispatchMessageA as _,
            hooked_dispatch_message_a as _,
        );
        let dispatch_message_w = attach_soft(
            "DispatchMessageW",
            DispatchMessageW as _,
            hooked_dispatch_message_w as _,
        );

        Ok::<_, anyhow::Error>(Hook {
            get_message_a,
            get_message_w,

            peek_message_a,
            peek_message_w,
            dispatch_message_a,
            dispatch_message_w,
        })
    })?;

    Ok(())
}

thread_local! {
    static MESSAGE_READING: Cell<bool> = const { Cell::new(false) };
}

#[inline]
fn message_reading() -> bool {
    MESSAGE_READING.get()
}

#[inline]
fn set_message_read<R>(f: impl FnOnce() -> R) -> R {
    let last = MESSAGE_READING.replace(true);
    defer!(MESSAGE_READING.set(last));
    f()
}

fn process_read_message<const UNICODE: bool>(
    msg: &mut MSG,
    reader: impl Fn(&mut MSG) -> bool,
) -> bool {
    crate::backend::window::thread_hooks::tick_watchdog();
    if !reader(msg) {
        on_message_read(msg);
        return false;
    }

    // For SDL games: Emit events BEFORE filtering so overlay gets them
    // even if we filter the message to block SDL
    on_message_read(msg);
    if should_filter_message(msg) {
        unsafe {
            // Call TranslateMessage for char messages
            _ = TranslateMessage(msg);

            // Call Default WndProc so non client area works.
            if UNICODE {
                CallWindowProcW(
                    Some(DefWindowProcA),
                    msg.hwnd,
                    msg.message,
                    msg.wParam,
                    msg.lParam,
                );
            } else {
                CallWindowProcA(
                    Some(DefWindowProcW),
                    msg.hwnd,
                    msg.message,
                    msg.wParam,
                    msg.lParam,
                );
            }
        }

        msg.message = msg::WM_NULL;
    }
    true
}

fn process_peek_message(
    msg: &mut MSG,
    remove: PEEK_MESSAGE_REMOVE_TYPE,
    reader: impl Fn(&mut MSG, PEEK_MESSAGE_REMOVE_TYPE) -> bool,
) -> bool {
    crate::backend::window::thread_hooks::tick_watchdog();
    if !reader(msg, remove) {
        return false;
    }

    let should_filter = should_filter_message(msg);
    if remove.contains(PM_REMOVE) {
        // For SDL games: Emit events BEFORE filtering so overlay gets them
        // even if we filter the message to block SDL
        on_message_read(msg);

        if should_filter {
            // Call TranslateMessage for char messages.
            unsafe {
                _ = TranslateMessage(msg);
            }
        }
    }

    if should_filter {
        msg.message = msg::WM_NULL;
    }

    true
}

#[tracing::instrument]
extern "system" fn hooked_get_message_a(
    lpmsg: *mut MSG,
    hwnd: HWND,
    wmsgfiltermin: u32,
    wmsgfiltermax: u32,
) -> BOOL {
    trace!("GetMessageA called");
    if lpmsg.is_null() {
        return BOOL(0);
    }

    process_read_message::<false>(unsafe { &mut *lpmsg }, |msg| unsafe {
        HOOK.wait().get_message_a.original_fn()(msg, hwnd, wmsgfiltermin, wmsgfiltermax).as_bool()
    })
    .into()
}

#[tracing::instrument]
extern "system" fn hooked_get_message_w(
    lpmsg: *mut MSG,
    hwnd: HWND,
    wmsgfiltermin: u32,
    wmsgfiltermax: u32,
) -> BOOL {
    trace!("GetMessageW called");
    if lpmsg.is_null() {
        return BOOL(0);
    }

    process_read_message::<true>(unsafe { &mut *lpmsg }, |msg| unsafe {
        HOOK.wait().get_message_w.original_fn()(msg, hwnd, wmsgfiltermin, wmsgfiltermax).as_bool()
    })
    .into()
}

#[tracing::instrument]
extern "system" fn hooked_peek_message_a(
    lpmsg: *mut MSG,
    hwnd: HWND,
    wmsgfiltermin: u32,
    wmsgfiltermax: u32,
    wremovemsg: PEEK_MESSAGE_REMOVE_TYPE,
) -> BOOL {
    trace!("PeekMessageA called");
    if lpmsg.is_null() {
        return BOOL(0);
    }

    process_peek_message(unsafe { &mut *lpmsg }, wremovemsg, |msg, remove| unsafe {
        HOOK.wait().peek_message_a.original_fn()(msg, hwnd, wmsgfiltermin, wmsgfiltermax, remove)
            .as_bool()
    })
    .into()
}

#[tracing::instrument]
extern "system" fn hooked_peek_message_w(
    lpmsg: *mut MSG,
    hwnd: HWND,
    wmsgfiltermin: u32,
    wmsgfiltermax: u32,
    wremovemsg: PEEK_MESSAGE_REMOVE_TYPE,
) -> BOOL {
    trace!("PeekMessageW called");
    if lpmsg.is_null() {
        return BOOL(0);
    }

    process_peek_message(unsafe { &mut *lpmsg }, wremovemsg, |msg, remove| unsafe {
        HOOK.wait().peek_message_w.original_fn()(msg, hwnd, wmsgfiltermin, wmsgfiltermax, remove)
            .as_bool()
    })
    .into()
}

fn process_dispatch_message(
    lpmsg: *const MSG,
    dispatcher: impl Fn(*const MSG) -> LRESULT,
) -> LRESULT {
    if lpmsg.is_null() {
        return dispatcher(lpmsg);
    }
    let msg = unsafe { &*lpmsg };
    if should_filter_message(msg) {
        let mut null_msg = *msg;
        apply_pump_consume(&mut null_msg, true);
        dispatcher(&null_msg)
    } else {
        dispatcher(lpmsg)
    }
}

#[tracing::instrument]
extern "system" fn hooked_dispatch_message_a(lpmsg: *const MSG) -> LRESULT {
    trace!("DispatchMessageA called");
    let Some(hook) = HOOK.wait().dispatch_message_a.as_ref() else {
        return LRESULT(0);
    };
    process_dispatch_message(lpmsg, |msg| unsafe { hook.original_fn()(msg) })
}

#[tracing::instrument]
extern "system" fn hooked_dispatch_message_w(lpmsg: *const MSG) -> LRESULT {
    trace!("DispatchMessageW called");
    let Some(hook) = HOOK.wait().dispatch_message_w.as_ref() else {
        return LRESULT(0);
    };
    process_dispatch_message(lpmsg, |msg| unsafe { hook.original_fn()(msg) })
}

pub(crate) fn on_message_read(msg: &MSG) {
    _ = with_root_backend(msg, |backend| {
        let listen_cursor;
        let listen_keyboard;
        {
            let proc = backend.proc.lock();
            listen_cursor = proc.listening_cursor();
            listen_keyboard = proc.listening_keyboard();
        }

        if listen_cursor || listen_keyboard {
            set_message_read(|| {
                let proc = backend.proc.lock();
                if listen_cursor {
                    emit_cursor_event_from_message(backend.id, &proc, msg);
                }
                if listen_keyboard {
                    emit_keyboard_event_from_message(backend.id, &proc, msg);
                }
            });
        }

        {
            let mut proc_queue = backend.proc_queue.lock();
            if proc_queue.is_empty() {
                return;
            }

            for f in proc_queue.drain(..) {
                f(backend);
            }
        }
    });
}

/// Same KEYDOWN `TranslateMessage` the Get/Peek filter path already does.
pub(crate) fn translate_keydown_for_char(msg: &MSG) {
    if matches!(msg.message, msg::WM_KEYDOWN | msg::WM_SYSKEYDOWN) {
        unsafe {
            _ = TranslateMessage(msg);
        }
    }
}

#[inline]
fn emit_cursor_event_from_message(id: u32, proc: &WindowProcData, msg: &MSG) {
    if proc.input_blocking() {
        return;
    }

    match msg.message {
        msg::WM_MOUSEMOVE => {
            emit_cursor_move_event(id, proc, msg.lParam);
        }
        msg::WM_LBUTTONDOWN | msg::WM_LBUTTONDBLCLK => {
            emit_cursor_event(id, proc, CursorAction::Left, true, msg.lParam);
        }
        msg::WM_LBUTTONUP => {
            emit_cursor_event(id, proc, CursorAction::Left, false, msg.lParam);
        }
        msg::WM_RBUTTONDOWN | msg::WM_RBUTTONDBLCLK => {
            emit_cursor_event(id, proc, CursorAction::Right, true, msg.lParam);
        }
        msg::WM_RBUTTONUP => {
            emit_cursor_event(id, proc, CursorAction::Right, false, msg.lParam);
        }
        msg::WM_MBUTTONDOWN | msg::WM_MBUTTONDBLCLK => {
            emit_cursor_event(id, proc, CursorAction::Middle, true, msg.lParam);
        }
        msg::WM_MBUTTONUP => {
            emit_cursor_event(id, proc, CursorAction::Middle, false, msg.lParam);
        }
        msg::WM_MOUSEWHEEL => {
            emit_cursor_scroll_event(id, proc, msg.wParam, msg.lParam, false);
        }
        msg::WM_MOUSEHWHEEL => {
            emit_cursor_scroll_event(id, proc, msg.wParam, msg.lParam, true);
        }
        _ => {}
    }
}

#[inline]
fn emit_keyboard_event_from_message(id: u32, _proc: &WindowProcData, msg: &MSG) {
    // NOTE: `proc` is already locked by the caller; never re-lock it here
    // (re-locking deadlocks the game GUI thread).

    // WM_CHAR / WM_SYSCHAR must always be forwarded to the overlay, even while
    // input is blocked.  The LL keyboard hook only sees WM_KEYDOWN/WM_KEYUP and
    // never emits Char events.  WM_CHAR is generated by TranslateMessage (called
    // inside the filter path for every intercepted WM_KEYDOWN) and represents the
    // actual Unicode character — it is the only source of correct text input that
    // accounts for keyboard layout, dead keys, and Shift/AltGr combinations.
    // Without forwarding it, <input> elements inside overlay panels never receive
    // typed characters.
    match msg.message {
        msg::WM_CHAR | msg::WM_SYSCHAR => {
            if let Some(ch) = char::from_u32(msg.wParam.0 as _) {
                OverlayEventSink::emit(keyboard_input(id, KeyboardInput::Char(ch)));
            }
            return;
        }
        _ => {}
    }

    // Always emit Key down/up from the pump — including while input is blocked.
    // The LL keyboard hook is a second source for raw-input games that never
    // queue WM_KEY*; browser-host dedupes near-duplicate transitions. Skipping
    // pump Keys while blocked (the old behaviour) left Alan Wake 2 with only
    // WM_CHAR: letters typed, but Ctrl+A/Backspace never became CEF key events.
    match msg.message {
        msg::WM_KEYDOWN | msg::WM_SYSKEYDOWN => {
            if let Some(key) = to_key(msg.lParam) {
                OverlayEventSink::emit(keyboard_input(
                    id,
                    KeyboardInput::Key {
                        key,
                        state: KeyInputState::Pressed,
                    },
                ));
            }
        }
        msg::WM_KEYUP | msg::WM_SYSKEYUP => {
            if let Some(key) = to_key(msg.lParam) {
                OverlayEventSink::emit(keyboard_input(
                    id,
                    KeyboardInput::Key {
                        key,
                        state: KeyInputState::Released,
                    },
                ));
            }
        }
        _ => {}
    }
}

#[inline]
fn parse_cursor_position(
    proc: &WindowProcData,
    lparam: LPARAM,
) -> (
    glint_overlay_event::input::InputPosition,
    glint_overlay_event::input::InputPosition,
) {
    use glint_overlay_event::input::InputPosition;

    let [x, y] = bytemuck::cast::<_, [i16; 2]>(lparam.0 as u32);
    let window = InputPosition {
        x: x as _,
        y: y as _,
    };
    let surface = InputPosition {
        x: window.x - proc.position.0,
        y: window.y - proc.position.1,
    };

    (surface, window)
}

#[inline]
fn emit_cursor_event(
    id: u32,
    proc: &WindowProcData,
    action: CursorAction,
    pressed: bool,
    lparam: LPARAM,
) {
    if proc.input_blocking() {
        return;
    }

    use glint_overlay_event::input::{CursorEvent, CursorInputState};

    let (surface, window) = parse_cursor_position(proc, lparam);
    let state = if pressed {
        CursorInputState::Pressed {
            double_click: false,
        }
    } else {
        CursorInputState::Released
    };

    OverlayEventSink::emit(OverlayEvent::Window {
        id,
        event: WindowEvent::Input(InputEvent::Cursor(CursorInput {
            event: CursorEvent::Action { action, state },
            client: surface,
            window,
        })),
    });
}

#[inline]
fn emit_cursor_move_event(id: u32, proc: &WindowProcData, lparam: LPARAM) {
    if proc.input_blocking() {
        return;
    }

    use glint_overlay_event::input::CursorEvent;

    let (surface, window) = parse_cursor_position(proc, lparam);

    OverlayEventSink::emit(OverlayEvent::Window {
        id,
        event: WindowEvent::Input(InputEvent::Cursor(CursorInput {
            event: CursorEvent::Move,
            client: surface,
            window,
        })),
    });
}

#[inline]
fn emit_cursor_scroll_event(
    id: u32,
    proc: &WindowProcData,
    wparam: WPARAM,
    lparam: LPARAM,
    horizontal: bool,
) {
    use glint_overlay_event::input::CursorEvent;

    let [_, delta] = bytemuck::cast::<_, [i16; 2]>(wparam.0 as u32);
    let (surface, window) = parse_cursor_position(proc, lparam);

    OverlayEventSink::emit(OverlayEvent::Window {
        id,
        event: WindowEvent::Input(InputEvent::Cursor(CursorInput {
            event: CursorEvent::Scroll {
                axis: if horizontal {
                    ScrollAxis::X
                } else {
                    ScrollAxis::Y
                },
                delta,
            },
            client: surface,
            window,
        })),
    });
}

const CURSOR_MESSAGES: &[u32] = &[
    msg::WM_MOUSEMOVE,
    msg::WM_LBUTTONDOWN,
    msg::WM_LBUTTONUP,
    msg::WM_LBUTTONDBLCLK,
    msg::WM_RBUTTONDOWN,
    msg::WM_RBUTTONUP,
    msg::WM_RBUTTONDBLCLK,
    msg::WM_MBUTTONDOWN,
    msg::WM_MBUTTONUP,
    msg::WM_MBUTTONDBLCLK,
    msg::WM_XBUTTONDOWN,
    msg::WM_XBUTTONUP,
    msg::WM_XBUTTONDBLCLK,
    msg::WM_MOUSEWHEEL,
    msg::WM_MOUSEHWHEEL,
];

const KEYBOARD_MESSAGES: &[u32] = &[
    msg::WM_KEYDOWN,
    msg::WM_KEYUP,
    msg::WM_CHAR,
    msg::WM_SYSKEYDOWN,
    msg::WM_SYSKEYUP,
    msg::WM_SYSCHAR,
];

#[inline]
fn is_cursor_message(message: u32) -> bool {
    CURSOR_MESSAGES.contains(&message)
}

#[inline]
fn is_keyboard_message(message: u32) -> bool {
    KEYBOARD_MESSAGES.contains(&message)
}

/// Overlay-bound mouse/key messages Steam `sub_1800A4220` consumes on the API path.
/// IME stays on Task 7 CALLWNDPROC — do not swallow it here.
#[inline]
pub(crate) fn should_consume_pump_message(message: u32, interactive: bool) -> bool {
    interactive && (is_cursor_message(message) || is_keyboard_message(message))
}

/// Rewrite overlay-bound input to `WM_NULL` while Interactive. Unrelated messages stay.
fn apply_pump_consume(msg: &mut MSG, interactive: bool) {
    if should_consume_pump_message(msg.message, interactive) {
        msg.message = msg::WM_NULL;
    }
}

/// Filter input messages when blocking is enabled
#[inline]
fn should_filter_message(msg: &MSG) -> bool {
    should_consume_pump_message(
        msg.message,
        with_root_backend(msg, |backend| backend.proc.lock().input_blocking()).unwrap_or(false),
    )
}

#[inline]
fn with_root_backend<R>(msg: &MSG, f: impl FnOnce(&WindowBackend) -> R) -> Option<R> {
    let root_hwnd = unsafe { GetAncestor(msg.hwnd, GA_ROOT) };
    if root_hwnd.is_invalid() {
        return None;
    }

    Backends::with_backend(root_hwnd.0 as _, f)
}

#[inline(always)]
fn keyboard_input(id: u32, input: KeyboardInput) -> OverlayEvent {
    OverlayEvent::Window {
        id,
        event: WindowEvent::Input(InputEvent::Keyboard(input)),
    }
}

#[inline]
fn to_key(lparam: LPARAM) -> Option<Key> {
    let [_, _, code, flags] = bytemuck::cast::<_, [u8; 4]>(lparam.0 as u32);
    Key::new(
        unsafe { MapVirtualKeyA(code as u32, MAPVK_VSC_TO_VK) as u8 },
        flags & 0x01 == 0x01,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_msg(message: u32) -> MSG {
        MSG {
            hwnd: HWND(core::ptr::null_mut()),
            message,
            wParam: WPARAM(0),
            lParam: LPARAM(0),
            time: 0,
            pt: windows::Win32::Foundation::POINT { x: 0, y: 0 },
        }
    }

    #[test]
    fn interactive_keydown_rewrites_to_wm_null() {
        let mut msg = fake_msg(msg::WM_KEYDOWN);
        apply_pump_consume(&mut msg, true);
        assert_eq!(msg.message, msg::WM_NULL);
    }

    #[test]
    fn interactive_lbuttondown_rewrites_to_wm_null() {
        let mut msg = fake_msg(msg::WM_LBUTTONDOWN);
        apply_pump_consume(&mut msg, true);
        assert_eq!(msg.message, msg::WM_NULL);
    }

    #[test]
    fn interactive_paint_passes_through() {
        let mut msg = fake_msg(msg::WM_PAINT);
        apply_pump_consume(&mut msg, true);
        assert_eq!(msg.message, msg::WM_PAINT);
    }

    #[test]
    fn not_interactive_keydown_passes_through() {
        let mut msg = fake_msg(msg::WM_KEYDOWN);
        apply_pump_consume(&mut msg, false);
        assert_eq!(msg.message, msg::WM_KEYDOWN);
    }

    #[test]
    fn interactive_ime_notify_passes_through() {
        let mut msg = fake_msg(msg::WM_IME_NOTIFY);
        apply_pump_consume(&mut msg, true);
        assert_eq!(msg.message, msg::WM_IME_NOTIFY);
    }
}
