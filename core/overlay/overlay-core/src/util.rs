//! Common utilies used in many modules internally.

use core::mem::{self, ManuallyDrop};
use std::ffi::CString;

use anyhow::bail;
use scopeguard::defer;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, LUID, RECT, WPARAM},
        Graphics::Dxgi::{IDXGIAdapter, IDXGIFactory, IDXGIKeyedMutex},
        UI::WindowsAndMessaging::{
            CS_OWNDC, CreateWindowExA, DefWindowProcW, DestroyWindow, GetClientRect, HWND_MESSAGE,
            RegisterClassA, UnregisterClassA, WINDOW_EX_STYLE, WNDCLASSA, WS_POPUP,
        },
    },
    core::{Interface, PCSTR, s},
};

// Cloning COM objects for ManuallyDrop<Option<T>> never decrease ref count and leak wtf
// as per: https://github.com/microsoft/windows-rs/blob/83d4e0b4d49d004f52523614f292bc1526142052/crates/samples/windows/direct3d12/src/main.rs#L493
pub unsafe fn wrap_com_manually_drop<T: Interface>(inf: &T) -> ManuallyDrop<Option<T>> {
    unsafe { mem::transmute_copy(inf) }
}

/// Get Client area size of the window.
pub fn get_client_size(win: HWND) -> anyhow::Result<(u32, u32)> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(win, &mut rect)? };

    Ok((rect.right as u32, rect.bottom as u32))
}

/// Create dummy class and window for various operation.
///
/// Creating another dummy windows in closures fail.
pub fn with_dummy_hwnd<R>(hinstance: HINSTANCE, f: impl FnOnce(HWND) -> R) -> anyhow::Result<R> {
    extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    unsafe {
        let class_name = CString::new(format!(
            "glint-overlay-core-{} dummy window class",
            hinstance.0 as usize
        ))
        .unwrap();
        if RegisterClassA(&WNDCLASSA {
            style: CS_OWNDC,
            hInstance: hinstance,
            lpszClassName: PCSTR(class_name.as_ptr() as _),
            lpfnWndProc: Some(window_proc),
            ..Default::default()
        }) == 0
        {
            bail!("RegisterClassA call failed");
        }
        defer!({
            _ = UnregisterClassA(PCSTR(class_name.as_ptr() as _), Some(hinstance));
        });

        let hwnd = CreateWindowExA(
            WINDOW_EX_STYLE(0),
            PCSTR(class_name.as_ptr() as _),
            s!("glint-overlay-core dummy window"),
            WS_POPUP,
            0,
            0,
            2,
            2,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )?;
        defer!({
            _ = DestroyWindow(hwnd);
        });

        Ok(f(hwnd))
    }
}

/// Max time the render thread waits for the shared-texture keyed mutex.
/// If the host (Electron) stalls while holding it, the game must keep running
/// — skipping the overlay draw for a frame is always preferable to freezing
/// the game's render thread.
const KEYED_MUTEX_TIMEOUT_MS: u32 = 100;

const WAIT_ABANDONED: i32 = 0x80;

/// If [`IDXGIKeyedMutex`],
/// * Exists, acquire the mutex with `0` value key (bounded wait), run closure and release.
/// * Not exists, just run closure.
///
/// Returns `Ok(None)` if the mutex could not be acquired in time (frame skipped).
#[inline]
pub fn with_keyed_mutex<R>(
    mutex: Option<&IDXGIKeyedMutex>,
    f: impl FnOnce() -> R,
) -> windows::core::Result<Option<R>> {
    match mutex {
        Some(mutex) => {
            // AcquireSync reports timeout via a *success* HRESULT
            // (WAIT_TIMEOUT), so the raw HRESULT must be inspected.
            let hr = unsafe {
                (Interface::vtable(mutex).AcquireSync)(
                    Interface::as_raw(mutex),
                    0,
                    KEYED_MUTEX_TIMEOUT_MS,
                )
            };
            hr.ok()?;
            if hr.0 != 0 && hr.0 != WAIT_ABANDONED {
                // WAIT_TIMEOUT: skip this frame instead of blocking.
                return Ok(None);
            }
            defer!(unsafe {
                _ = mutex.ReleaseSync(0);
            });

            Ok(Some(f()))
        }
        None => Ok(Some(f())),
    }
}

pub fn find_adapter_by_luid(factory: &IDXGIFactory, luid: LUID) -> Option<IDXGIAdapter> {
    let mut i = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters(i) } {
        i += 1;
        let Ok(desc) = (unsafe { adapter.GetDesc() }) else {
            continue;
        };

        if desc.AdapterLuid == luid {
            return Some(adapter);
        }
    }

    None
}
