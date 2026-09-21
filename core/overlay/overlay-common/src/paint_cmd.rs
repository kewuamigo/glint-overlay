//! Steam `sub_1800BE1F0` paint cmds as a typed overlay-common enum.
//!
//! Packed Steam sizes are the decode-test contract only. Game↔host wire is
//! [`crate::request::WindowRequest`] (bincode). Do not clone `CSharedMemStream`
//! / `GameOverlayRender_SharedTex_*` as a second pipe.
//!
//! Cmd is `i32` (read 4). No case 2. Live IDB 2026-08-15 (`user-ida-pro-mcp`).

use bincode::{Decode, Encode};

/// Steam log for the default jumptable case (including cmd 2).
pub const UNSUPPORTED_RENDER_COMMAND: &str =
    "Unsupported render command (%d) in vgui render stream";

/// Logged when the loop exits without cmd 4.
pub const LEFT_RENDER_LOOP_WITHOUT_END_FRAME: &str = "Left render loop without EndFrame!";

/// Cmd 10, once.
pub const ENABLING_CLEARING_ON_EVERY_FRAME: &str = "Enabling clearing on every frame";

/// Cmd 13. Steam formats `"Taking screenshot via %s"`.
pub const SCREENSHOT_VIA_RENDER_STREAM: &str = "Taking screenshot via render stream";

/// Cmd 22. Steam formats `"Detected Movie hot-key via %s"`.
pub const MOVIE_HOTKEY_VIA_RENDER_STREAM: &str = "Detected Movie hot-key via render stream";

/// VGUI cmd 1, nbytes < 1.
pub const VGUI_TEXTURE_ZERO_BYTES: &str = "Warning: Texture %d is 0 bytes... bad VGUI!";

/// VGUI cmd 1, `nbytes != 4*w*h`.
pub const VGUI_TEXTURE_SIZE_MISMATCH: &str =
    "Texture size(%d) was != width(%u)*height(%u)*4, not loading";

/// Packed payload after the `i32` cmd. Variable cmds are not in this table.
///
/// Verified `r8d` read sizes in `sub_1800BE1F0` (no case 2).
pub fn packed_payload_size(cmd: i32) -> Option<usize> {
    Some(match cmd {
        0 => 8,   // k_EBeginFrame
        3 => 52,  // k_EDrawTexturedRect 0x34
        4 => 0,   // EndFrame
        5 | 6 => 0,
        7 => 2,   // k_ESetCursor WORD
        8 => 1,   // k_EShowCursor byte
        9 => 49,  // k_ESetHotKey 0x31
        10 => 0,  // clear-every-frame
        11 => 4,  // k_EDeleteTexture
        12 | 13 => 0,
        14 => 542, // k_EIMECommand 0x21E
        16 => 2,   // k_EDeleteCursor
        17 => 60,  // k_EDrawAndUpdateSharedTexture 0x3C
        18 | 19 | 20 | 21 | 22 => 0,
        24 | 25 | 26 | 27 | 28 => 0,
        29 => 69, // k_EDrawChromePaintBufferRect 0x45
        30 => 8,  // k_EDeleteChromePaintBuffer
        31 | 32 => 0,
        33 => 4, // k_EOverlayForceDisplayScale (movss)
        _ => return None,
    })
}

