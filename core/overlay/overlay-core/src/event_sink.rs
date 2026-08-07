//! Provides [`OverlayEventSink`] for receiving [`OverlayEvent`] from overlay system.

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
};

use glint_overlay_event::{
    OverlayEvent, WindowEvent,
    input::{
        CursorAction, CursorEvent, CursorInputState, InputEvent, InputPosition, KeyInputState,
        KeyboardInput,
    },
};
use parking_lot::Mutex;

/// Shift held — Shift+Tab is the shell hotkey and must not follow layer-1 focus.
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
/// Left button held — keep delivering moves to the press-focus layer (resize grow).
static LEFT_DOWN: AtomicBool = AtomicBool::new(false);

type SinkFn = Arc<dyn Fn(OverlayEvent) + Send + Sync>;

struct Inner {
    next: u64,
    sinks: Vec<(u64, SinkFn)>,
    /// First successful binder wins per layer.
    layers: HashMap<u32, u64>,
    /// Per-window layer rects `(layer, x, y, w, h)` ascending — for input hit-test.
    rects: HashMap<u32, Vec<(u32, i32, i32, u32, u32)>>,
    /// Last cursor-hit layer per window (keyboard follows this owner).
    focus_layer: HashMap<u32, u32>,
}

/// Global multi-client event sink registry.
static CURRENT: LazyLock<Mutex<Inner>> = LazyLock::new(|| {
    Mutex::new(Inner {
        next: 1,
        sinks: Vec::new(),
        layers: HashMap::new(),
        rects: HashMap::new(),
        focus_layer: HashMap::new(),
    })
});

/// Event sink for overlay system.
pub struct OverlayEventSink;

impl OverlayEventSink {
    #[inline]
    /// Check if any client event sink is registered.
    pub fn connected() -> bool {
        !CURRENT.lock().sinks.is_empty()
    }

    /// Publish layer rects for hit-testing (called from layout recompute).
    pub(crate) fn set_layer_rects(hwnd: u32, rects: Vec<(u32, i32, i32, u32, u32)>) {
        CURRENT.lock().rects.insert(hwnd, rects);
    }

    /// Bind `layer` to `client` if no owner yet (first binder wins).
    pub fn bind_layer(client: u64, layer: u32) {
        CURRENT.lock().layers.entry(layer).or_insert(client);
    }

    /// Remove all layer bindings owned by `client`.
    ///
    /// Returns the unbound layer ids. Also strips those layers from hit-test
    /// rects / focus so zombie quads cannot steal input.
    pub fn unbind_client_layers(client: u64) -> Vec<u32> {
        let mut inner = CURRENT.lock();
        let layers: Vec<u32> = inner
            .layers
            .iter()
            .filter(|(_, owner)| **owner == client)
            .map(|(layer, _)| *layer)
            .collect();
        inner.layers.retain(|_, owner| *owner != client);
        for rects in inner.rects.values_mut() {
            rects.retain(|(layer, ..)| !layers.contains(layer));
        }
        inner.focus_layer.retain(|_, layer| !layers.contains(layer));
        layers
    }

    /// Owner of `layer`, if bound.
    pub fn owner_of_layer(layer: u32) -> Option<u64> {
        CURRENT.lock().layers.get(&layer).copied()
    }

    /// Deliver `event` to a single sink.
    pub fn emit_to(client: u64, event: OverlayEvent) {
        let sink = CURRENT
            .lock()
            .sinks
            .iter()
            .find(|(id, _)| *id == client)
            .map(|(_, s)| s.clone());
        if let Some(sink) = sink {
            sink(event);
        }
    }

    #[inline]
    /// Emit [`OverlayEvent`].
    ///
    /// Window lifecycle events broadcast to all clients. Input events are
    /// hit-tested to the topmost layer under the cursor and delivered only to
    /// that layer's owner. If no owner is bound, the input event is **dropped**.
    pub(crate) fn emit(event: OverlayEvent) {
        if matches!(
            event,
            OverlayEvent::Window {
                event: WindowEvent::Input(_),
                ..
            }
        ) {
            Self::emit_input(event);
            return;
        }

        let sinks: Vec<_> = CURRENT.lock().sinks.iter().map(|(_, s)| s.clone()).collect();
        for sink in sinks {
            sink(event.clone());
        }
    }

