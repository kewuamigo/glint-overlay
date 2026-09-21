pub(crate) mod cursor;
pub(crate) mod input_event;
pub(crate) mod input_ll;
pub(crate) mod proc;
pub(crate) mod thread_hooks;

use super::WindowBackend;
use glint_overlay_common::cursor::Cursor;
use glint_overlay_common::request::HotkeyChord;
use windows::Win32::Foundation::RECT;

pub(crate) struct WindowProcData {
    pub position: (i32, i32),

    pub listen_input: ListenInputFlags,
    pub blocking_state: Option<InputBlockData>,
    pub blocking_cursor: Option<Cursor>,
    pub hotkey: HotkeyChord,
    pub last_ime: Option<u32>,

    cursor_state: CursorState,
    ime: ImeState,
    last_click_time: i32,
}

impl WindowProcData {
    pub fn new() -> Self {
        Self {
            position: (0, 0),

            listen_input: ListenInputFlags::empty(),
            blocking_state: None,
            blocking_cursor: Some(Cursor::Default),
            hotkey: HotkeyChord::default(),
            last_ime: None,

            cursor_state: CursorState::Outside,
            ime: ImeState::Disabled,
            last_click_time: 0,
        }
    }

    pub fn reset(&mut self) {
        self.position = (0, 0);
        self.listen_input = ListenInputFlags::empty();
        self.blocking_cursor = Some(Cursor::Default);
        self.hotkey = HotkeyChord::default();
        self.last_ime = None;
    }

    #[inline]
    pub fn listening_cursor(&self) -> bool {
        self.listen_input.contains(ListenInputFlags::CURSOR) || self.blocking_state.is_some()
    }

    #[inline]
    pub fn listening_keyboard(&self) -> bool {
        self.listen_input.contains(ListenInputFlags::KEYBOARD) || self.blocking_state.is_some()
    }

    #[inline]
    pub fn input_blocking(&self) -> bool {
        self.blocking_state.is_some()
    }

    pub fn update_click_time(&mut self, new_time: i32) -> u32 {
        let delta = (new_time as u32).wrapping_sub(self.last_click_time as _);
        self.last_click_time = new_time;
        delta
    }
}

#[derive(Clone, Copy)]
pub(crate) struct InputBlockData {
    pub clip_cursor: Option<RECT>,
    pub old_ime_cx: usize,
    /// ShowCursor display count at Interactive show (Steam `dword_18016CC58`).
    /// Game `ShowCursor` while Interactive mutates this only — not the OS count.
    pub show_count: i32,
    /// `GetCursor` handle saved at show; game `SetCursor` updates this only.
    pub saved_cursor: isize,
    /// `GetClassLongPtrW(hwnd, GCLP_HCURSOR)` saved at show.
    pub class_cursor: usize,
}

impl InputBlockData {
    /// Steam hooked `ShowCursor` while overlay is shown: virtual count only.
    pub fn apply_show_cursor(&mut self, show: bool) -> i32 {
        if show {
            self.show_count += 1;
        } else {
            self.show_count -= 1;
        }
        self.show_count
    }

    /// Steam hooked `SetCursor` while overlay is shown: save handle, return previous.
    pub fn apply_set_cursor(&mut self, cursor: isize) -> isize {
        let prev = self.saved_cursor;
        self.saved_cursor = cursor;
        prev
    }
}

/// Steam `sub_1800A5AC0`: `ShowCursor(TRUE) - 1`, then show until the count is >= 0.
pub(crate) fn show_until_visible(mut show_cursor: impl FnMut(bool) -> i32) -> i32 {
    let saved = show_cursor(true) - 1;
    if saved < 0 {
        while show_cursor(true) < 0 {}
    }
    saved
}

/// Steam `sub_1800A55F0`: drive `ShowCursor` until the display count equals `target`.
pub(crate) fn restore_show_count(target: i32, mut show_cursor: impl FnMut(bool) -> i32) {
    let mut n = show_cursor(true);
    if n > target {
        loop {
            n = show_cursor(false);
            if n <= target {
                break;
            }
        }
    }
    if n < target {
        while show_cursor(true) < target {}
    }
}

/// Current clip is tighter than the desktop/virtual-screen rect.
pub(crate) fn clip_is_tighter(clip: RECT, desktop: RECT) -> bool {
    clip.left > desktop.left
        || clip.top > desktop.top
        || clip.right < desktop.right
        || clip.bottom < desktop.bottom
}

/// Steam restores `GCLP_HCURSOR` only when the saved value is non-zero.
pub(crate) fn class_cursor_to_restore(saved: usize) -> Option<usize> {
    (saved != 0).then_some(saved)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CursorState {
    Inside(i16, i16),
    Outside,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImeState {
    Enabled,
    Compose,
    Disabled,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    /// Flags for listening to input events.
    pub struct ListenInputFlags: u8 {
        /// Listen for cursor events.
        const CURSOR = 0b00000001;
        /// Listen for keyboard events.
        const KEYBOARD = 0b00000010;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_block(show_count: i32, saved_cursor: isize, class_cursor: usize) -> InputBlockData {
        InputBlockData {
            clip_cursor: Some(RECT {
                left: 10,
                top: 10,
                right: 20,
                bottom: 20,
            }),
            old_ime_cx: 0,
            show_count,
            saved_cursor,
            class_cursor,
        }
    }

    #[test]
    fn show_until_visible_saves_count_from_hidden() {
        let mut n = -3;
        let saved = show_until_visible(|show| {
            assert!(show);
            n += 1;
            n
        });
        assert_eq!(saved, -3);
        assert!(n >= 0);
    }

    #[test]
    fn restore_show_count_returns_to_saved_hidden() {
        let mut n = 0;
        restore_show_count(-1, |show| {
            if show {
                n += 1;
            } else {
                n -= 1;
            }
            n
        });
        assert_eq!(n, -1);
    }

    #[test]
    fn restore_show_count_already_at_target() {
        let mut n = 2;
        restore_show_count(2, |show| {
            if show {
                n += 1;
            } else {
                n -= 1;
            }
            n
        });
        assert_eq!(n, 2);
    }

    #[test]
    fn interactive_show_cursor_false_does_not_drop_restore() {
        let mut data = fake_block(3, 0x1234, 0x5678);
        assert_eq!(data.apply_show_cursor(false), 2);
        assert_eq!(data.show_count, 2);
        assert_eq!(data.saved_cursor, 0x1234);
        assert_eq!(data.class_cursor, 0x5678);
        assert!(data.clip_cursor.is_some());
    }

    #[test]
    fn interactive_set_cursor_null_does_not_drop_restore() {
        let mut data = fake_block(3, 0x1234, 0x5678);
        assert_eq!(data.apply_set_cursor(0), 0x1234);
        assert_eq!(data.saved_cursor, 0);
        assert_eq!(data.show_count, 3);
        assert_eq!(data.class_cursor, 0x5678);
        assert!(data.clip_cursor.is_some());
    }

    #[test]
    fn clip_tighter_than_desktop_unclips() {
        let desktop = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let game = RECT {
            left: 100,
            top: 100,
            right: 500,
            bottom: 500,
        };
        assert!(clip_is_tighter(game, desktop));
        assert!(!clip_is_tighter(desktop, desktop));
    }

    #[test]
    fn class_cursor_restore_skips_zero() {
        assert_eq!(class_cursor_to_restore(0), None);
        assert_eq!(class_cursor_to_restore(0xABC), Some(0xABC));
    }
}
