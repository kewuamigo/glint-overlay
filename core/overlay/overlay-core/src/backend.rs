//! Manage window states for rendering overlays.
//! You can access states for specific window using [`Backends::with_backend`].
//! This allows you to interact with the overlay state of a window, including its layout and rendering data.

#[doc(hidden)]
pub mod render;

pub mod window;

use core::{mem, num::NonZeroU32};
use std::collections::VecDeque;

use anyhow::Context;
use glint_overlay_common::cursor::Cursor;
use glint_overlay_event::{GpuLuid, OverlayEvent, WindowEvent};
use dashmap::mapref::multiple::RefMulti;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tracing::trace;
use window::proc::hooked_wnd_proc;
use windows::Win32::{
    Foundation::{HWND, LPARAM, RECT, WPARAM},
    Graphics::Dxgi::IDXGIAdapter,
    UI::{
        Input::{
            Ime::{HIMC, ImmAssociateContext, ImmCreateContext, ImmDestroyContext},
            KeyboardAndMouse::{GetCapture, ReleaseCapture, SetFocus},
        },
        WindowsAndMessaging::{
            self as msg, ClipCursor, DefWindowProcA, GWLP_WNDPROC, GetClipCursor, GetSystemMetrics,
            PostMessageA, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SetCursor, SetWindowLongPtrA,
            ShowCursor, WNDPROC,
        },
    },
};

use crate::{
    backend::{
        render::RenderData,
        window::{InputBlockData, ListenInputFlags, WindowProcData, cursor::load_cursor, input_ll},
    },
    event_sink::OverlayEventSink,
    interop::DxInterop,
    layout::OverlayLayout,
    types::IntDashMap,
    util::get_client_size,
};

static BACKENDS: Lazy<Backends> = Lazy::new(|| Backends {
    map: IntDashMap::default(),
});

/// Global store for window backends.
pub struct Backends {
    map: IntDashMap<u32, WindowBackend>,
}

impl Backends {
    /// Iterate over all window backends.
    pub fn iter<'a>() -> impl Iterator<Item = RefMulti<'a, u32, WindowBackend>> {
        BACKENDS.map.iter()
    }

    #[must_use]
    /// Run closure with the specified backend, if it exists.
    pub fn with_backend<R>(id: u32, f: impl FnOnce(&WindowBackend) -> R) -> Option<R> {
        Some(f(&*BACKENDS.map.get(&id)?))
    }

    #[doc(hidden)]
    pub fn with_or_init_backend<R>(
        id: u32,
        adapter_fn: impl FnOnce() -> Option<IDXGIAdapter>,
        f: impl FnOnce(&WindowBackend) -> R,
    ) -> anyhow::Result<R> {
        if let Some(backend) = BACKENDS.map.get(&id) {
            return Ok(f(&backend));
        }

        let backend = BACKENDS
            .map
            .entry(id)
            .or_try_insert_with(|| {
                let original_proc: WNDPROC = unsafe {
                    mem::transmute::<isize, WNDPROC>(SetWindowLongPtrA(
                        HWND(id as _),
                        GWLP_WNDPROC,
                        hooked_wnd_proc as *const () as _,
                    ) as _)
                };

                let interop = DxInterop::create(adapter_fn().as_ref())
                    .context("failed to create backend interop dxdevice")?;

                let window_size = get_client_size(HWND(id as _))?;

                OverlayEventSink::emit(OverlayEvent::Window {
                    id,
                    event: WindowEvent::Added {
                        width: window_size.0,
                        height: window_size.1,
                        gpu_id: interop.gpu_id(),
                    },
                });

                Ok::<_, anyhow::Error>(WindowBackend {
                    id,
                    original_proc,
                    layout: Mutex::new(OverlayLayout::new()),
                    proc: Mutex::new(WindowProcData::new()),
                    render: Mutex::new(RenderData::new(interop, window_size)),
                    proc_queue: Mutex::new(VecDeque::new()),
                })
            })?
            .downgrade();

        Ok(f(&backend))
    }

    fn remove_backend(hwnd: HWND) {
        let key = hwnd.0 as u32;
        BACKENDS.map.remove(&key);

        OverlayEventSink::emit(OverlayEvent::Window {
            id: key,
            event: WindowEvent::Destroyed,
        });
    }

