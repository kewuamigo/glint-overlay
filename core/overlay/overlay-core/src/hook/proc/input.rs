use core::{cell::Cell, ffi::c_void};

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use scopeguard::defer;
use tracing::{debug, warn};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, POINT, RECT},
        Graphics::Gdi::HDC,
        UI::{
            Input::{
                HRAWINPUT, Ime::HIMC, KeyboardAndMouse::GetActiveWindow,
                RAW_INPUT_DATA_COMMAND_FLAGS, RAWINPUT, RAWINPUTDEVICE, RAWINPUTDEVICE_FLAGS,
                RAWINPUTDEVICELIST, RAWINPUTHEADER, RID_HEADER, RID_INPUT, RIDEV_CAPTUREMOUSE,
                RIDEV_EXCLUDE, RIDEV_NOLEGACY,
            },
            WindowsAndMessaging::{
                CURSOR_SHOWING, CURSOR_SUPPRESSED, CURSORINFO, CURSORINFO_FLAGS,
                GetForegroundWindow, HCURSOR,
            },
        },
    },
    core::BOOL,
};

use crate::{
    backend::{Backends, window::InputBlockData},
    hook::proc::message_reading,
};

windows::core::link!("user32.dll" "system" fn ClipCursor(lprect: *const RECT) -> BOOL);
windows::core::link!("user32.dll" "system" fn SetCursorPos(x: i32, y: i32) -> BOOL);

windows::core::link!("user32.dll" "system" fn GetClipCursor(lprect: *mut RECT) -> BOOL);
windows::core::link!("user32.dll" "system" fn GetCursorPos(lppoint: *mut POINT) -> BOOL);
windows::core::link!("user32.dll" "system" fn GetPhysicalCursorPos(lppoint: *mut POINT) -> BOOL);
windows::core::link!("user32.dll" "system" fn GetKeyboardState(buf: *mut u8) -> BOOL);
windows::core::link!("user32.dll" "system" fn GetKeyState(vkey: i32) -> i16);
windows::core::link!("user32.dll" "system" fn GetAsyncKeyState(vkey: i32) -> i16);
windows::core::link!(
    "user32.dll" "system"
    fn GetRawInputData(
        hrawinput: HRAWINPUT,
        uicommand: RAW_INPUT_DATA_COMMAND_FLAGS,
        pdata: *mut c_void,
        pcbsize: *mut u32,
        cbsizeheader: u32,
    ) -> u32
);
windows::core::link!("user32.dll" "system" fn GetRawInputBuffer(pdata: *mut RAWINPUT, pcbsize: *mut u32, cbsizeheader: u32) -> u32);
windows::core::link!("user32.dll" "system" fn ShowCursor(bshow: BOOL) -> i32);
windows::core::link!("user32.dll" "system" fn SetCursor(hcursor: HCURSOR) -> HCURSOR);
windows::core::link!("user32.dll" "system" fn GetCursor() -> HCURSOR);
windows::core::link!("user32.dll" "system" fn GetCursorInfo(pci: *mut CURSORINFO) -> BOOL);
windows::core::link!("user32.dll" "system" fn SetCapture(hwnd: HWND) -> HWND);
windows::core::link!("user32.dll" "system" fn ReleaseCapture() -> BOOL);
windows::core::link!("user32.dll" "system" fn FlashWindow(hwnd: HWND, binvert: BOOL) -> BOOL);
windows::core::link!("user32.dll" "system" fn FlashWindowEx(pfwi: *const c_void) -> BOOL);
windows::core::link!("gdi32.dll" "system" fn SetDeviceGammaRamp(hdc: HDC, lpramp: *const c_void) -> BOOL);
windows::core::link!("user32.dll" "system" fn RegisterDeviceNotificationA(hrecipient: HANDLE, notificationfilter: *const c_void, flags: u32) -> *mut c_void);
windows::core::link!("user32.dll" "system" fn RegisterDeviceNotificationW(hrecipient: HANDLE, notificationfilter: *const c_void, flags: u32) -> *mut c_void);
windows::core::link!("user32.dll" "system" fn UnregisterDeviceNotification(handle: *mut c_void) -> BOOL);
windows::core::link!("user32.dll" "system" fn RegisterRawInputDevices(prawinputdevices: *const RAWINPUTDEVICE, uinumdevices: u32, cbsize: u32) -> BOOL);
windows::core::link!("user32.dll" "system" fn GetRegisteredRawInputDevices(prawinputdevices: *mut RAWINPUTDEVICE, puinumdevices: *mut u32, cbsize: u32) -> u32);
windows::core::link!("user32.dll" "system" fn GetRawInputDeviceList(prawinputdevicelist: *mut RAWINPUTDEVICELIST, puinumdevices: *mut u32, cbsize: u32) -> u32);
windows::core::link!("user32.dll" "system" fn GetRawInputDeviceInfoA(hdevice: HANDLE, uicommand: u32, pdata: *mut c_void, pcbsize: *mut u32) -> u32);
windows::core::link!("user32.dll" "system" fn GetRawInputDeviceInfoW(hdevice: HANDLE, uicommand: u32, pdata: *mut c_void, pcbsize: *mut u32) -> u32);
windows::core::link!("imm32.dll" "system" fn ImmAssociateContext(hwnd: HWND, himc: HIMC) -> HIMC);