    fn emit_input(event: OverlayEvent) {
        let OverlayEvent::Window {
            id: hwnd,
            event: WindowEvent::Input(input),
        } = &event
        else {
            return;
        };

        let owner = match input {
            InputEvent::Cursor(cursor) => {
                let pressed_left = matches!(
                    &cursor.event,
                    CursorEvent::Action {
                        state: CursorInputState::Pressed { .. },
                        action: CursorAction::Left,
                    }
                );
                let released_left = matches!(
                    &cursor.event,
                    CursorEvent::Action {
                        state: CursorInputState::Released,
                        action: CursorAction::Left,
                    }
                );

                // Capture while LMB is down AND through the Released event itself.
                // Clearing LEFT_DOWN before route drops release outside the old texture
                // rect → browser-host never ends resize_drag.
                let layer = if LEFT_DOWN.load(Ordering::Relaxed) && !pressed_left {
                    CURRENT
                        .lock()
                        .focus_layer
                        .get(hwnd)
                        .copied()
                        .or_else(|| Self::hit_test(hwnd, cursor.window))
                } else {
                    Self::hit_test(hwnd, cursor.window)
                };
                if pressed_left {
                    LEFT_DOWN.store(true, Ordering::Relaxed);
                } else if released_left {
                    LEFT_DOWN.store(false, Ordering::Relaxed);
                }
                if let Some(layer) = layer {
                    CURRENT.lock().focus_layer.insert(*hwnd, layer);
                    Self::owner_of_layer(layer)
                } else {
                    None
                }
            }
            InputEvent::Keyboard(key) => {
                // Shift+Tab toggles the session owner: Electron shell (layer 0)
                // when present, otherwise CEF browser-host (layer 1).
                let mut shift_tab = false;
                if let KeyboardInput::Key { key, state } = key {
                    let vk = key.code.get();
                    if vk == 0x10 {
                        SHIFT_DOWN.store(
                            matches!(state, KeyInputState::Pressed),
                            Ordering::Relaxed,
                        );
                    } else if vk == 0x09
                        && matches!(state, KeyInputState::Pressed)
                        && SHIFT_DOWN.load(Ordering::Relaxed)
                    {
                        shift_tab = true;
                    }
                }
                if shift_tab {
                    Self::owner_of_layer(0).or_else(|| Self::owner_of_layer(1))
                } else {
                    let inner = CURRENT.lock();
                    let layer = inner.focus_layer.get(hwnd).copied().or_else(|| {
                        // No cursor focus yet — topmost layer with an owner.
                        inner.rects.get(hwnd)?.iter().rev().find_map(|(layer, ..)| {
                            inner.layers.contains_key(layer).then_some(*layer)
                        })
                    }).or_else(|| {
                        // Parked CEF: layer may be bound via ListenInput before any
                        // paint rect exists — still deliver keyboard (hotkey Shift).
                        [1u32, 0]
                            .into_iter()
                            .find(|l| inner.layers.contains_key(l))
                    });
                    layer.and_then(|l| inner.layers.get(&l).copied())
                }
            }
        };

        // No owner → drop (do not broadcast input to all clients).
        let Some(owner) = owner else {
            return;
        };

        let event = if let OverlayEvent::Window {
            id,
            event: WindowEvent::Input(InputEvent::Cursor(cursor)),
        } = event
        {
            let layer = CURRENT.lock().focus_layer.get(&id).copied();
            let offset = layer
                .and_then(|l| {
                    CURRENT.lock().rects.get(&id).and_then(|rects| {
                        rects
                            .iter()
                            .find(|(lid, ..)| *lid == l)
                            .map(|(_, x, y, ..)| (*x, *y))
                    })
                })
                .unwrap_or((0, 0));
            let mut cursor = cursor;
            cursor.client = InputPosition {
                x: cursor.window.x - offset.0,
                y: cursor.window.y - offset.1,
            };
            OverlayEvent::Window {
                id,
                event: WindowEvent::Input(InputEvent::Cursor(cursor)),
            }
        } else {
            event
        };

        // Fan Shift to layer 0 so a legacy Electron OverlayInputRouter can arm
        // Shift+Tab even when layer 1 owns keyboard focus (Path C).
        if let OverlayEvent::Window {
            event: WindowEvent::Input(InputEvent::Keyboard(KeyboardInput::Key { key, .. })),
            ..
        } = &event
        {
            if key.code.get() == 0x10 {
                if let Some(shell) = Self::owner_of_layer(0) {
                    if shell != owner {
                        Self::emit_to(shell, event.clone());
                    }
                }
            }
        }

        Self::emit_to(owner, event);
    }