/// Typed paint command. Discriminants match Steam cmd values except [`Self::Unsupported`].
#[derive(Debug, Encode, Decode, Clone, PartialEq)]
pub enum PaintCmd {
    /// 0 `k_EBeginFrame` — 8 bytes stored as `a1[1]`.
    BeginFrame {
        /// Stream timestamp (u64).
        timestamp: u64,
    },
    /// 1 `k_ELoadTexture` — VGUI header only; pixels are not uploaded.
    LoadTexture {
        /// Texture id (payload +0).
        id: u32,
        /// Byte count (payload +12).
        nbytes: u32,
        /// Width (payload +16).
        width: u32,
        /// Height (payload +20).
        height: u32,
    },
    /// 3 `k_EDrawTexturedRect` — 52 bytes; VGUI draw, no-op.
    DrawTexturedRect,
    /// 4 unnamed EndFrame — 0 bytes; ends the batch.
    EndFrame,
    /// 5 unnamed → Steam `sub_1800A5FE0`.
    Unnamed5,
    /// 6 unnamed → Steam `sub_1800A5F70`.
    Unnamed6,
    /// 7 `k_ESetCursor` — WORD.
    SetCursor {
        /// Cursor id.
        cursor: u16,
    },
    /// 8 `k_EShowCursor` — 1 byte.
    ShowCursor {
        /// Non-zero shows.
        show: u8,
    },
    /// 9 `k_ESetHotKey` — 49 bytes: six vk/mod pairs + flag.
    SetHotKey {
        /// Six (vk, modifiers) pairs.
        chords: [[u16; 2]; 6],
        /// Trailing flag byte.
        flag: u8,
    },
    /// 10 unnamed clear-every-frame.
    ClearEveryFrame,
    /// 11 `k_EDeleteTexture` — u32 id.
    DeleteTexture {
        /// VGUI texture id.
        id: u32,
    },
    /// 12 unnamed (vtable+104).
    Unnamed12,
    /// 13 unnamed screenshot.
    Screenshot,
    /// 14 `k_EIMECommand` — 542 bytes, inner switch 1–0xC.
    ImeCommand {
        /// Inner command (first dword).
        inner: u32,
    },
    /// 15 `k_ECreateCustomCursor` — 22-byte header; pixels skipped.
    CreateCustomCursor {
        /// Cursor id (first WORD).
        id: u16,
    },
    /// 16 `k_EDeleteCursor` — u16.
    DeleteCursor {
        /// Cursor id.
        id: u16,
    },
    /// 17 `k_EDrawAndUpdateSharedTexture` — existing [`crate::request::UpdateSharedHandle`] /
    /// [`crate::request::UpdateLayerHandle`].
    DrawAndUpdateSharedTexture,
    /// 18 flag on (`byte_18016CDC4 = 1`).
    Flag18On,
    /// 19 flag off.
    Flag18Off,
    /// 20 unnamed → `sub_1800A7640`.
    Unnamed20,
    /// 21 unnamed → `sub_1800A76B0`.
    Unnamed21,
    /// 22 unnamed movie hotkey.
    MovieHotkey,
    /// 23 `k_ESendTextToGame` — u32 len + bytes, len ≤ `0x100000`.
    SendTextToGame {
        /// Bytes (not injected as keyboard).
        text: Vec<u8>,
    },
    /// 24 flag (`byte_18017D2C3 = 1`).
    Flag24,
    /// 25 flag on (`byte_18016CCC1 = 1`).
    Flag25On,
    /// 26 flag off.
    Flag25Off,
    /// 27 flag on (`byte_18016CCC2 = 1`).
    Flag27On,
    /// 28 flag off.
    Flag27Off,
    /// 29 `k_EDrawChromePaintBufferRect` — dest rect + buffer id.
    DrawChromePaintBufferRect {
        /// Chrome buffer id (packed +0x30 / `var_4F4+4`).
        buffer_id: u64,
        /// Dest x (window px on the typed wire; packed floats at +0x10).
        x: f32,
        /// Dest y.
        y: f32,
        /// Dest width.
        width: f32,
        /// Dest height.
        height: f32,
    },
    /// 30 `k_EDeleteChromePaintBuffer` — u64.
    DeleteChromePaintBuffer {
        /// Buffer id.
        buffer_id: u64,
    },
    /// 31 unnamed → `sub_1800620B0(..., 1)`.
    Unnamed31,
    /// 32 unnamed → `sub_1800620B0(..., 0)`.
    Unnamed32,
    /// 33 `k_EOverlayForceDisplayScale` — 4 bytes, `movss`.
    OverlayForceDisplayScale {
        /// Display scale.
        scale: f32,
    },
    /// Default jumptable (cmd 2 and any other unknown).
    Unsupported {
        /// Raw cmd.
        cmd: i32,
    },
}