struct Hook {
    clip_cursor: DetourHook<ClipCursorFn>,
    set_cursor_pos: DetourHook<SetCursorFn>,

    get_clip_cursor: DetourHook<GetClipCursorFn>,
    get_cursor_pos: DetourHook<GetCursorPos>,
    get_physical_cursor_pos: DetourHook<GetPhysicalCursorPos>,
    get_async_key_state: DetourHook<GetAsyncKeyStateFn>,
    get_key_state: DetourHook<GetKeyStateFn>,
    get_keyboard_state: DetourHook<GetKeyboardStateFn>,
    get_raw_input_data: DetourHook<GetRawInputDataFn>,
    get_raw_input_buffer: DetourHook<GetRawInputBufferFn>,
    show_cursor: Option<DetourHook<ShowCursorFn>>,
    set_cursor: Option<DetourHook<SetCursorHandleFn>>,
    get_cursor: Option<DetourHook<GetCursorFn>>,
    get_cursor_info: Option<DetourHook<GetCursorInfoFn>>,
    set_capture: Option<DetourHook<SetCaptureFn>>,
    release_capture: Option<DetourHook<ReleaseCaptureFn>>,
    flash_window: Option<DetourHook<FlashWindowFn>>,
    flash_window_ex: Option<DetourHook<FlashWindowExFn>>,
    set_device_gamma_ramp: Option<DetourHook<SetDeviceGammaRampFn>>,
    register_device_notification_a: Option<DetourHook<RegisterDeviceNotificationFn>>,
    register_device_notification_w: Option<DetourHook<RegisterDeviceNotificationFn>>,
    unregister_device_notification: Option<DetourHook<UnregisterDeviceNotificationFn>>,
    register_raw_input_devices: Option<DetourHook<RegisterRawInputDevicesFn>>,
    get_registered_raw_input_devices: Option<DetourHook<GetRegisteredRawInputDevicesFn>>,
    get_raw_input_device_list: Option<DetourHook<GetRawInputDeviceListFn>>,
    get_raw_input_device_info_a: Option<DetourHook<GetRawInputDeviceInfoFn>>,
    get_raw_input_device_info_w: Option<DetourHook<GetRawInputDeviceInfoFn>>,
    imm_associate_context: Option<DetourHook<ImmAssociateContextFn>>,
}
static HOOK: OnceCell<Hook> = OnceCell::new();

type ClipCursorFn = unsafe extern "system" fn(*const RECT) -> BOOL;
type SetCursorFn = unsafe extern "system" fn(i32, i32) -> BOOL;

type GetClipCursorFn = unsafe extern "system" fn(*mut RECT) -> BOOL;
type GetCursorPos = unsafe extern "system" fn(*mut POINT) -> BOOL;
type GetPhysicalCursorPos = unsafe extern "system" fn(*mut POINT) -> BOOL;
type GetAsyncKeyStateFn = unsafe extern "system" fn(i32) -> i16;
type GetKeyStateFn = unsafe extern "system" fn(i32) -> i16;
type GetKeyboardStateFn = unsafe extern "system" fn(*mut u8) -> BOOL;
type GetRawInputDataFn = unsafe extern "system" fn(
    HRAWINPUT,
    RAW_INPUT_DATA_COMMAND_FLAGS,
    *mut c_void,
    *mut u32,
    u32,
) -> u32;
type GetRawInputBufferFn = unsafe extern "system" fn(*mut RAWINPUT, *mut u32, u32) -> u32;
type ShowCursorFn = unsafe extern "system" fn(BOOL) -> i32;
type SetCursorHandleFn = unsafe extern "system" fn(HCURSOR) -> HCURSOR;
type GetCursorFn = unsafe extern "system" fn() -> HCURSOR;
type GetCursorInfoFn = unsafe extern "system" fn(*mut CURSORINFO) -> BOOL;
type SetCaptureFn = unsafe extern "system" fn(HWND) -> HWND;
type ReleaseCaptureFn = unsafe extern "system" fn() -> BOOL;
type FlashWindowFn = unsafe extern "system" fn(HWND, BOOL) -> BOOL;
type FlashWindowExFn = unsafe extern "system" fn(*const c_void) -> BOOL;
type SetDeviceGammaRampFn = unsafe extern "system" fn(HDC, *const c_void) -> BOOL;
type RegisterDeviceNotificationFn =
    unsafe extern "system" fn(HANDLE, *const c_void, u32) -> *mut c_void;
