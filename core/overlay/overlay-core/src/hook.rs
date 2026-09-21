//! Collection of hooks required to intercept window events and rendering.

mod child;
mod combase;
mod d3d8;
mod ddraw;
mod dinput;
mod dx;
mod fg;
mod gameinput;
mod hid;
mod late;
mod mantle;
mod opengl;
mod proc;
mod setupapi;
mod winmm;
mod xinput;

pub use child::{create_process_w_original, set_child_inject};
pub(crate) use proc::{
    on_message_read, should_consume_pump_message, translate_keydown_for_char,
    with_cursor_passthrough,
};

pub mod util {
    pub use super::dx::original_execute_command_lists;
}

use anyhow::Context;
use windows::Win32::Foundation::HINSTANCE;

#[tracing::instrument]
/// Install various hooks.
pub fn install(_hinstance: HINSTANCE) -> anyhow::Result<()> {
    proc::hook().context("Proc hook failed")?;
    late::graphics_reinstall();
    dinput::hook();
    xinput::hook();
    winmm::hook();
    hid::hook();
    setupapi::hook();
    gameinput::hook();
    combase::hook();
    child::hook();
    late::hook();

    Ok(())
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::HINSTANCE;

    #[test]
    fn install_takes_hinstance_not_hwnd() {
        let _: fn(HINSTANCE) -> anyhow::Result<()> = super::install;
    }
}