impl PaintCmd {
    /// Steam integer opcode. [`Self::Unsupported`] returns the raw cmd.
    pub fn opcode(&self) -> i32 {
        match self {
            Self::BeginFrame { .. } => 0,
            Self::LoadTexture { .. } => 1,
            Self::DrawTexturedRect => 3,
            Self::EndFrame => 4,
            Self::Unnamed5 => 5,
            Self::Unnamed6 => 6,
            Self::SetCursor { .. } => 7,
            Self::ShowCursor { .. } => 8,
            Self::SetHotKey { .. } => 9,
            Self::ClearEveryFrame => 10,
            Self::DeleteTexture { .. } => 11,
            Self::Unnamed12 => 12,
            Self::Screenshot => 13,
            Self::ImeCommand { .. } => 14,
            Self::CreateCustomCursor { .. } => 15,
            Self::DeleteCursor { .. } => 16,
            Self::DrawAndUpdateSharedTexture => 17,
            Self::Flag18On => 18,
            Self::Flag18Off => 19,
            Self::Unnamed20 => 20,
            Self::Unnamed21 => 21,
            Self::MovieHotkey => 22,
            Self::SendTextToGame { .. } => 23,
            Self::Flag24 => 24,
            Self::Flag25On => 25,
            Self::Flag25Off => 26,
            Self::Flag27On => 27,
            Self::Flag27Off => 28,
            Self::DrawChromePaintBufferRect { .. } => 29,
            Self::DeleteChromePaintBuffer { .. } => 30,
            Self::Unnamed31 => 31,
            Self::Unnamed32 => 32,
            Self::OverlayForceDisplayScale { .. } => 33,
            Self::Unsupported { cmd } => *cmd,
        }
    }

    /// Format Steam's unsupported-command log line.
    pub fn unsupported_log(cmd: i32) -> String {
        UNSUPPORTED_RENDER_COMMAND.replace("%d", &cmd.to_string())
    }
}

/// Packed Steam stream decode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackedDecodeError {
    /// Buffer shorter than an `i32` cmd.
    TruncatedCmd,
    /// Payload shorter than the verified size.
    TruncatedPayload {
        /// Cmd that needed more bytes.
        cmd: i32,
        /// Expected payload bytes.
        expected: usize,
        /// Bytes remaining.
        got: usize,
    },
}

/// Decode one Steam packed command: `i32` cmd + payload. Does not allocate a
/// second shared-memory stream — tests only.
pub fn decode_packed(buf: &[u8]) -> Result<(PaintCmd, usize), PackedDecodeError> {
    if buf.len() < 4 {
        return Err(PackedDecodeError::TruncatedCmd);
    }
    let cmd = i32::from_le_bytes(buf[0..4].try_into().unwrap());
    let rest = &buf[4..];
    let (cmd, payload_len) = decode_payload(cmd, rest)?;
    Ok((cmd, 4 + payload_len))
}

