//! Steam `sub_1800BE1F0` interpreter on the existing overlay-common proto.
//!
//! Opcode 17 mutex miss → [`MailboxSample::Cache`] (last mailbox). Unknown cmd
//! logs Steam's unsupported string. VGUI tex cmds decode + log/no-op. Cursor /
//! hotkey go through existing SetBlockingCursor / store-hotkey paths — no
//! user32 rewrite, no mouse/keyboard injection from paint.

use glint_overlay_common::{
    cursor::Cursor,
    paint_cmd::{
        ENABLING_CLEARING_ON_EVERY_FRAME, LEFT_RENDER_LOOP_WITHOUT_END_FRAME,
        MOVIE_HOTKEY_VIA_RENDER_STREAM, PaintCmd, SCREENSHOT_VIA_RENDER_STREAM,
    },
    request::HotkeyChord,
};
use tracing::warn;

use crate::util::MailboxSample;

/// Present-time mutex state for opcode 17 (`k_EDrawAndUpdateSharedTexture`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedTexBusy {
    /// `AcquireSync(0)` held.
    pub lock_held: bool,
    /// Last-complete mailbox exists.
    pub has_cache: bool,
}

/// Side effect for the injected backend. Paint never injects mouse/keyboard.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintAction {
    /// No backend mutation.
    None,
    /// Opcode 17 sample policy (still-draw on miss+cache).
    SharedTex(MailboxSample),
    /// Cmds 7/15/16 → existing `set_blocking_cursor` (Interactive also `SetCursor`).
    SetBlockingCursor(Option<Cursor>),
    /// Cmd 8 → keep shape on show; hide clears cursor. Interactive uses the
    /// existing `ShowCursor` passthrough — not a second show-count.
    ShowCursor {
        /// Non-zero shows.
        show: bool,
    },
    /// Cmd 9 → first non-zero chord; same store as `HotKeyAndVisibility`.
    SetHotKey(HotkeyChord),
    /// Cmd 14 → store inner; Interactive uses existing IME path only.
    ImeCommand {
        /// Inner command 1–0xC.
        inner: u32,
    },
    /// Cmd 29 — mailbox dest rect (same DXGI mailbox, not VGUI pixels).
    ChromeDest {
        /// Buffer id (Glint: layer as u64).
        buffer_id: u64,
        /// Dest x in window pixels.
        x: f32,
        /// Dest y.
        y: f32,
        /// Dest width.
        width: f32,
        /// Dest height.
        height: f32,
    },
    /// Cmd 30 — unbind that buffer id.
    DeleteChrome {
        /// Buffer id.
        buffer_id: u64,
    },
}

/// Result of one cmd, including Steam-style log text when applicable.
#[derive(Clone, Debug, PartialEq)]
pub struct InterpretResult {
    /// Backend action.
    pub action: PaintAction,
    /// Steam log line, if any.
    pub log: Option<String>,
}

/// Stream interpreter state (BeginFrame/EndFrame + unnamed flags).
#[derive(Clone, Debug, Default)]
pub struct PaintInterpreter {
    in_frame: bool,
    logged_clear_every_frame: bool,
    clear_every_frame: bool,
    flag_18: bool,
    flag_24: bool,
    flag_25: bool,
    flag_27: bool,
    unnamed31: bool,
    display_scale: f32,
    #[allow(dead_code)]
    last_hotkey: Option<[[u16; 2]; 6]>,
    #[allow(dead_code)]
    last_ime: Option<u32>,
}

impl PaintInterpreter {
    /// New idle interpreter.
    pub fn new() -> Self {
        Self::default()
    }

    /// True between BeginFrame and EndFrame.
    pub fn in_frame(&self) -> bool {
        self.in_frame
    }

    /// End-of-stream: log if EndFrame was missing.
    pub fn finish(&mut self) -> InterpretResult {
        if self.in_frame {
            self.in_frame = false;
            let log = LEFT_RENDER_LOOP_WITHOUT_END_FRAME.to_string();
            warn!("{log}");
            InterpretResult {
                action: PaintAction::None,
                log: Some(log),
            }
        } else {
            InterpretResult {
                action: PaintAction::None,
                log: None,
            }
        }
    }

