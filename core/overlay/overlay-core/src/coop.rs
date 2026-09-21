//! Steam cooperative present / using-device hints (path-1to1 §12).
//!
//! `BOverlayNeedsPresent` is a game-facing hint. Present hooks must not Present
//! because of it. `last` is stamped when the export returns true (hooks stay
//! untouched — we do not stamp on blit).

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::System::SystemInformation::GetTickCount;

use crate::backend::Backends;

/// Steam `0x1770` — 6000 ms.
pub const NEEDS_PRESENT_MS: u32 = 0x1770;

static LAST_HINT_MS: AtomicU32 = AtomicU32::new(0);

/// Steam `BOverlayNeedsPresent` math (`0x1800AE420`).
/// `tick` / `last` are `GetTickCount` milliseconds (wrap every ~49.7 days).
pub fn needs_present(tick: u32, last: u32, renderer_wants: bool) -> bool {
    tick < last || tick.wrapping_sub(last) > NEEDS_PRESENT_MS || renderer_wants
}

/// `renderer_wants` = Interactive or a pending mailbox/layer.
pub fn renderer_wants(interactive: bool, pending_mailbox: bool) -> bool {
    interactive || pending_mailbox
}

/// Steam shares one Using* impl; true while Interactive.
pub fn using_device(interactive: bool) -> bool {
    interactive
}

fn is_interactive() -> bool {
    Backends::iter().any(|b| b.proc.lock().blocking_state.is_some())
}

fn pending_mailbox() -> bool {
    Backends::iter().any(|b| b.render.lock().has_pending_mailbox())
}

fn pump_overlay() {
    // Steam analog only. execute_gui closures stay on the GUI thread
    // (on_message_read). Empty pump is what the brief allowed.
}

/// Steam `BOverlayNeedsPresent` @ `0x1800AE420`. Hint only.
pub fn b_overlay_needs_present() -> bool {
    pump_overlay();
    let tick = unsafe { GetTickCount() };
    let last = LAST_HINT_MS.load(Ordering::Relaxed);
    let needs = needs_present(
        tick,
        last,
        renderer_wants(is_interactive(), pending_mailbox()),
    );
    if needs {
        LAST_HINT_MS.store(tick, Ordering::Relaxed);
    }
    needs
}

/// Steam `IsOverlayEnabled` @ `0x1800B07F0`. True once this DLL is attached.
pub fn is_overlay_enabled() -> bool {
    true
}

/// Steam `SteamOverlayIsUsingGamepad` / `Keyboard` / `Mouse` (one impl).
pub fn overlay_is_using_input() -> bool {
    using_device(is_interactive())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_after_6s_needs_present() {
        assert!(needs_present(NEEDS_PRESENT_MS + 1, 0, false));
    }

    #[test]
    fn exactly_6s_does_not_need_present() {
        assert!(!needs_present(NEEDS_PRESENT_MS, 0, false));
    }

    #[test]
    fn under_6s_without_renderer_does_not_need_present() {
        assert!(!needs_present(NEEDS_PRESENT_MS - 1, 0, false));
    }

    #[test]
    fn wraparound_needs_present() {
        assert!(needs_present(10, u32::MAX, false));
    }

    #[test]
    fn renderer_wants_needs_present_even_when_fresh() {
        assert!(needs_present(100, 100, true));
    }

    #[test]
    fn stamp_on_true_resets_the_6s_window() {
        let tick = NEEDS_PRESENT_MS + 1;
        assert!(needs_present(tick, 0, false));
        let last = tick;
        assert!(!needs_present(tick + 100, last, false));
        assert!(needs_present(tick + NEEDS_PRESENT_MS + 1, last, false));
    }

    #[test]
    fn renderer_wants_is_interactive_or_pending_mailbox() {
        assert!(renderer_wants(true, false));
        assert!(renderer_wants(false, true));
        assert!(!renderer_wants(false, false));
    }

    #[test]
    fn using_device_follows_interactive() {
        assert!(using_device(true));
        assert!(!using_device(false));
    }
}