type UnregisterDeviceNotificationFn = unsafe extern "system" fn(*mut c_void) -> BOOL;
type RegisterRawInputDevicesFn = unsafe extern "system" fn(*const RAWINPUTDEVICE, u32, u32) -> BOOL;
type GetRegisteredRawInputDevicesFn =
    unsafe extern "system" fn(*mut RAWINPUTDEVICE, *mut u32, u32) -> u32;
type GetRawInputDeviceListFn =
    unsafe extern "system" fn(*mut RAWINPUTDEVICELIST, *mut u32, u32) -> u32;
type GetRawInputDeviceInfoFn = unsafe extern "system" fn(HANDLE, u32, *mut c_void, *mut u32) -> u32;
type ImmAssociateContextFn = unsafe extern "system" fn(HWND, HIMC) -> HIMC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum User32Gate {
    Original,
    Swallow,
}

fn capture_gate(passthrough: bool, interactive: bool) -> User32Gate {
    if passthrough || !interactive {
        User32Gate::Original
    } else {
        User32Gate::Swallow
    }
}

fn flash_gamma_gate(interactive: bool) -> User32Gate {
    if interactive {
        User32Gate::Swallow
    } else {
        User32Gate::Original
    }
}

#[cfg(test)]
fn register_device_notification_gate(_interactive: bool) -> User32Gate {
    User32Gate::Original
}

fn unregister_device_notification_gate(interactive: bool) -> User32Gate {
    if interactive {
        User32Gate::Swallow
    } else {
        User32Gate::Original
    }
}

const HID_USAGE_PAGE_GENERIC: u16 = 1;
const HID_USAGE_GENERIC_MOUSE: u16 = 2;

fn exclusive_raw_mouse_flag_mask() -> u32 {
    RIDEV_NOLEGACY.0 | RIDEV_CAPTUREMOUSE.0 | RIDEV_EXCLUDE.0
}

fn is_exclusive_raw_mouse(usage_page: u16, usage: u16, flags: u32) -> bool {
    let flags = RAWINPUTDEVICE_FLAGS(flags);
    usage_page == HID_USAGE_PAGE_GENERIC
        && usage == HID_USAGE_GENERIC_MOUSE
        && (flags.contains(RIDEV_NOLEGACY)
            || flags.contains(RIDEV_CAPTUREMOUSE)
            || flags.contains(RIDEV_EXCLUDE))
}

fn raw_register_flags_for_call(
    usage_page: u16,
    usage: u16,
    flags: u32,
    interactive: bool,
    passthrough: bool,
) -> u32 {
    if passthrough || !interactive || !is_exclusive_raw_mouse(usage_page, usage, flags) {
        flags
    } else {
        flags & !exclusive_raw_mouse_flag_mask()
    }
}

#[cfg(test)]
fn raw_query_gate(_interactive: bool) -> User32Gate {
    User32Gate::Original
}