    /// Apply one typed cmd. `shared` is only used for opcode 17.
    pub fn interpret(&mut self, cmd: &PaintCmd, shared: Option<SharedTexBusy>) -> InterpretResult {
        let mut log = None;
        let action = match cmd {
            PaintCmd::BeginFrame { .. } => {
                self.in_frame = true;
                PaintAction::None
            }
            PaintCmd::EndFrame => {
                self.in_frame = false;
                PaintAction::None
            }
            PaintCmd::LoadTexture {
                id,
                nbytes,
                width,
                height,
            } => {
                log = Some(vgui_load_log(*id, *nbytes, *width, *height));
                if let Some(ref line) = log {
                    warn!("{line}");
                }
                PaintAction::None
            }
            PaintCmd::DrawTexturedRect | PaintCmd::DeleteTexture { .. } => {
                warn!("VGUI textured cmd no-op (no pixel upload)");
                PaintAction::None
            }
            PaintCmd::Unnamed5
            | PaintCmd::Unnamed6
            | PaintCmd::Unnamed12
            | PaintCmd::Unnamed20
            | PaintCmd::Unnamed21 => PaintAction::None,
            PaintCmd::SetCursor { cursor } => {
                PaintAction::SetBlockingCursor(Cursor::from_u32(*cursor as u32))
            }
            PaintCmd::ShowCursor { show } => PaintAction::ShowCursor { show: *show != 0 },
            PaintCmd::SetHotKey { chords, .. } => {
                self.last_hotkey = Some(*chords);
                PaintAction::SetHotKey(first_hotkey(chords))
            }
            PaintCmd::ClearEveryFrame => {
                self.clear_every_frame = true;
                if !self.logged_clear_every_frame {
                    self.logged_clear_every_frame = true;
                    log = Some(ENABLING_CLEARING_ON_EVERY_FRAME.to_string());
                    warn!("{ENABLING_CLEARING_ON_EVERY_FRAME}");
                }
                PaintAction::None
            }
            PaintCmd::Screenshot => {
                log = Some(SCREENSHOT_VIA_RENDER_STREAM.to_string());
                warn!("{SCREENSHOT_VIA_RENDER_STREAM}");
                PaintAction::None
            }
            PaintCmd::ImeCommand { inner } => {
                self.last_ime = Some(*inner);
                PaintAction::ImeCommand { inner: *inner }
            }
            PaintCmd::CreateCustomCursor { .. } => {
                PaintAction::SetBlockingCursor(Some(Cursor::Default))
            }
            PaintCmd::DeleteCursor { .. } => PaintAction::SetBlockingCursor(None),
            PaintCmd::DrawAndUpdateSharedTexture => {
                let sample = shared
                    .map(|s| crate::util::mailbox_sample(s.lock_held, s.has_cache))
                    .unwrap_or(MailboxSample::Live);
                PaintAction::SharedTex(sample)
            }
            PaintCmd::Flag18On => {
                self.flag_18 = true;
                PaintAction::None
            }
            PaintCmd::Flag18Off => {
                self.flag_18 = false;
                PaintAction::None
            }
            PaintCmd::MovieHotkey => {
                log = Some(MOVIE_HOTKEY_VIA_RENDER_STREAM.to_string());
                warn!("{MOVIE_HOTKEY_VIA_RENDER_STREAM}");
                PaintAction::None
            }
            PaintCmd::SendTextToGame { .. } => PaintAction::None,
            PaintCmd::Flag24 => {
                self.flag_24 = true;
                PaintAction::None
            }
            PaintCmd::Flag25On => {
                self.flag_25 = true;
                PaintAction::None
            }
            PaintCmd::Flag25Off => {
                self.flag_25 = false;
                PaintAction::None
            }
            PaintCmd::Flag27On => {
                self.flag_27 = true;
                PaintAction::None
            }
            PaintCmd::Flag27Off => {
                self.flag_27 = false;
                PaintAction::None
            }
            PaintCmd::DrawChromePaintBufferRect {
                buffer_id,
                x,
                y,
                width,
                height,
            } => PaintAction::ChromeDest {
                buffer_id: *buffer_id,
                x: *x,
                y: *y,
                width: *width,
                height: *height,
            },
            PaintCmd::DeleteChromePaintBuffer { buffer_id } => PaintAction::DeleteChrome {
                buffer_id: *buffer_id,
            },
            PaintCmd::Unnamed31 => {
                self.unnamed31 = true;
                PaintAction::None
            }
            PaintCmd::Unnamed32 => {
                self.unnamed31 = false;
                PaintAction::None
            }
            PaintCmd::OverlayForceDisplayScale { scale } => {
                self.display_scale = *scale;
                PaintAction::None
            }
            PaintCmd::Unsupported { cmd } => {
                let line = PaintCmd::unsupported_log(*cmd);
                warn!("{line}");
                log = Some(line);
                PaintAction::None
            }
        };

        InterpretResult { action, log }
    }
}