fn need(cmd: i32, rest: &[u8], n: usize) -> Result<(), PackedDecodeError> {
    if rest.len() < n {
        Err(PackedDecodeError::TruncatedPayload {
            cmd,
            expected: n,
            got: rest.len(),
        })
    } else {
        Ok(())
    }
}

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn f32_at(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn decode_payload(cmd: i32, rest: &[u8]) -> Result<(PaintCmd, usize), PackedDecodeError> {
    if let Some(n) = packed_payload_size(cmd) {
        need(cmd, rest, n)?;
        let body = &rest[..n];
        let parsed = match cmd {
            0 => PaintCmd::BeginFrame {
                timestamp: u64::from_le_bytes(body.try_into().unwrap()),
            },
            3 => PaintCmd::DrawTexturedRect,
            4 => PaintCmd::EndFrame,
            5 => PaintCmd::Unnamed5,
            6 => PaintCmd::Unnamed6,
            7 => PaintCmd::SetCursor {
                cursor: u16::from_le_bytes(body.try_into().unwrap()),
            },
            8 => PaintCmd::ShowCursor { show: body[0] },
            9 => decode_hotkey(body),
            10 => PaintCmd::ClearEveryFrame,
            11 => PaintCmd::DeleteTexture {
                id: u32_at(body, 0),
            },
            12 => PaintCmd::Unnamed12,
            13 => PaintCmd::Screenshot,
            14 => PaintCmd::ImeCommand {
                inner: u32_at(body, 0),
            },
            16 => PaintCmd::DeleteCursor {
                id: u16::from_le_bytes(body.try_into().unwrap()),
            },
            17 => PaintCmd::DrawAndUpdateSharedTexture,
            18 => PaintCmd::Flag18On,
            19 => PaintCmd::Flag18Off,
            20 => PaintCmd::Unnamed20,
            21 => PaintCmd::Unnamed21,
            22 => PaintCmd::MovieHotkey,
            24 => PaintCmd::Flag24,
            25 => PaintCmd::Flag25On,
            26 => PaintCmd::Flag25Off,
            27 => PaintCmd::Flag27On,
            28 => PaintCmd::Flag27Off,
            29 => decode_chrome_rect(body),
            30 => PaintCmd::DeleteChromePaintBuffer {
                buffer_id: u64::from_le_bytes(body.try_into().unwrap()),
            },
            31 => PaintCmd::Unnamed31,
            32 => PaintCmd::Unnamed32,
            33 => PaintCmd::OverlayForceDisplayScale {
                scale: f32_at(body, 0),
            },
            _ => unreachable!("packed_payload_size"),
        };
        return Ok((parsed, n));
    }

    match cmd {
        1 => decode_load_texture(rest),
        15 => decode_create_custom_cursor(rest),
        23 => decode_send_text(rest),
        _ => Ok((PaintCmd::Unsupported { cmd }, 0)),
    }
}

/// 49 bytes: six vk/mod pairs + flag. Pairs stored as u16 (Steam dwords truncated).
fn decode_hotkey(body: &[u8]) -> PaintCmd {
    let mut chords = [[0u16; 2]; 6];
    for (i, slot) in chords.iter_mut().enumerate() {
        let off = i * 8;
        if off + 8 <= body.len() {
            slot[0] = u32_at(body, off) as u16;
            slot[1] = u32_at(body, off + 4) as u16;
        }
    }
    PaintCmd::SetHotKey {
        chords,
        flag: body.get(48).copied().unwrap_or(0),
    }
}

/// Chrome 69-byte blob. buffer_id @ +0x30 (`var_4F4+4`). Dest floats @ +0x10
/// (`var_510`..`var_504`, movss into the draw).
fn decode_chrome_rect(body: &[u8]) -> PaintCmd {
    PaintCmd::DrawChromePaintBufferRect {
        buffer_id: u64::from_le_bytes(body[0x30..0x38].try_into().unwrap()),
        x: f32_at(body, 0x10),
        y: f32_at(body, 0x14),
        width: f32_at(body, 0x18),
        height: f32_at(body, 0x1c),
    }
}

fn decode_load_texture(rest: &[u8]) -> Result<(PaintCmd, usize), PackedDecodeError> {
    need(1, rest, 32)?;
    let nbytes = u32_at(rest, 12);
    let pixel_len = nbytes as usize;
    need(1, rest, 32 + pixel_len)?;
    Ok((
        PaintCmd::LoadTexture {
            id: u32_at(rest, 0),
            nbytes,
            width: u32_at(rest, 16),
            height: u32_at(rest, 20),
        },
        32 + pixel_len,
    ))
}

fn decode_create_custom_cursor(rest: &[u8]) -> Result<(PaintCmd, usize), PackedDecodeError> {
    need(15, rest, 22)?;
    Ok((
        PaintCmd::CreateCustomCursor {
            id: u16::from_le_bytes(rest[0..2].try_into().unwrap()),
        },
        22,
    ))
}

fn decode_send_text(rest: &[u8]) -> Result<(PaintCmd, usize), PackedDecodeError> {
    need(23, rest, 4)?;
    let len = u32_at(rest, 0) as usize;
    if len > 0x100000 {
        return Ok((PaintCmd::Unsupported { cmd: 23 }, 4));
    }
    need(23, rest, 4 + len)?;
    Ok((
        PaintCmd::SendTextToGame {
            text: rest[4..4 + len].to_vec(),
        },
        4 + len,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(cmd: i32, payload: &[u8]) -> Vec<u8> {
        let mut v = cmd.to_le_bytes().to_vec();
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn no_case_2_in_payload_table() {
        assert!(packed_payload_size(2).is_none());
        for cmd in 0..=33 {
            if cmd == 1 || cmd == 2 || cmd == 15 || cmd == 23 {
                continue;
            }
            assert!(
                packed_payload_size(cmd).is_some(),
                "missing fixed size for {cmd}"
            );
        }
    }

    #[test]
    fn decode_begin_frame() {
        let ts: u64 = 0x0102_0304_0506_0708;
        let (cmd, n) = decode_packed(&packed(0, &ts.to_le_bytes())).unwrap();
        assert_eq!(n, 12);
        assert_eq!(cmd, PaintCmd::BeginFrame { timestamp: ts });
        assert_eq!(cmd.opcode(), 0);
    }

    #[test]
    fn decode_end_frame() {
        let (cmd, n) = decode_packed(&packed(4, &[])).unwrap();
        assert_eq!(n, 4);
        assert_eq!(cmd, PaintCmd::EndFrame);
        assert_eq!(cmd.opcode(), 4);
    }

    #[test]
    fn decode_chrome_rect() {
        let mut payload = [0u8; 69];
        payload[0x10..0x14].copy_from_slice(&1.0f32.to_le_bytes());
        payload[0x14..0x18].copy_from_slice(&2.0f32.to_le_bytes());
        payload[0x18..0x1c].copy_from_slice(&3.0f32.to_le_bytes());
        payload[0x1c..0x20].copy_from_slice(&4.0f32.to_le_bytes());
        payload[0x30..0x38].copy_from_slice(&0xAABBCCDDEEFFF001u64.to_le_bytes());
        let (cmd, n) = decode_packed(&packed(29, &payload)).unwrap();
        assert_eq!(n, 4 + 69);
        match cmd {
            PaintCmd::DrawChromePaintBufferRect {
                buffer_id,
                x,
                y,
                width,
                height,
            } => {
                assert_eq!(buffer_id, 0xAABBCCDDEEFFF001);
                assert_eq!((x, y, width, height), (1.0, 2.0, 3.0, 4.0));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(cmd.opcode(), 29);
    }

    #[test]
    fn decode_delete_chrome() {
        let id: u64 = 7;
        let (cmd, n) = decode_packed(&packed(30, &id.to_le_bytes())).unwrap();
        assert_eq!(n, 12);
        assert_eq!(cmd, PaintCmd::DeleteChromePaintBuffer { buffer_id: 7 });
        assert_eq!(cmd.opcode(), 30);
    }

    #[test]
    fn decode_shared_tex() {
        let (cmd, n) = decode_packed(&packed(17, &[0u8; 60])).unwrap();
        assert_eq!(n, 64);
        assert_eq!(cmd, PaintCmd::DrawAndUpdateSharedTexture);
        assert_eq!(cmd.opcode(), 17);
    }

    #[test]
    fn decode_unsupported_cmd_2() {
        let (cmd, n) = decode_packed(&packed(2, &[])).unwrap();
        assert_eq!(n, 4);
        assert_eq!(cmd, PaintCmd::Unsupported { cmd: 2 });
        assert_eq!(
            PaintCmd::unsupported_log(2),
            "Unsupported render command (2) in vgui render stream"
        );
    }

    #[test]
    fn decode_unsupported_unknown() {
        let (cmd, _) = decode_packed(&packed(99, &[])).unwrap();
        assert_eq!(cmd, PaintCmd::Unsupported { cmd: 99 });
        assert_eq!(cmd.opcode(), 99);
    }
}