fn imm_associate_gate(passthrough: bool, interactive: bool) -> User32Gate {
    if passthrough || !interactive {
        User32Gate::Original
    } else {
        User32Gate::Swallow
    }
}

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
    HOOK.get_or_try_init(|| unsafe {
        debug!("hooking ClipCursor");
        let clip_cursor = DetourHook::attach(ClipCursor as _, hooked_clip_cursor as _)?;

        debug!("hooking SetCursorPos");
        let set_cursor_pos = DetourHook::attach(SetCursorPos as _, hooked_set_cursor_pos as _)?;

        debug!("hooking GetClipCursor");
        let get_clip_cursor = DetourHook::attach(GetClipCursor as _, hooked_get_clip_cursor as _)?;

        debug!("hooking GetCursorPos");
        let get_cursor_pos = DetourHook::attach(GetCursorPos as _, hooked_get_cursor_pos as _)?;

        debug!("hooking GetPhysicalCursorPos");
        let get_physical_cursor_pos = DetourHook::attach(
            GetPhysicalCursorPos as _,
            hooked_get_physical_cursor_pos as _,
        )?;

        debug!("hooking GetAsyncKeyState");
        let get_async_key_state =
            DetourHook::attach(GetAsyncKeyState as _, hooked_get_async_key_state as _)?;

        debug!("hooking GetKeyState");
        let get_key_state = DetourHook::attach(GetKeyState as _, hooked_get_key_state as _)?;

        debug!("hooking GetKeyboardState");
        let get_keyboard_state =
            DetourHook::attach(GetKeyboardState as _, hooked_get_keyboard_state as _)?;

        debug!("hooking GetRawInputData");
        let get_raw_input_data =
            DetourHook::attach(GetRawInputData as _, hooked_get_raw_input_data as _)?;

        debug!("hooking GetRawInputBuffer");
        let get_raw_input_buffer =
            DetourHook::attach(GetRawInputBuffer as _, hooked_get_raw_input_buffer as _)?;

        let show_cursor = attach_soft("ShowCursor", ShowCursor as _, hooked_show_cursor as _);
        let set_cursor = attach_soft("SetCursor", SetCursor as _, hooked_set_cursor as _);
        let get_cursor = attach_soft("GetCursor", GetCursor as _, hooked_get_cursor as _);
        let get_cursor_info = attach_soft(
            "GetCursorInfo",
            GetCursorInfo as _,
            hooked_get_cursor_info as _,
        );
        let set_capture = attach_soft("SetCapture", SetCapture as _, hooked_set_capture as _);
        let release_capture = attach_soft(
            "ReleaseCapture",
            ReleaseCapture as _,
            hooked_release_capture as _,
        );
        let flash_window = attach_soft("FlashWindow", FlashWindow as _, hooked_flash_window as _);
        let flash_window_ex = attach_soft(
            "FlashWindowEx",
            FlashWindowEx as _,
            hooked_flash_window_ex as _,
        );
        let set_device_gamma_ramp = attach_soft(
            "SetDeviceGammaRamp",
            SetDeviceGammaRamp as _,
            hooked_set_device_gamma_ramp as _,
        );
        let register_device_notification_a = attach_soft(
            "RegisterDeviceNotificationA",
            RegisterDeviceNotificationA as _,
            hooked_register_device_notification_a as _,
        );
        let register_device_notification_w = attach_soft(
            "RegisterDeviceNotificationW",
            RegisterDeviceNotificationW as _,
            hooked_register_device_notification_w as _,
        );
        let unregister_device_notification = attach_soft(
            "UnregisterDeviceNotification",
            UnregisterDeviceNotification as _,
            hooked_unregister_device_notification as _,
        );
        let register_raw_input_devices = attach_soft(
            "RegisterRawInputDevices",
            RegisterRawInputDevices as _,
            hooked_register_raw_input_devices as _,
        );
        let get_registered_raw_input_devices = attach_soft(
            "GetRegisteredRawInputDevices",
            GetRegisteredRawInputDevices as _,
            hooked_get_registered_raw_input_devices as _,
        );
        let get_raw_input_device_list = attach_soft(
            "GetRawInputDeviceList",
            GetRawInputDeviceList as _,
            hooked_get_raw_input_device_list as _,
        );
        let get_raw_input_device_info_a = attach_soft(
            "GetRawInputDeviceInfoA",
            GetRawInputDeviceInfoA as _,
            hooked_get_raw_input_device_info_a as _,
        );
        let get_raw_input_device_info_w = attach_soft(
            "GetRawInputDeviceInfoW",
            GetRawInputDeviceInfoW as _,
            hooked_get_raw_input_device_info_w as _,
        );
        let imm_associate_context = attach_soft(
            "ImmAssociateContext",
            ImmAssociateContext as _,
            hooked_imm_associate_context as _,
        );

        Ok::<_, anyhow::Error>(Hook {
            clip_cursor,
            set_cursor_pos,

            get_clip_cursor,
            get_cursor_pos,
            get_physical_cursor_pos,
            get_async_key_state,
            get_key_state,
            get_keyboard_state,
            get_raw_input_data,
            get_raw_input_buffer,
            show_cursor,
            set_cursor,
            get_cursor,
            get_cursor_info,
            set_capture,
            release_capture,
            flash_window,
            flash_window_ex,
            set_device_gamma_ramp,
            register_device_notification_a,
            register_device_notification_w,
            unregister_device_notification,
            register_raw_input_devices,
            get_registered_raw_input_devices,
            get_raw_input_device_list,
            get_raw_input_device_info_a,
            get_raw_input_device_info_w,
            imm_associate_context,
        })
    })?;

    Ok(())
}

#[inline]
fn active_hwnd_can_block() -> Option<HWND> {
    let hwnd = unsafe { GetActiveWindow() };

    if !hwnd.is_invalid() && !message_reading() {
        Some(hwnd)
    } else {
        None
    }
}

#[inline]
fn active_hwnd_with<R>(f: impl FnOnce(&mut InputBlockData) -> R) -> Option<R> {
    Backends::with_backend(active_hwnd_can_block()?.0 as _, |backend| {
        let mut proc = backend.proc.lock();
        Some(f(proc.blocking_state.as_mut()?))
    })
    .flatten()
}

thread_local! {
    static CURSOR_PASSTHROUGH: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn with_cursor_passthrough<R>(f: impl FnOnce() -> R) -> R {
    CURSOR_PASSTHROUGH.with(|p| {
        p.set(true);
        defer!(CURSOR_PASSTHROUGH.with(|p| p.set(false)));
        f()
    })
}

#[inline]
fn cursor_passthrough() -> bool {
    CURSOR_PASSTHROUGH.with(Cell::get)
}

/// Steam uses a process-global overlay-shown flag for cursor APIs.
#[inline]
fn with_any_blocking<R>(f: impl FnOnce(&mut InputBlockData) -> R) -> Option<R> {
    for backend in Backends::iter() {
        let mut proc = backend.proc.lock();
        if let Some(data) = proc.blocking_state.as_mut() {
            return Some(f(data));
        }
    }
    None
}

#[inline]
fn any_interactive() -> bool {
    with_any_blocking(|_| ()).is_some()
}

#[inline]
fn foreground_hwnd_input_blocked() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };

    !hwnd.is_invalid()
        && Backends::with_backend(hwnd.0 as _, |backend| backend.proc.lock().input_blocking())
            .unwrap_or(false)
}

