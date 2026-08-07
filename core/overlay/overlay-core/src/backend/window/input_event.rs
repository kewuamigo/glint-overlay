//! Shared overlay input event builders for WndProc and low-level hooks.

use glint_overlay_event::{
    OverlayEvent, WindowEvent,
    input::{CursorEvent, CursorInput, InputEvent, InputPosition, KeyboardInput},
};

/// Build a cursor [`OverlayEvent`] from window-space coordinates and surface offset.
pub(crate) fn cursor_overlay_event(
    hwnd: u32,
    surface_offset: (i32, i32),
    window: InputPosition,
    event: CursorEvent,
) -> OverlayEvent {
    let client = InputPosition {
        x: window.x - surface_offset.0,
        y: window.y - surface_offset.1,
    };
    OverlayEvent::Window {
        id: hwnd,
        event: WindowEvent::Input(InputEvent::Cursor(CursorInput {
            event,
            client,
            window,
        })),
    }
}

pub(crate) fn keyboard_overlay_event(hwnd: u32, input: KeyboardInput) -> OverlayEvent {
    OverlayEvent::Window {
        id: hwnd,
        event: WindowEvent::Input(InputEvent::Keyboard(input)),
    }
}
