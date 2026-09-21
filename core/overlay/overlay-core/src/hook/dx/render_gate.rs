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
            DrawGate::Draw
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DrawGate, adopt_renderer};
    use crate::backend::render::Renderer;

    #[test]
    fn adopt_none_is_draw() {
        let mut active = None;
        assert_eq!(
            adopt_renderer(&mut active, Renderer::Dx12, &[]),
            DrawGate::Draw
        );
        assert_eq!(active, Some(Renderer::Dx12));
    }

    #[test]
    fn adopt_already_target_is_draw() {
        let mut active = Some(Renderer::Dx12);
        assert_eq!(
            adopt_renderer(&mut active, Renderer::Dx12, &[]),
            DrawGate::Draw
        );
    }

    #[test]
    fn adopt_other_api_is_ignored() {
        let mut active = Some(Renderer::Dx11);
        assert_eq!(
            adopt_renderer(&mut active, Renderer::Dx12, &[]),
            DrawGate::Ignored
        );
        assert_eq!(active, Some(Renderer::Dx11));
    }

    #[test]
    fn adopt_migrate_from_stays_adopted() {
        let mut active = Some(Renderer::Opengl);
        assert_eq!(
            adopt_renderer(&mut active, Renderer::Dx12, &[Renderer::Opengl]),
            DrawGate::Adopted
        );
        assert_eq!(active, Some(Renderer::Dx12));
    }
}