#[inline]
fn active_hwnd_input_blocked() -> bool {
    active_hwnd_with(|_| ()).is_some()
}

#[tracing::instrument]
extern "system" fn hooked_clip_cursor(lprect: *const RECT) -> BOOL {
    if active_hwnd_with(|data| {
        data.clip_cursor = unsafe { lprect.as_ref() }.copied();
    })
    .is_some()
    {
        return BOOL(1);
    }

    unsafe { HOOK.wait().clip_cursor.original_fn()(lprect) }
}

#[tracing::instrument]
extern "system" fn hooked_set_cursor_pos(x: i32, y: i32) -> BOOL {
    if foreground_hwnd_input_blocked() {
        return BOOL(1);
    }

    unsafe { HOOK.wait().set_cursor_pos.original_fn()(x, y) }
}

#[tracing::instrument]
extern "system" fn hooked_get_clip_cursor(lprect: *mut RECT) -> BOOL {
    match active_hwnd_with(|data| data.clip_cursor).flatten() {
        Some(rect) => {
            unsafe { lprect.write(rect) };
            BOOL(1)
        }
        None => unsafe { HOOK.wait().get_clip_cursor.original_fn()(lprect) },
    }
}

#[tracing::instrument]
extern "system" fn hooked_get_cursor_pos(lppoint: *mut POINT) -> BOOL {
    if foreground_hwnd_input_blocked() {
        // Return a fixed position instead of the real cursor position to prevent games from tracking mouse movement
        if !lppoint.is_null() {
            unsafe {
                lppoint.write(POINT { x: 0, y: 0 });
            }
        }
        return BOOL(1);
    }

    unsafe { HOOK.wait().get_cursor_pos.original_fn()(lppoint) }
}

#[tracing::instrument]
extern "system" fn hooked_get_physical_cursor_pos(lppoint: *mut POINT) -> BOOL {
    if foreground_hwnd_input_blocked() {
        // Return a fixed position instead of the real cursor position to prevent games from tracking mouse movement
        if !lppoint.is_null() {
            unsafe {
                lppoint.write(POINT { x: 0, y: 0 });
            }
        }
        return BOOL(1);
    }

    unsafe { HOOK.wait().get_physical_cursor_pos.original_fn()(lppoint) }
}

#[tracing::instrument]
extern "system" fn hooked_get_async_key_state(vkey: i32) -> i16 {
    if foreground_hwnd_input_blocked() {
        return 0;
    }

    unsafe { HOOK.wait().get_async_key_state.original_fn()(vkey) }
}

#[tracing::instrument]
extern "system" fn hooked_get_key_state(vkey: i32) -> i16 {
    if active_hwnd_input_blocked() {
        return 0;
    }

    unsafe { HOOK.wait().get_key_state.original_fn()(vkey) }
}

#[tracing::instrument]
extern "system" fn hooked_get_keyboard_state(buf: *mut u8) -> BOOL {
    if active_hwnd_input_blocked() {
        // buf is 256 bytes array according to doc.
        unsafe {
            buf.write_bytes(0u8, 256);
        };
        return BOOL(1);
    }

    unsafe { HOOK.wait().get_keyboard_state.original_fn()(buf) }
}

#[tracing::instrument]
extern "system" fn hooked_get_raw_input_data(
    hrawinput: HRAWINPUT,
    uicommand: RAW_INPUT_DATA_COMMAND_FLAGS,
    pdata: *mut c_void,
    pcbsize: *mut u32,
    cbsizeheader: u32,
) -> u32 {
    if foreground_hwnd_input_blocked() {
        // Determine the expected data size based on the command, matching the
        // real Win32 API behaviour so callers (e.g. Godot 4) that validate the
        // returned size against the queried size do not crash.
        let data_size: u32 = match uicommand {
            RID_HEADER => core::mem::size_of::<RAWINPUTHEADER>() as u32,
            RID_INPUT => core::mem::size_of::<RAWINPUT>() as u32,
            _ => 0,
        };

        if !pcbsize.is_null() {
            unsafe { pcbsize.write(data_size) };
        }

        // Size query: return 0 (success) after writing the required buffer size.
        if pdata.is_null() {
            return 0;
        }

        // Data query: write a dummy HID struct and return the number of bytes
        // written, exactly as the real API would on success.
        // Use RIM_TYPEHID so games treat this as an unknown HID device and
        // ignore the empty payload instead of interpreting zeroed fields as
        // mouse/keyboard input.

        let hid_header = RAWINPUTHEADER {
            dwType: 2, // RIM_TYPEHID
            dwSize: data_size,
            ..Default::default()
        };

        match uicommand {
            RID_HEADER => unsafe {
                pdata.cast::<RAWINPUTHEADER>().write(hid_header);
            },
            RID_INPUT => unsafe {
                pdata.cast::<RAWINPUT>().write(RAWINPUT {
                    header: hid_header,
                    ..Default::default()
                });
            },
            _ => {}
        }

        return data_size;
    }

    unsafe {
        HOOK.wait().get_raw_input_data.original_fn()(
            hrawinput,
            uicommand,
            pdata,
            pcbsize,
            cbsizeheader,
        )
    }
}

