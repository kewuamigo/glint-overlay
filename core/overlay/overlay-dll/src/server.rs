//! Server-side IPC: named-pipe bind, listener loop, and per-client protocol.

use anyhow::Context;
use glint_overlay_common::{
    ipc::{ClientRequest, Frame, ServerResponse, ServerToClientPacket},
    request::{Request, WindowRequest},
};
use glint_overlay_core::{
    backend::{Backends, window::ListenInputFlags},
    event_sink::OverlayEventSink,
};
use glint_overlay_event::{OverlayEvent, WindowEvent};
use bincode::Encode;
use core::time::Duration;
use scopeguard::defer;
use std::ffi::OsStr;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, split},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    sync::mpsc::{UnboundedSender, unbounded_channel},
    time::sleep,
};
use tracing::{debug, error, trace, warn};
use windows::{
    Win32::{
        Foundation::{GENERIC_READ, GENERIC_WRITE},
        Security::{
            ACL, AllocateAndInitializeSid,
            Authorization::{
                EXPLICIT_ACCESS_A, SET_ACCESS, SetEntriesInAclA, TRUSTEE_A, TRUSTEE_IS_SID,
                TRUSTEE_IS_USER,
            },
            FreeSid, InitializeSecurityDescriptor, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID,
            SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SECURITY_WORLD_SID_AUTHORITY,
            SetSecurityDescriptorDacl,
        },
        System::SystemServices::{SECURITY_DESCRIPTOR_REVISION, SECURITY_WORLD_RID},
    },
    core::{BOOL, PSTR},
};

use crate::clients::{SessionCounter, should_cleanup_backends};

/// In-flight `serve_connection` tasks; cleanup runs only when this hits 0.
static SESSIONS: SessionCounter = SessionCounter::new();

/// Which pipe accepted the client — hard-isolates layer ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeRole {
    /// Electron AppShell — layer 0 only (`UpdateSharedHandle`).
    Shell,
    /// Native CEF browser-host — layer ≥1 only (`UpdateLayerHandle`).
    Cef,
}

/// Create a named-pipe server with Everyone read/write access.
pub fn bind_named_pipe(addr: impl AsRef<OsStr>, first: bool) -> anyhow::Result<NamedPipeServer> {
    Ok(unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .create_with_security_attributes_raw(
                addr,
                &mut SECURITY_ATTRIBUTES {
                    nLength: 1,
                    lpSecurityDescriptor: &mut create_everyone_security_desc()
                        .context("failed to create Everyone security desc")?
                        as *mut _ as _,
                    bInheritHandle: BOOL(0),
                } as *mut _ as _,
            )?
    })
}

/// Accept clients on `server`, spawning a new pipe after each session via `create_server`.
pub async fn listen_loop(
    mut server: NamedPipeServer,
    mut create_server: impl FnMut() -> anyhow::Result<NamedPipeServer>,
    role: PipeRole,
) {
    loop {
        debug!(?role, "waiting ipc client...");
        match server.connect().await {
            Ok(_) => {
                let connected = server;
                // Count before recreate/spawn so cleanup can't race the gap.
                SESSIONS.enter();
                // Immediately recreate listener so the next client can connect.
                server = loop {
                    match create_server() {
                        Ok(s) => break s,
                        Err(err) => {
                            error!(
                                "failed to create server. retrying after 5 seconds. err: {err:?}"
                            );
                            sleep(Duration::from_secs(5)).await;
                        }
                    }
                };
                tokio::spawn(async move {
                    if let Err(err) = serve_connection(connected, role).await {
                        warn!("client connection ended unexpectedly. err: {:?}", err);
                    }
                });
            }
            Err(err) => {
                error!("failed to connect to client. err: {err:?}");
            }
        }
    }
}

