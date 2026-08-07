use tracing::{debug, trace};

use crate::backend::render::Renderer;

/// Whether the overlay should draw on this frame for the active graphics API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawGate {
    /// Active renderer matches — proceed with compositing.
    Draw,
    /// Renderer was adopted from a compatible API — skip this frame.
    Adopted,
    /// Another renderer is active — skip silently.
    Ignored,
}

/// Pick or migrate the active renderer before DXGI-backed overlay compositing.
pub fn adopt_renderer(
    active: &mut Option<Renderer>,
    target: Renderer,
    migrate_from: &[Renderer],
) -> DrawGate {
    match active {
        Some(current) if *current == target => DrawGate::Draw,
        Some(current) if migrate_from.contains(current) => {
            debug!("switching from {current:?} to {target:?} render");
            *active = Some(target);
            DrawGate::Adopted
        }
        Some(current) => {
            trace!("ignoring {target:?} rendering (active: {current:?})");
            DrawGate::Ignored
        }
        None => {
            *active = Some(target);
            DrawGate::Adopted
        }
    }
}