fn first_hotkey(chords: &[[u16; 2]; 6]) -> HotkeyChord {
    chords
        .iter()
        .find(|[vk, modifiers]| *vk != 0 || *modifiers != 0)
        .map(|[vk, modifiers]| HotkeyChord {
            vk: *vk,
            modifiers: *modifiers,
        })
        .unwrap_or_default()
}

fn vgui_load_log(id: u32, nbytes: u32, width: u32, height: u32) -> String {
    if nbytes < 1 {
        return format!("Warning: Texture {id} is 0 bytes... bad VGUI!");
    }
    let expected = width.saturating_mul(height).saturating_mul(4);
    if nbytes != expected {
        format!("Texture size({nbytes}) was != width({width})*height({height})*4, not loading")
    } else {
        format!("VGUI k_ELoadTexture id={id} skipped (no pixel upload)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_overlay_common::paint_cmd::{decode_packed, packed_payload_size};

    fn packed(cmd: i32, payload: &[u8]) -> Vec<u8> {
        let mut v = cmd.to_le_bytes().to_vec();
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn enum_covers_0_to_33_except_2() {
        for cmd in 0..=33 {
            if cmd == 2 {
                let (decoded, _) = decode_packed(&packed(2, &[])).unwrap();
                assert!(matches!(decoded, PaintCmd::Unsupported { cmd: 2 }));
                continue;
            }
            if packed_payload_size(cmd).is_none() {
                let payload: Vec<u8> = match cmd {
                    1 => vec![0u8; 32],
                    15 => vec![0u8; 22],
                    23 => vec![0u8; 4],
                    _ => continue,
                };
                let (decoded, _) = decode_packed(&packed(cmd, &payload)).unwrap();
                assert_eq!(decoded.opcode(), cmd, "cmd {cmd}");
                continue;
            }
            let n = packed_payload_size(cmd).unwrap();
            let (decoded, _) = decode_packed(&packed(cmd, &vec![0u8; n])).unwrap();
            assert_eq!(decoded.opcode(), cmd, "cmd {cmd}");
        }
    }

    #[test]
    fn unknown_logs_steam_string() {
        let mut interp = PaintInterpreter::new();
        let r = interp.interpret(&PaintCmd::Unsupported { cmd: 2 }, None);
        assert_eq!(
            r.log.as_deref(),
            Some("Unsupported render command (2) in vgui render stream")
        );
        assert_eq!(r.action, PaintAction::None);
    }

    #[test]
    fn opcode_17_busy_still_draws_cache() {
        let mut interp = PaintInterpreter::new();
        let r = interp.interpret(
            &PaintCmd::DrawAndUpdateSharedTexture,
            Some(SharedTexBusy {
                lock_held: false,
                has_cache: true,
            }),
        );
        assert_eq!(r.action, PaintAction::SharedTex(MailboxSample::Cache));
    }

    #[test]
    fn opcode_17_busy_no_cache_skips() {
        let mut interp = PaintInterpreter::new();
        let r = interp.interpret(
            &PaintCmd::DrawAndUpdateSharedTexture,
            Some(SharedTexBusy {
                lock_held: false,
                has_cache: false,
            }),
        );
        assert_eq!(r.action, PaintAction::SharedTex(MailboxSample::Skip));
    }

    #[test]
    fn update_shared_handle_is_opcode_17() {
        assert_eq!(PaintCmd::DrawAndUpdateSharedTexture.opcode(), 17);
    }

    #[test]
    fn begin_end_frame_batch() {
        let mut interp = PaintInterpreter::new();
        interp.interpret(&PaintCmd::BeginFrame { timestamp: 1 }, None);
        assert!(interp.in_frame());
        interp.interpret(&PaintCmd::EndFrame, None);
        assert!(!interp.in_frame());
        assert!(interp.finish().log.is_none());
    }

    #[test]
    fn missing_end_frame_logs() {
        let mut interp = PaintInterpreter::new();
        interp.interpret(&PaintCmd::BeginFrame { timestamp: 0 }, None);
        let r = interp.finish();
        assert_eq!(r.log.as_deref(), Some(LEFT_RENDER_LOOP_WITHOUT_END_FRAME));
    }

    #[test]
    fn chrome_rect_and_delete() {
        let mut interp = PaintInterpreter::new();
        let r = interp.interpret(
            &PaintCmd::DrawChromePaintBufferRect {
                buffer_id: 1,
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            },
            None,
        );
        assert_eq!(
            r.action,
            PaintAction::ChromeDest {
                buffer_id: 1,
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 50.0,
            }
        );
        let r = interp.interpret(&PaintCmd::DeleteChromePaintBuffer { buffer_id: 1 }, None);
        assert_eq!(r.action, PaintAction::DeleteChrome { buffer_id: 1 });
    }

    #[test]
    fn paint_does_not_inject_input() {
        let mut interp = PaintInterpreter::new();
        let text = interp.interpret(
            &PaintCmd::SendTextToGame {
                text: b"hi".to_vec(),
            },
            None,
        );
        assert_eq!(text.action, PaintAction::None);
        let ime = interp.interpret(&PaintCmd::ImeCommand { inner: 1 }, None);
        assert_eq!(ime.action, PaintAction::ImeCommand { inner: 1 });
        assert!(!matches!(
            ime.action,
            PaintAction::SetBlockingCursor(_) | PaintAction::SharedTex(_)
        ));
        let hot = interp.interpret(
            &PaintCmd::SetHotKey {
                chords: [[0x09, 0x1], [0; 2], [0; 2], [0; 2], [0; 2], [0; 2]],
                flag: 0,
            },
            None,
        );
        assert_eq!(
            hot.action,
            PaintAction::SetHotKey(HotkeyChord {
                vk: 0x09,
                modifiers: 0x1,
            })
        );
    }

    #[test]
    fn set_show_create_delete_cursor_have_apply_actions() {
        let mut interp = PaintInterpreter::new();
        assert_eq!(
            interp
                .interpret(&PaintCmd::SetCursor { cursor: 0 }, None)
                .action,
            PaintAction::SetBlockingCursor(Cursor::from_u32(0))
        );
        assert_eq!(
            interp
                .interpret(&PaintCmd::ShowCursor { show: 0 }, None)
                .action,
            PaintAction::ShowCursor { show: false }
        );
        assert_eq!(
            interp
                .interpret(&PaintCmd::ShowCursor { show: 1 }, None)
                .action,
            PaintAction::ShowCursor { show: true }
        );
        assert_eq!(
            interp
                .interpret(&PaintCmd::CreateCustomCursor { id: 3 }, None)
                .action,
            PaintAction::SetBlockingCursor(Some(Cursor::Default))
        );
        assert_eq!(
            interp
                .interpret(&PaintCmd::DeleteCursor { id: 3 }, None)
                .action,
            PaintAction::SetBlockingCursor(None)
        );
    }

    #[test]
    fn first_nonzero_hotkey_pair_wins() {
        let chords = [[0, 0], [0x09, 0x1], [0x20, 0x2], [0; 2], [0; 2], [0; 2]];
        assert_eq!(
            first_hotkey(&chords),
            HotkeyChord {
                vk: 0x09,
                modifiers: 0x1,
            }
        );
        assert_eq!(first_hotkey(&[[0; 2]; 6]), HotkeyChord::default());
    }
}