#[tracing::instrument]
extern "system" fn hooked_get_raw_input_buffer(
    pdata: *mut RAWINPUT,
    pcbsize: *mut u32,
    cbsizeheader: u32,
) -> u32 {
    if foreground_hwnd_input_blocked() {
        unsafe { *pcbsize = 0 };
        return 0;
    }

    unsafe { HOOK.wait().get_raw_input_buffer.original_fn()(pdata, pcbsize, cbsizeheader) }
}

#[tracing::instrument]
extern "system" fn hooked_show_cursor(show: BOOL) -> i32 {
    if !cursor_passthrough() {
        if let Some(count) = with_any_blocking(|data| data.apply_show_cursor(show.as_bool())) {
            return count;
        }
    }

    unsafe { HOOK.wait().show_cursor.as_ref().unwrap().original_fn()(show) }
}

#[tracing::instrument]
extern "system" fn hooked_set_cursor(hcursor: HCURSOR) -> HCURSOR {
    if !cursor_passthrough() {
        if let Some(prev) = with_any_blocking(|data| data.apply_set_cursor(hcursor.0 as isize)) {
            return HCURSOR(prev as *mut _);
        }
    }

    unsafe { HOOK.wait().set_cursor.as_ref().unwrap().original_fn()(hcursor) }
}

#[tracing::instrument]
extern "system" fn hooked_get_cursor() -> HCURSOR {
    if !cursor_passthrough() {
        if let Some(saved) = with_any_blocking(|data| data.saved_cursor) {
            return HCURSOR(saved as *mut _);
        }
    }

    unsafe { HOOK.wait().get_cursor.as_ref().unwrap().original_fn()() }
}

fn mask_cursor_info(info: &mut CURSORINFO, data: &InputBlockData) {
    info.ptScreenPos = POINT { x: 0, y: 0 };
    if info.flags != CURSOR_SUPPRESSED {
        info.flags = if data.show_count >= 0 {
            CURSOR_SHOWING
        } else {
            CURSORINFO_FLAGS(0)
        };
    }
    info.hCursor = HCURSOR(data.saved_cursor as *mut _);
}

#[tracing::instrument]
extern "system" fn hooked_get_cursor_info(pci: *mut CURSORINFO) -> BOOL {
    let ok = unsafe { HOOK.wait().get_cursor_info.as_ref().unwrap().original_fn()(pci) };
    if !pci.is_null() {
        if let Some(()) = with_any_blocking(|data| unsafe { mask_cursor_info(&mut *pci, data) }) {
            return ok;
        }
    }
    ok
}

#[tracing::instrument]
extern "system" fn hooked_set_capture(hwnd: HWND) -> HWND {
    if !cursor_passthrough() && capture_gate(false, any_interactive()) == User32Gate::Swallow {
        return hwnd;
    }

    unsafe { HOOK.wait().set_capture.as_ref().unwrap().original_fn()(hwnd) }
}

#[tracing::instrument]
extern "system" fn hooked_release_capture() -> BOOL {
    if !cursor_passthrough() && capture_gate(false, any_interactive()) == User32Gate::Swallow {
        return BOOL(1);
    }

    unsafe { HOOK.wait().release_capture.as_ref().unwrap().original_fn()() }
}

#[tracing::instrument]
extern "system" fn hooked_flash_window(hwnd: HWND, binvert: BOOL) -> BOOL {
    if flash_gamma_gate(any_interactive()) == User32Gate::Swallow {
        return BOOL(1);
    }

    unsafe { HOOK.wait().flash_window.as_ref().unwrap().original_fn()(hwnd, binvert) }
}

#[tracing::instrument]
extern "system" fn hooked_flash_window_ex(pfwi: *const c_void) -> BOOL {
    if flash_gamma_gate(any_interactive()) == User32Gate::Swallow {
        return BOOL(1);
    }

    unsafe { HOOK.wait().flash_window_ex.as_ref().unwrap().original_fn()(pfwi) }
}

