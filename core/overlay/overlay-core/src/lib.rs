//! ## Glint compositor
//! Renders an overlay in front of existing window GPU framebuffers.
//!
//! It hooks various graphics API calls to detect graphical windows in the process.
//! The compositor automatically detects which graphics API the window is using,
//! and chooses a suitable renderer.
//!
//! It can also capture inputs going through the target window.
//! You can listen them or even block them from reaching application handlers.
//!
//! ## Example
//! ```no_run
//! use glint_overlay_core::event_sink::OverlayEventSink;
//! use glint_overlay_core::initialize;
//!
//! let module_handle = 0x1_0000usize; // HINSTANCE from DLL attach
//! initialize(module_handle).expect("initialization failed");
//!
//! OverlayEventSink::add(|event| {
//!     let _ = event;
//! });
//! ```

mod gl;
mod wgl;

pub mod backend;
pub mod event_sink;
pub mod layout;
pub mod surface;

mod capture;
mod cef_frame;
mod compositor;
mod coop;
mod hook;
mod interop;
mod paint_cmd;
mod renderer;
mod resources;
mod texture;
mod types;

pub use capture::ingest_captured_frame;
pub use coop::{b_overlay_needs_present, is_overlay_enabled, overlay_is_using_input};
pub use hook::{create_process_w_original, set_child_inject};
pub use paint_cmd::{InterpretResult, PaintAction, PaintInterpreter, SharedTexBusy};
pub use types::IntDashMap;
pub use util::{MailboxSample, after_original_present, mailbox_sample, with_keyed_mutex_sampled};
mod util;

use anyhow::{Context, bail};
use once_cell::sync::OnceCell;
use windows::Win32::Foundation::HINSTANCE;

/// Module handle of the overlay.
static INSTANCE: OnceCell<usize> = OnceCell::new();

#[inline]
/// Get overlay [`HINSTANCE`]
pub(crate) fn instance() -> HINSTANCE {
    HINSTANCE(*INSTANCE.get().unwrap() as _)
}

/// Initialize overlay, hooks.
///
/// * Calling more than once will fail.
/// * Calling with holding loader lock (DllMain) will fail.
/// * If given `hinstance` is invalid, some resources may not appear correctly.
pub fn initialize(hinstance: usize) -> anyhow::Result<()> {
    if INSTANCE.set(hinstance).is_err() {
        bail!("Already initialized");
    }

    hook::install(HINSTANCE(hinstance as _)).context("hook initialization failed")?;
    Ok(())
}
