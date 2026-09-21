//! Common types and utilities for IPC communication between the overlay client and server.

use bincode::{Decode, Encode};
use glint_overlay_event::OverlayEvent;
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::request::Request;

/// Creates a unique IPC address for the given process ID and module handle.
/// Because there can be multiple overlays in the same process, we need to distinguish with the module handle.
///
/// This function is used internally by `glint-overlay-client` and `glint-overlay-dll` crates to establish IPC communication.
pub fn create_ipc_addr(pid: u32, module_handle: u32) -> String {
    format!(
        "\\\\.\\pipe\\{}-{pid}-{module_handle}",
        crate::product::PIPE_NAME_PREFIX
    )
}

/// Dedicated pipe for the native CEF browser-host (never shared with Electron shell).
pub fn create_ipc_addr_cef(pid: u32, module_handle: u32) -> String {
    format!("{}-cef", create_ipc_addr(pid, module_handle))
}

/// Derive the CEF pipe from the shell pipe address Electron already holds.
pub fn cef_pipe_from_shell(shell_pipe: &str) -> String {
    if shell_pipe.ends_with("-cef") {
        shell_pipe.to_owned()
    } else {
        format!("{shell_pipe}-cef")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint_cmd::PaintCmd;
    use crate::request::{BlockInput, HotKeyAndVisibility, HotkeyChord, Request, WindowRequest};

    #[test]
    fn ipc_addr_format() {
        let addr = create_ipc_addr(4242, 0x1000);
        assert_eq!(addr, r"\\.\pipe\glint-overlay-4242-4096");
        assert_eq!(
            create_ipc_addr_cef(4242, 0x1000),
            r"\\.\pipe\glint-overlay-4242-4096-cef"
        );
        assert_eq!(
            cef_pipe_from_shell(r"\\.\pipe\glint-overlay-1-2"),
            r"\\.\pipe\glint-overlay-1-2-cef"
        );
    }

    #[test]
    fn request_bincode_roundtrip() {
        let req = Request::Window {
            id: 7,
            request: WindowRequest::BlockInput(BlockInput { block: true }),
        };
        let bytes = bincode::encode_to_vec(&req, bincode::config::standard()).unwrap();
        let (decoded, len): (Request, usize) =
            bincode::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(len, bytes.len());
        assert!(matches!(
            decoded,
            Request::Window {
                id: 7,
                request: WindowRequest::BlockInput(BlockInput { block: true })
            }
        ));
    }

    #[test]
    fn hotkey_and_visibility_bincode_roundtrip() {
        let original = HotKeyAndVisibility {
            visible: true,
            hotkey: HotkeyChord {
                vk: 0x09,
                modifiers: 0x0001,
            },
        };
        let req = Request::Window {
            id: 3,
            request: WindowRequest::HotKeyAndVisibility(original.clone()),
        };
        let bytes = bincode::encode_to_vec(&req, bincode::config::standard()).unwrap();
        let (decoded, len): (Request, usize) =
            bincode::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(len, bytes.len());
        match decoded {
            Request::Window {
                id: 3,
                request: WindowRequest::HotKeyAndVisibility(got),
            } => assert_eq!(got, original),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn paint_cmd_chrome_bincode_roundtrip() {
        let original = PaintCmd::DrawChromePaintBufferRect {
            buffer_id: 1,
            x: 8.0,
            y: 16.0,
            width: 320.0,
            height: 240.0,
        };
        let req = Request::Window {
            id: 1,
            request: WindowRequest::PaintCmd(original.clone()),
        };
        let bytes = bincode::encode_to_vec(&req, bincode::config::standard()).unwrap();
        let (decoded, len): (Request, usize) =
            bincode::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(len, bytes.len());
        match decoded {
            Request::Window {
                id: 1,
                request: WindowRequest::PaintCmd(got),
            } => assert_eq!(got, original),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn update_shared_handle_is_opcode_17() {
        assert_eq!(PaintCmd::DrawAndUpdateSharedTexture.opcode(), 17);
    }
}

/// Describes a request sent from the client to the server.
#[derive(Encode, Decode)]
pub struct ClientRequest {
    /// Unique identifier for matching responses.
    pub id: u32,

    /// The actual request data.
    pub req: Request,
}

/// Describes a response sent from server to client.
#[derive(Encode, Decode)]
pub struct ServerResponse {
    /// Unique identifier matching the request.
    pub id: u32,

    /// The raw response data.
    pub data: Vec<u8>,
}

/// Describes a packet sent from server to client.
#[derive(Encode, Decode)]
pub enum ServerToClientPacket {
    /// The packet is a response to a specific request.
    Response(ServerResponse),

    /// The packet is an event notification.
    Event(OverlayEvent),
}

/// Describes a frame header for IPC communication.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// Size of the frame body in bytes.
    pub size: u32,
}

impl Frame {
    /// Reads a frame header from the given async reader.
    pub async fn read(mut r: impl AsyncRead + Unpin) -> io::Result<Self> {
        Ok(Self {
            size: r.read_u32().await?,
        })
    }

    /// Writes the frame header to the given async writer.
    pub async fn write(self, mut w: impl AsyncWrite + Unpin) -> io::Result<()> {
        w.write_u32(self.size).await?;
        Ok(())
    }
}