#[tracing::instrument]
extern "system" fn hooked_set_device_gamma_ramp(hdc: HDC, lpramp: *const c_void) -> BOOL {
    if flash_gamma_gate(any_interactive()) == User32Gate::Swallow {
        return BOOL(1);
    }

    unsafe {
        HOOK.wait()
            .set_device_gamma_ramp
            .as_ref()
            .unwrap()
            .original_fn()(hdc, lpramp)
    }
}

#[tracing::instrument]
extern "system" fn hooked_register_device_notification_a(
    hrecipient: HANDLE,
    notificationfilter: *const c_void,
    flags: u32,
) -> *mut c_void {
    unsafe {
        HOOK.wait()
            .register_device_notification_a
            .as_ref()
            .unwrap()
            .original_fn()(hrecipient, notificationfilter, flags)
    }
}

#[tracing::instrument]
extern "system" fn hooked_register_device_notification_w(
    hrecipient: HANDLE,
    notificationfilter: *const c_void,
    flags: u32,
) -> *mut c_void {
    unsafe {
        HOOK.wait()
            .register_device_notification_w
            .as_ref()
            .unwrap()
            .original_fn()(hrecipient, notificationfilter, flags)
    }
}

#[tracing::instrument]
extern "system" fn hooked_unregister_device_notification(handle: *mut c_void) -> BOOL {
    if unregister_device_notification_gate(any_interactive()) == User32Gate::Swallow {
        return BOOL(1);
    }

    unsafe {
        HOOK.wait()
            .unregister_device_notification
            .as_ref()
            .unwrap()
            .original_fn()(handle)
    }
}

#[tracing::instrument]
extern "system" fn hooked_register_raw_input_devices(
    prawinputdevices: *const RAWINPUTDEVICE,
    uinumdevices: u32,
    cbsize: u32,
) -> BOOL {
    let original = || unsafe {
        HOOK.wait()
            .register_raw_input_devices
            .as_ref()
            .unwrap()
            .original_fn()(prawinputdevices, uinumdevices, cbsize)
    };

    if cursor_passthrough()
        || !any_interactive()
        || prawinputdevices.is_null()
        || uinumdevices == 0
        || cbsize as usize != core::mem::size_of::<RAWINPUTDEVICE>()
    {
        return original();
    }

    let devices = unsafe { core::slice::from_raw_parts(prawinputdevices, uinumdevices as usize) };
    let mut filtered: Vec<RAWINPUTDEVICE> = devices.to_vec();
    for device in &mut filtered {
        device.dwFlags = RAWINPUTDEVICE_FLAGS(raw_register_flags_for_call(
            device.usUsagePage,
            device.usUsage,
            device.dwFlags.0,
            true,
            false,
        ));
    }

    unsafe {
        HOOK.wait()
            .register_raw_input_devices
            .as_ref()
            .unwrap()
            .original_fn()(filtered.as_ptr(), filtered.len() as u32, cbsize)
    }
}

#[tracing::instrument]
extern "system" fn hooked_get_registered_raw_input_devices(
    prawinputdevices: *mut RAWINPUTDEVICE,
    puinumdevices: *mut u32,
    cbsize: u32,
) -> u32 {
    unsafe {
        HOOK.wait()
            .get_registered_raw_input_devices
            .as_ref()
            .unwrap()
            .original_fn()(prawinputdevices, puinumdevices, cbsize)
    }
}

#[tracing::instrument]
extern "system" fn hooked_get_raw_input_device_list(
    prawinputdevicelist: *mut RAWINPUTDEVICELIST,
    puinumdevices: *mut u32,
    cbsize: u32,
) -> u32 {
    unsafe {
        HOOK.wait()
            .get_raw_input_device_list
            .as_ref()
            .unwrap()
            .original_fn()(prawinputdevicelist, puinumdevices, cbsize)
    }
}

#[tracing::instrument]
extern "system" fn hooked_get_raw_input_device_info_a(
    hdevice: HANDLE,
    uicommand: u32,
    pdata: *mut c_void,
    pcbsize: *mut u32,
) -> u32 {
    unsafe {
        HOOK.wait()
            .get_raw_input_device_info_a
            .as_ref()
            .unwrap()
            .original_fn()(hdevice, uicommand, pdata, pcbsize)
    }
}

#[tracing::instrument]
extern "system" fn hooked_get_raw_input_device_info_w(
    hdevice: HANDLE,
    uicommand: u32,
    pdata: *mut c_void,
    pcbsize: *mut u32,
) -> u32 {
    unsafe {
        HOOK.wait()
            .get_raw_input_device_info_w
            .as_ref()
            .unwrap()
            .original_fn()(hdevice, uicommand, pdata, pcbsize)
    }
}

