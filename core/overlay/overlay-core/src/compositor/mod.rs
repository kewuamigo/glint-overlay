//! Present-blit padló. Steam injected path has no independent scanout.

/// Off: GameOverlayRenderer64 has no WaitForVBlank / overlay plane present.
/// Scanout is DrawOverlayFrame inside hooked Present only (sub_180093DA0).
pub(crate) const INDEPENDENT_SCANOUT: bool = false;

pub(crate) fn independent_active() -> bool {
    INDEPENDENT_SCANOUT
}

pub(crate) fn ensure_overlay_plane() {
    if !INDEPENDENT_SCANOUT {
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_scanout_is_off() {
        assert!(!INDEPENDENT_SCANOUT);
        assert!(!independent_active());
    }
}
