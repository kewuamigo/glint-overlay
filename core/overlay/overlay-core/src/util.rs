//! Common utilies used in many modules internally.

use core::{
    cell::RefCell,
    mem::{self, ManuallyDrop},
};

use scopeguard::defer;
use windows::{
    Win32::{
        Foundation::{HWND, LUID, RECT},
        Graphics::Dxgi::{IDXGIAdapter, IDXGIFactory, IDXGIKeyedMutex},
        UI::WindowsAndMessaging::GetClientRect,
    },
    core::Interface,
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

/// AcquireSync timeout: 0 = do not wait. If the host holds the mutex, still
/// draw the last mailbox (Steam `k_EDrawAndUpdateSharedTexture` LABEL_114).
/// Skipping the quad blanks the overlay and flickers when the game Presents fast.
const KEYED_MUTEX_TIMEOUT_MS: u32 = 0;

const WAIT_ABANDONED: i32 = 0x80;

/// Steam cached `hTexture`: mutex miss draws the last complete snapshot,
/// not the in-flight shared tex (that is the stroboscope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxSample {
    Live,
    Cache,
    Skip,
}

pub fn mailbox_sample(lock_held: bool, has_cache: bool) -> MailboxSample {
    if lock_held {
        MailboxSample::Live
    } else if has_cache {
        MailboxSample::Cache
    } else {
        MailboxSample::Skip
    }
}

/// After AcquireSync: `Ok(true)` = lock held (ReleaseSync after original Present),
/// `Ok(false)` = timeout still-draw without the lock, `Err` = hard fail (skip).
fn keyed_mutex_lock_held(hr: windows::core::HRESULT) -> windows::core::Result<bool> {
    hr.ok()?;
    Ok(hr.0 == 0 || hr.0 == WAIT_ABANDONED)
}

thread_local! {
    static HELD_KEYED_MUTEXES: RefCell<Vec<IDXGIKeyedMutex>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    static RELEASE_AFTER_PRESENT: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
}

fn keyed_mutex_already_held(mutex: &IDXGIKeyedMutex) -> bool {
    let raw = Interface::as_raw(mutex);
    HELD_KEYED_MUTEXES.with(|held| {
        held.borrow()
            .iter()
            .any(|parked| Interface::as_raw(parked) == raw)
    })
}

fn park_keyed_mutex(mutex: &IDXGIKeyedMutex) {
    HELD_KEYED_MUTEXES.with(|held| held.borrow_mut().push(mutex.clone()));
}

fn release_keyed_mutex_holds() {
    #[cfg(test)]
    RELEASE_AFTER_PRESENT.with(|c| c.set(c.get() + 1));
    HELD_KEYED_MUTEXES.with(|held| {
        for mutex in held.borrow_mut().drain(..) {
            unsafe {
                _ = mutex.ReleaseSync(0);
            }
        }
    });
}

/// D5: run original Present/Present1/SwapBuffers/`vkQueuePresentKHR`, then `ReleaseSync`.
#[inline]
pub fn after_original_present<R>(present: impl FnOnce() -> R) -> R {
    struct ReleaseAfterPresent;
    impl Drop for ReleaseAfterPresent {
        fn drop(&mut self) {
            release_keyed_mutex_holds();
        }
    }
    let _release = ReleaseAfterPresent;
    present()
}

/// If [`IDXGIKeyedMutex`],
/// * Exists, acquire the mutex with `0` value key (timeout 0), run closure and release.
/// * Not exists, just run closure.
///
/// Returns `Ok(None)` only if AcquireSync hard-failed. Timeout still runs `f`
/// without the lock — callers that sample the live shared tex will tear;
/// use [`with_keyed_mutex_sampled`] + last-good cache instead.
#[inline]
#[allow(dead_code)]
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
            if !keyed_mutex_lock_held(hr)? {
                return Ok(Some(f()));
            }
            defer!(unsafe {
                _ = mutex.ReleaseSync(0);
            });

            Ok(Some(f()))
        }
        None => Ok(Some(f())),
    }
}

/// Like [`with_keyed_mutex`], but `f(lock_held)` so the caller can sample
/// the last-good cache on timeout instead of the in-flight shared tex.
///
/// A successful AcquireSync is held until [`after_original_present`] (D5).
#[inline]
pub fn with_keyed_mutex_sampled<R>(
    mutex: Option<&IDXGIKeyedMutex>,
    f: impl FnOnce(bool) -> R,
) -> windows::core::Result<Option<R>> {
    match mutex {
        Some(mutex) => {
            if keyed_mutex_already_held(mutex) {
                return Ok(Some(f(true)));
            }
            let hr = unsafe {
                (Interface::vtable(mutex).AcquireSync)(
                    Interface::as_raw(mutex),
                    0,
                    KEYED_MUTEX_TIMEOUT_MS,
                )
            };
            if !keyed_mutex_lock_held(hr)? {
                return Ok(Some(f(false)));
            }
            park_keyed_mutex(mutex);
            Ok(Some(f(true)))
        }
        None => Ok(Some(f(true))),
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::HRESULT;

    const WAIT_TIMEOUT: i32 = 0x102;

    #[test]
    fn keyed_mutex_timeout_ms_is_zero() {
        assert_eq!(KEYED_MUTEX_TIMEOUT_MS, 0);
    }

    #[test]
    fn keyed_mutex_timeout_does_not_skip_draw() {
        let mut drew = false;
        let held =
            keyed_mutex_lock_held(HRESULT(WAIT_TIMEOUT)).expect("timeout is not a hard fail");
        assert!(!held);
        if keyed_mutex_lock_held(HRESULT(WAIT_TIMEOUT)).is_ok() {
            drew = true;
        }
        assert!(drew);
    }

    #[test]
    fn keyed_mutex_hard_fail_skips_draw() {
        assert!(keyed_mutex_lock_held(HRESULT(0x8000_4005u32 as i32)).is_err());
    }

    #[test]
    fn keyed_mutex_ok_holds_lock() {
        assert!(keyed_mutex_lock_held(HRESULT(0)).unwrap());
        assert!(keyed_mutex_lock_held(HRESULT(WAIT_ABANDONED)).unwrap());
    }

    #[test]
    fn timeout_with_cache_samples_cache_not_live() {
        assert_eq!(mailbox_sample(false, true), MailboxSample::Cache);
    }

    #[test]
    fn lock_or_no_cache_samples_live() {
        assert_eq!(mailbox_sample(true, true), MailboxSample::Live);
        assert_eq!(mailbox_sample(true, false), MailboxSample::Live);
        assert_eq!(mailbox_sample(false, false), MailboxSample::Skip);
    }

    #[test]
    fn after_original_present_releases_after_trampoline() {
        RELEASE_AFTER_PRESENT.with(|c| c.set(0));
        let mut releases_during_present = u32::MAX;
        let hr = after_original_present(|| {
            releases_during_present = RELEASE_AFTER_PRESENT.with(|c| c.get());
            7
        });
        assert_eq!(releases_during_present, 0);
        assert_eq!(RELEASE_AFTER_PRESENT.with(|c| c.get()), 1);
        assert_eq!(hr, 7);
    }
}