#[tracing::instrument]
extern "system" fn hooked_imm_associate_context(hwnd: HWND, himc: HIMC) -> HIMC {
    if imm_associate_gate(cursor_passthrough(), any_interactive()) == User32Gate::Swallow {
        return HIMC::default();
    }

    unsafe {
        HOOK.wait()
            .imm_associate_context
            .as_ref()
            .unwrap()
            .original_fn()(hwnd, himc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(show_count: i32, saved_cursor: isize) -> InputBlockData {
        InputBlockData {
            clip_cursor: None,
            old_ime_cx: 0,
            show_count,
            saved_cursor,
            class_cursor: 0,
        }
    }

    #[test]
    fn get_cursor_info_does_not_leak_overlay_showing() {
        let data = block(-1, 0x11);
        let mut info = CURSORINFO {
            cbSize: core::mem::size_of::<CURSORINFO>() as u32,
            flags: CURSOR_SHOWING,
            hCursor: HCURSOR(0x99 as *mut _),
            ptScreenPos: POINT { x: 50, y: 60 },
        };
        mask_cursor_info(&mut info, &data);
        assert_eq!(info.flags, CURSORINFO_FLAGS(0));
        assert_eq!(info.hCursor.0 as isize, 0x11);
        assert_eq!(info.ptScreenPos, POINT { x: 0, y: 0 });
    }

    #[test]
    fn overlay_owned_set_cursor_passthrough_keeps_saved() {
        let mut data = block(3, 0x1234);
        with_cursor_passthrough(|| {
            if !cursor_passthrough() {
                data.apply_set_cursor(0x99);
            }
        });
        assert_eq!(data.saved_cursor, 0x1234);
        assert_eq!(data.show_count, 3);
    }

    #[test]
    fn interactive_set_capture_on_foreign_hwnd_does_not_apply() {
        assert_eq!(capture_gate(false, true), User32Gate::Swallow);
    }

    #[test]
    fn overlay_owned_capture_passthrough_hits_original() {
        assert_eq!(capture_gate(true, true), User32Gate::Original);
    }

    #[test]
    fn capture_not_interactive_hits_original() {
        assert_eq!(capture_gate(false, false), User32Gate::Original);
    }

    #[test]
    fn interactive_flash_gamma_swallowed() {
        assert_eq!(flash_gamma_gate(true), User32Gate::Swallow);
    }

    #[test]
    fn flash_gamma_not_interactive_hits_original() {
        assert_eq!(flash_gamma_gate(false), User32Gate::Original);
    }

    #[test]
    fn register_device_notification_prefers_original() {
        assert_eq!(
            register_device_notification_gate(true),
            User32Gate::Original
        );
        assert_eq!(
            register_device_notification_gate(false),
            User32Gate::Original
        );
    }

    #[test]
    fn interactive_unregister_device_notification_is_noop() {
        assert_eq!(
            unregister_device_notification_gate(true),
            User32Gate::Swallow
        );
        assert_eq!(
            unregister_device_notification_gate(false),
            User32Gate::Original
        );
    }

    #[test]
    fn interactive_exclusive_raw_mouse_register_is_stripped() {
        let flags = RIDEV_NOLEGACY.0 | RIDEV_CAPTUREMOUSE.0 | RIDEV_EXCLUDE.0;
        assert_eq!(
            raw_register_flags_for_call(1, 2, flags, true, false),
            flags & !exclusive_raw_mouse_flag_mask()
        );
    }

    #[test]
    fn exclusive_raw_mouse_not_interactive_hits_original() {
        let flags = RIDEV_NOLEGACY.0 | RIDEV_CAPTUREMOUSE.0;
        assert_eq!(
            raw_register_flags_for_call(1, 2, flags, false, false),
            flags
        );
    }

    #[test]
    fn non_exclusive_raw_mouse_interactive_hits_original() {
        assert_eq!(raw_register_flags_for_call(1, 2, 0, true, false), 0);
    }

    #[test]
    fn raw_query_apis_prefer_original() {
        assert_eq!(raw_query_gate(true), User32Gate::Original);
        assert_eq!(raw_query_gate(false), User32Gate::Original);
    }

    #[test]
    fn interactive_imm_associate_does_not_apply() {
        assert_eq!(imm_associate_gate(false, true), User32Gate::Swallow);
    }

    #[test]
    fn overlay_owned_imm_passthrough_hits_original() {
        assert_eq!(imm_associate_gate(true, true), User32Gate::Original);
    }

    #[test]
    fn imm_not_interactive_hits_original() {
        assert_eq!(imm_associate_gate(false, false), User32Gate::Original);
    }

    #[test]
    fn get_cursor_info_keeps_suppressed_flag() {
        let data = block(1, 0x22);
        let mut info = CURSORINFO {
            cbSize: core::mem::size_of::<CURSORINFO>() as u32,
            flags: CURSOR_SUPPRESSED,
            hCursor: HCURSOR(0x99 as *mut _),
            ptScreenPos: POINT { x: 1, y: 2 },
        };
        mask_cursor_info(&mut info, &data);
        assert_eq!(info.flags, CURSOR_SUPPRESSED);
        assert_eq!(info.hCursor.0 as isize, 0x22);
    }
}
