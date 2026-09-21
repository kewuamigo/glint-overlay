//! Host ↔ CEF IPC protocol (Protobuf `Envelope` for `MSG_PROTO` frames).
//!
//! Schema SSOT: `proto/cef_ipc.proto`. Framing (`[len][msg_type][payload]`) lives
//! in browser-host / CEF — this crate only handles the protobuf payload.

#![deny(clippy::unwrap_used)]

use prost::Message;

include!(concat!(env!("OUT_DIR"), "/gameoverlay.cef.rs"));

/// Current protocol major spoken by this crate (`PROTOCOL_VERSION_1`).
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::ProtocolVersion1;

/// TCP frame `msg_type` for protobuf `Envelope` payloads (design §4.1).
pub const MSG_PROTO: u8 = 2;

/// Encode an [`Envelope`] to protobuf bytes (frame payload only).
pub fn encode_envelope(msg: &Envelope) -> Result<Vec<u8>, prost::EncodeError> {
    let mut buf = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut buf)?;
    Ok(buf)
}

/// Decode an [`Envelope`] from protobuf bytes (frame payload only).
pub fn decode_envelope(buf: &[u8]) -> Result<Envelope, prost::DecodeError> {
    Envelope::decode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Body;

    #[test]
    fn roundtrip_envelope_ready() {
        let original = Envelope {
            protocol_version: PROTOCOL_VERSION as i32,
            correlation_id: 0,
            body: Some(Body::Ready(Ready { chrome_top_px: 130 })),
        };
        let bytes = encode_envelope(&original).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn hello_carries_protocol_version() {
        let original = Envelope {
            protocol_version: ProtocolVersion::ProtocolVersion1 as i32,
            correlation_id: 0,
            body: Some(Body::Hello(Hello {})),
        };
        let bytes = encode_envelope(&original).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");
        assert_eq!(
            decoded.protocol_version,
            ProtocolVersion::ProtocolVersion1 as i32
        );
        assert!(matches!(decoded.body, Some(Body::Hello(_))));
        assert_eq!(PROTOCOL_VERSION, ProtocolVersion::ProtocolVersion1);
    }

    #[test]
    fn key_event_modifiers_and_focus_target() {
        // EVENTFLAG_CONTROL_DOWN | EVENTFLAG_SHIFT_DOWN
        const CTRL_SHIFT: u32 = (1 << 2) | (1 << 1);

        let original = Envelope {
            protocol_version: PROTOCOL_VERSION as i32,
            correlation_id: 0,
            body: Some(Body::KeyEvent(KeyEvent {
                r#type: KeyEventType::RawDown as i32,
                target: FocusTarget::Content as i32,
                modifiers: CTRL_SHIFT,
                windows_key_code: 0x41, // VK_A
                native_key_code: 0x41,
                is_system_key: false,
                character: String::new(),
            })),
        };
        let bytes = encode_envelope(&original).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");
        let Some(Body::KeyEvent(ev)) = decoded.body else {
            panic!("expected KeyEvent body");
        };
        assert_eq!(ev.target, FocusTarget::Content as i32);
        assert_eq!(ev.modifiers, CTRL_SHIFT);
        assert_eq!(ev.r#type, KeyEventType::RawDown as i32);
        assert_eq!(ev.windows_key_code, 0x41);
    }

    #[test]
    fn nav_state_roundtrip_all_fields() {
        let original = Envelope {
            protocol_version: PROTOCOL_VERSION as i32,
            correlation_id: 7,
            body: Some(Body::NavState(NavState {
                url: "https://example.com/path".into(),
                title: "Example".into(),
                loading: true,
                can_go_back: true,
                can_go_forward: false,
            })),
        };
        let bytes = encode_envelope(&original).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");
        let Some(Body::NavState(nav)) = decoded.body else {
            panic!("expected NavState body");
        };
        assert_eq!(nav.url, "https://example.com/path");
        assert_eq!(nav.title, "Example");
        assert!(nav.loading);
        assert!(nav.can_go_back);
        assert!(!nav.can_go_forward);
        assert_eq!(decoded.correlation_id, 7);
    }

    #[test]
    fn shutdown_quit_message_loop_true_and_false() {
        for quit in [true, false] {
            let original = Envelope {
                protocol_version: PROTOCOL_VERSION as i32,
                correlation_id: 0,
                body: Some(Body::Shutdown(Shutdown {
                    quit_message_loop: quit,
                })),
            };
            let bytes = encode_envelope(&original).expect("encode");
            let decoded = decode_envelope(&bytes).expect("decode");
            let Some(Body::Shutdown(s)) = decoded.body else {
                panic!("expected Shutdown body");
            };
            assert_eq!(s.quit_message_loop, quit);
        }
    }
}