    /// Topmost *bound* layer whose rect contains `window` coords, if any.
    /// Unbound layers are skipped so the next owned layer underneath can win.
    fn hit_test(hwnd: &u32, window: InputPosition) -> Option<u32> {
        let inner = CURRENT.lock();
        let rects = inner.rects.get(hwnd)?;
        // Ascending storage — walk descending so higher layers win.
        for (layer, x, y, w, h) in rects.iter().rev() {
            if !inner.layers.contains_key(layer) {
                continue;
            }
            if window.x >= *x
                && window.y >= *y
                && window.x < x.saturating_add(*w as i32)
                && window.y < y.saturating_add(*h as i32)
            {
                return Some(*layer);
            }
        }
        None
    }

    /// Register an event sink. Returns an id for [`Self::remove`].
    ///
    /// Overlay will not detect windows or render before at least one sink is set.
    pub fn add(sink: impl Fn(OverlayEvent) + Send + Sync + 'static) -> u64 {
        let mut inner = CURRENT.lock();
        let id = inner.next;
        inner.next = inner.next.saturating_add(1);
        inner.sinks.push((id, Arc::new(sink)));
        id
    }

    /// Remove a previously registered event sink and clear its layer bindings.
    pub fn remove(id: u64) {
        let mut inner = CURRENT.lock();
        inner.sinks.retain(|(sid, _)| *sid != id);
        inner.layers.retain(|_, owner| *owner != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_overlay_event::input::{Key, KeyInputState, KeyboardInput};
    use std::sync::Mutex;

    /// Global sink registry is process-wide — serialize these tests.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn key_event(hwnd: u32, vk: u8, pressed: bool) -> OverlayEvent {
        OverlayEvent::Window {
            id: hwnd,
            event: WindowEvent::Input(InputEvent::Keyboard(KeyboardInput::Key {
                key: Key::new(vk, false).expect("vk"),
                state: if pressed {
                    KeyInputState::Pressed
                } else {
                    KeyInputState::Released
                },
            })),
        }
    }

    fn reset_shift() {
        SHIFT_DOWN.store(false, Ordering::Relaxed);
    }

    #[test]
    fn shift_tab_reaches_cef_layer_when_parked_without_rects() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_shift();
        let got = Arc::new(Mutex::new(0usize));
        let got2 = got.clone();
        let id = OverlayEventSink::add(move |_| {
            *got2.lock().unwrap() += 1;
        });
        // ListenInput bind — no paint rects yet.
        OverlayEventSink::bind_layer(id, 1);

        OverlayEventSink::emit(key_event(42, 0x10, true));
        OverlayEventSink::emit(key_event(42, 0x09, true));

        assert_eq!(
            *got.lock().unwrap(),
            2,
            "Shift and Shift+Tab must reach the CEF owner while parked"
        );

        OverlayEventSink::remove(id);
        reset_shift();
    }

    #[test]
    fn shift_tab_prefers_electron_layer0_over_cef() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_shift();
        let shell_got = Arc::new(Mutex::new(0usize));
        let cef_got = Arc::new(Mutex::new(0usize));
        let shell_c = shell_got.clone();
        let cef_c = cef_got.clone();
        let shell = OverlayEventSink::add(move |_| {
            *shell_c.lock().unwrap() += 1;
        });
        let cef = OverlayEventSink::add(move |_| {
            *cef_c.lock().unwrap() += 1;
        });
        OverlayEventSink::bind_layer(shell, 0);
        OverlayEventSink::bind_layer(cef, 1);

        OverlayEventSink::emit(key_event(7, 0x10, true));
        OverlayEventSink::emit(key_event(7, 0x09, true));

        // Shift fans to both (layer-1 focus path + layer-0 fan); Tab (shift_tab) → layer 0 only.
        assert!(
            *shell_got.lock().unwrap() >= 1,
            "Electron shell must receive Shift+Tab"
        );
        assert_eq!(
            *cef_got.lock().unwrap(),
            1,
            "CEF gets Shift only; Tab must not follow layer 1 when layer 0 owns the hotkey"
        );

        OverlayEventSink::remove(shell);
        OverlayEventSink::remove(cef);
        reset_shift();
    }
}