/// Handle one connected client until the pipe closes.
#[tracing::instrument(skip(server))]
pub async fn serve_connection(server: NamedPipeServer, role: PipeRole) -> anyhow::Result<()> {
    defer!({
        if should_cleanup_backends(SESSIONS.leave()) {
            Backends::cleanup_backends();
        }
    });

    let mut conn = IpcServerConn::new(server).await?;
    let emitter = conn.create_emitter();
    // Register before initial emit so the client receives Added events.
    let client_id = OverlayEventSink::add({
        let emitter = emitter.clone();
        move |event| _ = emitter.emit(event)
    });
    defer!({
        debug!("cleanup start");
        let layers = OverlayEventSink::unbind_client_layers(client_id);
        for layer in layers {
            for backend in Backends::iter() {
                if let Err(err) = backend.update_layer(layer, None) {
                    warn!(
                        "failed to clear layer {layer} on disconnect. err: {:?}",
                        err
                    );
                }
            }
        }
        OverlayEventSink::remove(client_id);
    });

    {
        debug!("sending initial data");
        for backend in Backends::iter() {
            let render = backend.render.lock();
            let gpu_id = render.interop.gpu_id();
            let size = render.window_size;
            _ = emitter.emit(OverlayEvent::Window {
                id: *backend.key() as _,
                event: WindowEvent::Added {
                    width: size.0,
                    height: size.1,
                    gpu_id,
                },
            });
        }
    }

    while let Ok((req_id, req)) = conn.recv().await {
        trace!(?role, "recv id: {req_id} req: {req:?}");

        match req {
            Request::Window { id, request } => {
                conn.reply(
                    req_id,
                    handle_window_event(client_id, id, request, role)?,
                )?;
            }
        }
    }
    Ok(())
}

fn handle_window_event(
    client_id: u64,
    hwnd: u32,
    req: WindowRequest,
    role: PipeRole,
) -> anyhow::Result<bool> {
    let res = Backends::with_backend(hwnd, |backend| {
        match req {
            WindowRequest::SetPosition(position) => {
                if role != PipeRole::Shell {
                    warn!(?role, "rejected SetPosition from non-shell pipe");
                    return false;
                }
                backend.update_layout(|layout| {
                    layout.position = (position.x, position.y);
                });
            }

            WindowRequest::SetAnchor(anchor) => {
                if role != PipeRole::Shell {
                    warn!(?role, "rejected SetAnchor from non-shell pipe");
                    return false;
                }
                backend.update_layout(|layout| {
                    layout.anchor = (anchor.x, anchor.y);
                });
            }

            WindowRequest::SetMargin(margin) => {
                if role != PipeRole::Shell {
                    warn!(?role, "rejected SetMargin from non-shell pipe");
                    return false;
                }
                backend.update_layout(|layout| {
                    layout.margin = (margin.top, margin.right, margin.bottom, margin.left);
                });
            }

            WindowRequest::ListenInput(cmd) => {
                // Input listen is global per game HWND: whichever host owns the
                // session arms it — Electron shell, or the CEF helper once it
                // hosts the shell document (no Electron overlay in the session).
                let mut flags = ListenInputFlags::empty();
                flags.set(ListenInputFlags::CURSOR, cmd.cursor);
                flags.set(ListenInputFlags::KEYBOARD, cmd.keyboard);

                backend.listen_input(flags);
                // CEF sessions start parked (no stamp). Bind layer 1 so keyboard
                // — including Shift+Tab — can route before the first paint.
                if role == PipeRole::Cef && cmd.keyboard {
                    OverlayEventSink::bind_layer(client_id, 1);
                }
            }

            WindowRequest::BlockInput(cmd) => {
                // Same owner rule as ListenInput — the interactive mode owner
                // may be the CEF helper.
                backend.block_input(cmd.block);
            }

            WindowRequest::SetBlockingCursor(cmd) => {
                // Shell (Electron) or CEF browser-host may set resize/hover cursors.
                if role != PipeRole::Shell && role != PipeRole::Cef {
                    warn!(?role, "rejected SetBlockingCursor");
                    return false;
                }
                backend.set_blocking_cursor(cmd.cursor);
            }

            WindowRequest::UpdateSharedHandle(shared) => {
                if role != PipeRole::Shell {
                    warn!(?role, "rejected UpdateSharedHandle — shell pipe only");
                    return false;
                }
                if let Err(err) = backend.update_surface(shared.handle) {
                    error!("failed to open shared surface. err: {:?}", err);
                    return false;
                }
                OverlayEventSink::bind_layer(client_id, 0);
            }

            WindowRequest::UpdateLayerHandle(shared) => {
                if role != PipeRole::Cef {
                    warn!(?role, layer = shared.layer, "rejected UpdateLayerHandle — cef pipe only");
                    return false;
                }
                if shared.layer == 0 {
                    warn!("rejected UpdateLayerHandle layer 0 on cef pipe");
                    return false;
                }
                if let Err(err) = backend.update_layer(shared.layer, shared.handle) {
                    error!("failed to open layer surface. err: {:?}", err);
                    return false;
                }
                OverlayEventSink::bind_layer(client_id, shared.layer);
            }

            WindowRequest::SetLayerPosition(pos) => {
                if role != PipeRole::Cef {
                    warn!(?role, "rejected SetLayerPosition — cef pipe only");
                    return false;
                }
                if pos.layer == 0 {
                    warn!("rejected SetLayerPosition layer 0 on cef pipe");
                    return false;
                }
                backend.set_layer_position(pos.layer, pos.x, pos.y);
            }

            WindowRequest::SetLayerInputRect(input) => {
                if role != PipeRole::Cef {
                    warn!(?role, "rejected SetLayerInputRect — cef pipe only");
                    return false;
                }
                if input.layer == 0 {
                    warn!("rejected SetLayerInputRect layer 0 on cef pipe");
                    return false;
                }
                backend.set_layer_input_rect(input.layer, input.rect);
            }
        }

        true
    });

    Ok(res.unwrap_or(false))
}