    /// Reset backend states for all windows.
    pub fn cleanup_backends() {
        for backend in BACKENDS.map.iter() {
            backend.reset();
        }
    }
}

pub type ProcDispatchFn = Box<dyn FnOnce(&WindowBackend) + Send>;

/// Data associated to a specific window for overlay rendering.
pub struct WindowBackend {
    /// Unique identifier for the window.
    pub id: u32,
    pub(crate) original_proc: WNDPROC,
    pub(crate) layout: Mutex<OverlayLayout>,
    pub(crate) proc: Mutex<WindowProcData>,
    #[doc(hidden)]
    pub render: Mutex<RenderData>,
    pub(crate) proc_queue: Mutex<VecDeque<ProcDispatchFn>>,
}

impl WindowBackend {
    #[tracing::instrument(skip(self))]
    /// Reset the backend state.
    /// This reset all set user settable state.
    pub fn reset(&self) {
        trace!("backend id: {:?} reset", self.id);
        *self.layout.lock() = OverlayLayout::new();
        self.render.lock().reset();
        self.proc.lock().reset();
        self.block_input(false);
    }

    /// Get the locally unique identifier for the GPU.
    /// This is the GPU adapter used by the window to present to surface.
    /// Overlay surface texture must be created with this GPU.
    /// Otherwise, surface cannot be rendered.
    pub fn gpu_luid(&self) -> GpuLuid {
        self.render.lock().interop.gpu_id()
    }

    /// Update overlay surface using the given shared handle (layer 0).
    pub fn update_surface(&self, handle: Option<NonZeroU32>) -> anyhow::Result<()> {
        self.update_layer(0, handle)
    }

    /// Update a specific DXGI layer surface.
    pub fn update_layer(&self, layer: u32, handle: Option<NonZeroU32>) -> anyhow::Result<()> {
        self.render.lock().update_layer(layer, handle)?;
        self.invalidate_layout();
        Ok(())
    }

    /// Set percent position for a DXGI layer.
    pub fn set_layer_position(
        &self,
        layer: u32,
        x: glint_overlay_common::size::PercentLength,
        y: glint_overlay_common::size::PercentLength,
    ) {
        self.render.lock().set_layer_position_pct(layer, x, y);
        self.invalidate_layout();
    }

    /// Set hit-test rect for a DXGI layer (`None` clears → texture bounds).
    pub fn set_layer_input_rect(
        &self,
        layer: u32,
        rect: Option<glint_overlay_common::request::LayerInputRect>,
    ) {
        self.render.lock().set_layer_input_rect(layer, rect);
        self.invalidate_layout();
    }

    /// Get overlay layout.
    pub fn layout(&self) -> OverlayLayout {
        OverlayLayout::clone(&self.layout.lock())
    }

    /// Update overlay layout.
    pub fn update_layout(&self, f: impl FnOnce(&mut OverlayLayout)) {
        f(&mut self.layout.lock());
        self.invalidate_layout();
    }

    /// Invalidate layout and recompute positions for all layers.
    pub fn invalidate_layout(&self) {
        let layout = self.layout.lock().clone();
        let mut render = self.render.lock();
        let window_size = render.window_size;

        // Layer 0
        let surface_size = render
            .surface
            .get()
            .map(|surface| surface.size())
            .unwrap_or((0, 0));
        render.position = if let Some((x, y)) = render.layer0_pos_pct {
            (
                x.resolve(window_size.0 as f32).round() as i32,
                y.resolve(window_size.1 as f32).round() as i32,
            )
        } else {
            layout.calc(surface_size, window_size)
        };

        // Extra layers
        let keys: Vec<u32> = render.layers.keys().copied().collect();
        for key in keys {
            let pos_pct = render.layers.get(&key).and_then(|l| l.pos_pct);
            let position = if let Some((x, y)) = pos_pct {
                (
                    x.resolve(window_size.0 as f32).round() as i32,
                    y.resolve(window_size.1 as f32).round() as i32,
                )
            } else {
                render.layers.get(&key).map(|l| l.position).unwrap_or((0, 0))
            };
            if let Some(layer) = render.layers.get_mut(&key) {
                layer.position = position;
            }
        }

        let position = render.position;
        let rects = render.layer_rects();
        drop(render);

        input_ll::update_route_position(position);
        crate::event_sink::OverlayEventSink::set_layer_rects(self.id, rects);
        self.proc.lock().position = position;
    }

