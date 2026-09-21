//! Pure session-mode / pin policy (US2). HostCtrl wiring is T017+.
//! Contract: `specs/003-cef-bridge-gaps/contracts/session-mode-hud.md`.

/// Overlay layer state when this helper hosts the shell document.
/// Strings match stdin control channel / shell mode names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Hidden,
    HudPinned,
    Interactive,
}

impl SessionMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "hidden" => Some(Self::Hidden),
            "hud_pinned" => Some(Self::HudPinned),
            "interactive" => Some(Self::Interactive),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::HudPinned => "hud_pinned",
            Self::Interactive => "interactive",
        }
    }
}

/// Next mode for dismiss-from-Interactive or pin-map change (contract transitions
/// 1, 3, 4). Caller passes pin-map emptiness *after* the pin event, or current
/// pins when dismissing Interactive.
pub fn next_mode(from: SessionMode, any_pinned: bool) -> SessionMode {
    match from {
        SessionMode::Interactive if any_pinned => SessionMode::HudPinned,
        SessionMode::Interactive => SessionMode::Hidden,
        SessionMode::HudPinned if !any_pinned => SessionMode::Hidden,
        SessionMode::Hidden if any_pinned => SessionMode::HudPinned,
        mode => mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_with_pins_dismisses_to_hud_pinned() {
        assert_eq!(
            next_mode(SessionMode::Interactive, true),
            SessionMode::HudPinned
        );
    }

    #[test]
    fn interactive_without_pins_dismisses_to_hidden() {
        assert_eq!(
            next_mode(SessionMode::Interactive, false),
            SessionMode::Hidden
        );
    }

    #[test]
    fn last_pin_clear_while_hud_pinned_goes_hidden() {
        assert_eq!(
            next_mode(SessionMode::HudPinned, false),
            SessionMode::Hidden
        );
    }

    #[test]
    fn hidden_plus_first_pin_goes_hud_pinned() {
        assert_eq!(next_mode(SessionMode::Hidden, true), SessionMode::HudPinned);
    }
}