fn create_everyone_security_desc() -> anyhow::Result<SECURITY_DESCRIPTOR> {
    let mut everyone_sid = PSID::default();
    unsafe {
        AllocateAndInitializeSid(
            &SECURITY_WORLD_SID_AUTHORITY,
            1,
            SECURITY_WORLD_RID as _,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut everyone_sid,
        )?;
    }
    defer!(unsafe {
        FreeSid(everyone_sid);
    });

    let access = EXPLICIT_ACCESS_A {
        grfAccessPermissions: GENERIC_READ.0 | GENERIC_WRITE.0,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_A {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: PSTR(everyone_sid.0.cast()),
            ..Default::default()
        },
    };

    let mut pacl: *mut ACL = 0 as _;
    unsafe {
        SetEntriesInAclA(Some(&[access]), None, &mut pacl).ok()?;
    }

    let mut security_desc = SECURITY_DESCRIPTOR::default();
    unsafe {
        InitializeSecurityDescriptor(
            PSECURITY_DESCRIPTOR(&mut security_desc as *mut _ as _),
            SECURITY_DESCRIPTOR_REVISION,
        )?;

        SetSecurityDescriptorDacl(
            PSECURITY_DESCRIPTOR(&mut security_desc as *mut _ as _),
            true,
            Some(pacl),
            false,
        )?;
    }

    Ok(security_desc)
}

/// IPC server implementatation.
pub struct IpcServerConn {
    rx: ReadHalf<NamedPipeServer>,
    buf: Vec<u8>,
    chan: UnboundedSender<ServerToClientPacket>,
}

impl IpcServerConn {
    /// Initiate a new [`IpcServerConn`] instance with the given named pipe server.
    pub async fn new(server: NamedPipeServer) -> anyhow::Result<Self> {
        let (rx, mut tx) = split(server);
        let (chan_tx, mut chan_rx) = unbounded_channel();

        tokio::spawn({
            async move {
                let mut buf = Vec::new();
                while let Some(packet) = chan_rx.recv().await {
                    bincode::encode_into_std_write(packet, &mut buf, bincode::config::standard())?;

                    Frame {
                        size: buf.len() as u32,
                    }
                    .write(&mut tx)
                    .await?;
                    tx.write_all(&buf).await?;

                    tx.flush().await?;

                    buf.clear();
                }

                Ok::<_, anyhow::Error>(())
            }
        });

        Ok(Self {
            rx,
            buf: Vec::new(),
            chan: chan_tx,
        })
    }

    /// Create new [`IpcClientEventEmitter`] instance for emitting events to the client.
    pub fn create_emitter(&self) -> IpcClientEventEmitter {
        IpcClientEventEmitter {
            inner: self.chan.clone(),
        }
    }

    /// Read one request from the client.
    pub async fn recv(&mut self) -> anyhow::Result<(u32, Request)> {
        let frame = Frame::read(&mut self.rx).await?;
        self.buf.resize(frame.size as usize, 0_u8);
        self.rx.read_exact(&mut self.buf).await?;

        let packet: ClientRequest =
            bincode::decode_from_slice(&self.buf, bincode::config::standard())?.0;
        Ok((packet.id, packet.req))
    }

    /// Reply to the client with the given request ID and data.
    pub fn reply(&mut self, id: u32, data: impl Encode) -> anyhow::Result<()> {
        _ = self
            .chan
            .send(ServerToClientPacket::Response(ServerResponse {
                id,
                data: bincode::encode_to_vec(data, bincode::config::standard())?,
            }));

        Ok(())
    }
}

/// Event emitter for IPC server.
#[derive(Clone)]
pub struct IpcClientEventEmitter {
    inner: UnboundedSender<ServerToClientPacket>,
}

impl IpcClientEventEmitter {
    /// Emit an event to the client.
    pub fn emit(&self, event: OverlayEvent) -> anyhow::Result<()> {
        self.inner.send(ServerToClientPacket::Event(event))?;

        Ok(())
    }
}