    /// Set which input events are being listened to.
    pub fn listen_input(&self, flags: ListenInputFlags) {
        self.proc.lock().listen_input = flags;
    }

    /// Sets the cursor to be displayed while input is blocked.
    pub fn set_blocking_cursor(&self, cursor: Option<Cursor>) {
        self.proc.lock().blocking_cursor = cursor;
    }

    /// Blocks or unblocks input for the window.
    ///
    /// The state change runs asynchronously on the window's GUI thread and the
    /// closures are idempotent. This method must never lock `proc` on the
    /// calling thread: it is called from the IPC server task, and blocking
    /// there stalls every subsequent request/ack while the GUI thread is busy.
    pub fn block_input(&self, block: bool) {
        if block {
            self.execute_gui(|backend| unsafe {
                // Do not hold the `proc` lock across SetFocus / ImmAssociateContext /
                // DefWindowProcA below: they synchronously dispatch messages back into
                // hooked_wnd_proc, which locks `proc` again and would deadlock the
                // game GUI thread.
                let (position, blocking_cursor) = {
                    let proc = backend.proc.lock();
                    if proc.blocking_state.is_some() {
                        return;
                    }
                    (proc.position, proc.blocking_cursor)
                };

                ShowCursor(true);
                // Always set a visible cursor shape.  blocking_cursor may be None
                // if the renderer hasn't emitted a cursor-changed event yet (first
                // open after Hidden mode), so fall back to the arrow cursor to
                // prevent SetCursor(NULL) which would make the cursor invisible.
                SetCursor(
                    blocking_cursor
                        .and_then(load_cursor)
                        .or_else(|| load_cursor(Cursor::Default)),
                );
                let clip_cursor = {
                    let mut rect = RECT::default();
                    _ = GetClipCursor(&mut rect);
                    let screen = RECT {
                        left: 0,
                        top: 0,
                        right: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                        bottom: GetSystemMetrics(SM_CYVIRTUALSCREEN),
                    };
                    _ = ClipCursor(None);

                    if rect != screen { Some(rect) } else { None }
                };

                let old_ime_cx =
                    ImmAssociateContext(HWND(backend.id as _), ImmCreateContext()).0 as usize;

                // give focus to target window
                _ = SetFocus(Some(HWND(backend.id as _)));

                // In case of ime is already enabled, hide composition windows
                DefWindowProcA(
                    HWND(backend.id as _),
                    msg::WM_IME_SETCONTEXT,
                    WPARAM(1),
                    LPARAM(0),
                );
                backend.proc.lock().blocking_state = Some(InputBlockData {
                    clip_cursor,
                    old_ime_cx,
                });

                input_ll::activate_route(backend.id, position);
                input_ll::acquire();
            });
        } else {
            self.execute_gui(|backend| unsafe {
                let data = {
                    let mut proc = backend.proc.lock();
                    proc.blocking_state.take()
                };
                // Idempotent: not blocking, nothing to undo.
                let Some(data) = data else {
                    return;
                };

                ShowCursor(false);
                if GetCapture().0 as u32 == backend.id {
                    _ = ReleaseCapture();
                }

                _ = ClipCursor(data.clip_cursor.as_ref().map(|r| r as _));
                let ime_cx = ImmAssociateContext(HWND(backend.id as _), HIMC(data.old_ime_cx as _));
                _ = ImmDestroyContext(ime_cx);

                input_ll::deactivate_route(backend.id);
                input_ll::release();

                OverlayEventSink::emit(OverlayEvent::Window {
                    id: backend.id,
                    event: WindowEvent::InputBlockingEnded,
                });
            });
        }
    }

    /// Execute function on the GUI thread.
    /// Calling `execute_gui` inside the closure will deadlock.
    pub fn execute_gui(&self, f: impl FnOnce(&WindowBackend) + Send + 'static) {
        let mut proc_queue = self.proc_queue.lock();
        proc_queue.push_back(Box::new(f));
        unsafe {
            _ = PostMessageA(Some(HWND(self.id as _)), msg::WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
}
