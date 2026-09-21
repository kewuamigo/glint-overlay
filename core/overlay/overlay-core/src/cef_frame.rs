//! Steam CEF-frame wait (D4). Host `SetEvent` after publish; present waits
//! timeout 0 then adaptive ≤ 50 ms (`sub_1800C5AE0` / `sub_1800C1290`).
//! Named Win32 event only — no `__goHost`, no second pipe.

use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use windows::{
    Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, WaitForMultipleObjects},
    },
    core::PCWSTR,
};

pub const ENV_DISABLE_WAIT_FOR_CEF_FRAME: &str = "GLINT_OVERLAY_DISABLE_WAIT_FOR_CEF_FRAME";
pub const CEF_FRAME_WAIT_CAP_MS: u32 = 50;

static FRAME_EVENT: Mutex<Option<isize>> = Mutex::new(None);
static ADAPTIVE_MS: AtomicU32 = AtomicU32::new(0);

pub fn cef_frame_event_name(pid: u32) -> String {
    format!("Local\\GlintCefFrame-{pid}")
}

pub fn cef_frame_wait_disabled(env_val: Option<&str>) -> bool {
    env_val.is_some_and(|s| !s.is_empty())
}

/// Steam: `WaitForMultipleObjects` timeout 0, then adaptive wait ≤ 50 ms.
/// `wait(timeout_ms) -> signaled`. Returns whether the event was signaled.
pub fn wait_cef_frame_with(mut wait: impl FnMut(u32) -> bool, adaptive_ms: &mut u32) -> bool {
    if wait(0) {
        return true;
    }
    let timeout = (*adaptive_ms).clamp(1, CEF_FRAME_WAIT_CAP_MS);
    let signaled = wait(timeout);
    if !signaled {
        let grow = ((CEF_FRAME_WAIT_CAP_MS.saturating_sub(*adaptive_ms)) / 4).max(1);
        *adaptive_ms = (*adaptive_ms + grow).min(CEF_FRAME_WAIT_CAP_MS);
    }
    signaled
}

pub fn wait_for_cef_frame() {
    if cef_frame_wait_disabled(
        std::env::var(ENV_DISABLE_WAIT_FOR_CEF_FRAME)
            .ok()
            .as_deref(),
    ) {
        return;
    }
    let Some(event) = frame_event() else {
        return;
    };
    let mut adaptive = ADAPTIVE_MS.load(Ordering::Relaxed);
    wait_cef_frame_with(|timeout| wait_event(event, timeout), &mut adaptive);
    ADAPTIVE_MS.store(adaptive, Ordering::Relaxed);
}

fn frame_event() -> Option<HANDLE> {
    let mut slot = FRAME_EVENT.lock();
    if let Some(raw) = *slot {
        return Some(HANDLE(raw as *mut _));
    }
    let mut name: Vec<u16> = cef_frame_event_name(std::process::id())
        .encode_utf16()
        .collect();
    name.push(0);
    let h =
        unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(name.as_ptr())) }.ok()?;
    *slot = Some(h.0 as isize);
    Some(h)
}

fn wait_event(event: HANDLE, timeout_ms: u32) -> bool {
    let handles = [event];
    unsafe { WaitForMultipleObjects(&handles, true, timeout_ms) == WAIT_OBJECT_0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_set_skips_wait() {
        assert!(cef_frame_wait_disabled(Some("1")));
        assert!(cef_frame_wait_disabled(Some("true")));
        assert!(!cef_frame_wait_disabled(None));
        assert!(!cef_frame_wait_disabled(Some("")));
    }

    #[test]
    fn timeout_zero_then_adaptive() {
        let mut adaptive = 0;
        let mut timeouts = Vec::new();
        let signaled = wait_cef_frame_with(
            |ms| {
                timeouts.push(ms);
                false
            },
            &mut adaptive,
        );
        assert!(!signaled);
        assert_eq!(timeouts[0], 0);
        assert!(timeouts[1] <= CEF_FRAME_WAIT_CAP_MS);
        assert!(adaptive <= CEF_FRAME_WAIT_CAP_MS);
    }

    #[test]
    fn timeout_zero_signaled_skips_adaptive() {
        let mut adaptive = 12;
        let mut timeouts = Vec::new();
        let signaled = wait_cef_frame_with(
            |ms| {
                timeouts.push(ms);
                true
            },
            &mut adaptive,
        );
        assert!(signaled);
        assert_eq!(timeouts, vec![0]);
        assert_eq!(adaptive, 12);
    }

    #[test]
    fn adaptive_wait_capped_at_50() {
        let mut adaptive = CEF_FRAME_WAIT_CAP_MS;
        let mut timeouts = Vec::new();
        wait_cef_frame_with(
            |ms| {
                timeouts.push(ms);
                false
            },
            &mut adaptive,
        );
        assert_eq!(timeouts, vec![0, CEF_FRAME_WAIT_CAP_MS]);
        assert_eq!(adaptive, CEF_FRAME_WAIT_CAP_MS);
    }

    #[test]
    fn event_name_is_per_session() {
        assert_eq!(cef_frame_event_name(4242), "Local\\GlintCefFrame-4242");
    }
}
