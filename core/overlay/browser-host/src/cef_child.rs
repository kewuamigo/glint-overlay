//! CEF helper spawn + TCP Protobuf framing (`MSG_PROTO` / `Envelope`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use glint_cef_protocol::{
    BridgePush, BridgeResult, CloseExtensionSatellite, ContentNavigate, CreateSession, Envelope,
    GoBack, GoForward, HelloAck, HostUiActionKind, KeyEvent, MSG_PROTO, MouseEvent,
    OpenExtensionSatellite, PROTOCOL_VERSION, Paint, ProtocolVersion, Reload, SessionMode,
    SetContentRect, SetFocus, SetInnerBounds, SetSurfaceSize, Shutdown, WheelEvent,
    decode_envelope, encode_envelope, envelope::Body,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct CefShare {
    pub parent_pid: u32,
    pub luid_low: u32,
    pub luid_high: i32,
}

#[derive(Debug)]
pub enum CefEvent {
    Ready {
        chrome_top_px: u32,
    },
    Paint {
        w: u32,
        h: u32,
        handle: u64,
        layout_generation: u32,
        layer: Option<u32>,
    },
    PaintError {
        msg: String,
    },
    Host(HostAction),
    NavState {
        url: String,
        title: String,
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// `__goHost.invoke` from the hosted page (`contracts/host-bridge.md`).
    BridgeInvoke {
        request_id: u32,
        method: String,
        args_json: String,
        plugin_id: String,
    },
    Exit(Option<i32>),
}

#[derive(Debug, Clone)]
pub struct HostAction {
    /// Thin compat for main (`"close"`, …) — sourced from `HostUiActionKind`.
    pub action: String,
}

pub struct CefSession {
    child: Child,
    writer: Arc<Mutex<OwnedWriteHalf>>,
}

impl CefSession {
    pub async fn start(
        share: CefShare,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<CefEvent>)> {
        let exe = resolve_cef_exe()?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let cwd = exe.parent().context("cef exe has no parent dir")?;

        info!(
            ?exe,
            port,
            parent_pid = share.parent_pid,
            "starting CEF helper"
        );

        let mut cmd = Command::new(&exe);
        cmd.args([
            "--port",
            &port.to_string(),
            "--parent-pid",
            &share.parent_pid.to_string(),
            "--luid-low",
            &share.luid_low.to_string(),
            "--luid-high",
            &share.luid_high.to_string(),
        ]);
        // The helper serves glint-plugin:// bundles out of these dirs.
        for root in crate::apps::asset_roots() {
            cmd.arg("--plugin-root").arg(root);
        }
        cmd.arg("--plugin-shared")
            .arg(crate::apps::shared_deps_root());
        cmd.current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .env("RUST_LOG", "info");
        // Do not set CREATE_NO_WINDOW — it can break Chromium subprocess / OSR paint.
        // Electron's spawn of glint-browser uses windowsHide instead.
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", exe.display()))?;

        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut acc = Vec::new();
                let mut buf = [0u8; 512];
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            acc.extend_from_slice(&buf[..n]);
                            while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
                                let line = String::from_utf8_lossy(&acc[..pos]);
                                let line = line.trim_end_matches(['\r', '\n']).trim();
                                if !line.is_empty() {
                                    warn!(%line, "CEF stderr");
                                }
                                acc.drain(..=pos);
                            }
                        }
                        Err(err) => {
                            debug!(%err, "CEF stderr read ended");
                            break;
                        }
                    }
                }
                if !acc.is_empty() {
                    let line = String::from_utf8_lossy(&acc);
                    let line = line.trim();
                    if !line.is_empty() {
                        warn!(%line, "CEF stderr");
                    }
                }
            });
        }

        let (tx, rx) = mpsc::unbounded_channel();

        let accept = tokio::time::timeout(Duration::from_secs(20), listener.accept());
        let (stream, _) = accept
            .await
            .context("CEF TCP accept timeout")?
            .context("CEF TCP accept failed")?;
        stream.set_nodelay(true)?;
        let (reader, writer) = stream.into_split();
        let writer = Arc::new(Mutex::new(writer));
        tokio::spawn(read_loop(reader, Arc::clone(&writer), tx));

        Ok((Self { child, writer }, rx))
    }

    pub async fn wait_ready(
        &mut self,
        rx: &mut mpsc::UnboundedReceiver<CefEvent>,
        timeout: Duration,
    ) -> anyhow::Result<u32> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                bail!("CEF helper ready timeout");
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Some(CefEvent::Ready { chrome_top_px })) => return Ok(chrome_top_px),
                Ok(Some(CefEvent::Exit(code))) => {
                    bail!("CEF helper exited before ready (code={code:?})")
                }
                Ok(Some(_)) => continue,
                Ok(None) => bail!("CEF event channel closed before ready"),
                Err(_) => bail!("CEF helper ready timeout"),
            }
        }
    }

    pub async fn create(&mut self, w: u32, h: u32, url: &str) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::CreateSession(CreateSession {
            width: w,
            height: h,
            url: url.to_string(),
            mode: SessionMode::Osr as i32,
        })))
        .await
    }

    /// Fullscreen chrome OSR / publish texture size (game client). Inner window unchanged.
    pub async fn set_surface_size(&mut self, w: u32, h: u32) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::SetSurfaceSize(SetSurfaceSize {
            width: w,
            height: h,
        })))
        .await
    }

    /// Inner browser window bounds within the fullscreen surface.
    pub async fn set_bounds(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        layout_generation: u32,
    ) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::SetInnerBounds(SetInnerBounds {
            x,
            y,
            width: w,
            height: h,
            layout_generation,
        })))
        .await
    }

    pub async fn send_mouse(&mut self, ev: MouseEvent) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::MouseEvent(ev))).await
    }

    pub async fn send_wheel(&mut self, ev: WheelEvent) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::WheelEvent(ev))).await
    }

    pub async fn send_key(&mut self, ev: KeyEvent) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::KeyEvent(ev))).await
    }

    /// Give/remove render-widget focus on one OSR browser. Without this the
    /// widget never becomes active, so Blink has no focused frame and keyboard
    /// events have nothing to land on.
    pub async fn set_focus(&mut self, focus: bool, target: CefFocusTarget) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::SetFocus(SetFocus {
            focus,
            target: target as i32,
        })))
        .await
    }

    /// Content-frame navigation behind the `browser.*` bridge methods.
    pub async fn content_navigate(&mut self, url: String) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::ContentNavigate(ContentNavigate { url })))
            .await
    }

    /// Place / clear the content OSR hole inside fullscreen shell chrome.
    pub async fn set_content_rect(
        &mut self,
        rect: Option<(i32, i32, u32, u32)>,
    ) -> anyhow::Result<()> {
        let body = match rect {
            None => SetContentRect {
                clear: true,
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            Some((x, y, w, h)) => SetContentRect {
                clear: false,
                x,
                y,
                width: w,
                height: h,
            },
        };
        self.send_envelope(make_env(Body::SetContentRect(body)))
            .await
    }

    pub async fn go_back(&mut self) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::GoBack(GoBack {}))).await
    }

    pub async fn go_forward(&mut self) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::GoForward(GoForward {})))
            .await
    }

    pub async fn reload(&mut self) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::Reload(Reload {}))).await
    }

    /// Open Chrome-style extension satellite (options / popup). Never converts content_.
    pub async fn open_extension_satellite(
        &mut self,
        extension_id: String,
        kind: ExtensionSatelliteKind,
    ) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::OpenExtensionSatellite(
            OpenExtensionSatellite {
                extension_id,
                kind: kind as i32,
            },
        )))
        .await
    }

    /// Close the Chrome-style extension satellite if any (OSR untouched).
    pub async fn close_extension_satellite(&mut self) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::CloseExtensionSatellite(
            CloseExtensionSatellite {},
        )))
        .await
    }

    /// Settle one `__goHost.invoke`. `result_json` is a JSON value ("" = undefined).
    pub async fn send_bridge_result(
        &mut self,
        request_id: u32,
        result: Result<String, String>,
    ) -> anyhow::Result<()> {
        let (ok, result_json, error) = match result {
            Ok(json) => (true, json, String::new()),
            Err(err) => (false, String::new(), err),
        };
        self.send_envelope(make_env(Body::BridgeResult(BridgeResult {
            request_id,
            ok,
            result_json,
            error,
        })))
        .await
    }

    /// Host → page push (`UiMessage` JSON, delivered as `window.postMessage`).
    pub async fn send_bridge_push(&mut self, message_json: String) -> anyhow::Result<()> {
        self.send_envelope(make_env(Body::BridgePush(BridgePush { message_json })))
            .await
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        let _ = self
            .send_envelope(make_env(Body::Shutdown(Shutdown {
                quit_message_loop: true,
            })))
            .await;
        if tokio::time::timeout(Duration::from_secs(1), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
        Ok(())
    }

    async fn send_envelope(&mut self, env: Envelope) -> anyhow::Result<()> {
        write_envelope(&self.writer, &env).await
    }
}

fn make_env(body: Body) -> Envelope {
    Envelope {
        protocol_version: PROTOCOL_VERSION as i32,
        correlation_id: 0,
        body: Some(body),
    }
}

async fn write_envelope(writer: &Mutex<OwnedWriteHalf>, env: &Envelope) -> anyhow::Result<()> {
    let payload = encode_envelope(env).context("encode Envelope")?;
    let mut frame = Vec::with_capacity(4 + 1 + payload.len());
    let len = (1 + payload.len()) as u32;
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(MSG_PROTO);
    frame.extend_from_slice(&payload);
    writer.lock().await.write_all(&frame).await?;
    Ok(())
}

async fn read_loop(
    mut reader: OwnedReadHalf,
    writer: Arc<Mutex<OwnedWriteHalf>>,
    tx: mpsc::UnboundedSender<CefEvent>,
) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) => {
                let _ = tx.send(CefEvent::Exit(None));
                break;
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) => {
                warn!(%err, "CEF socket read error");
                let _ = tx.send(CefEvent::Exit(None));
                break;
            }
        }
        while buf.len() >= 4 {
            let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if len == 0 || len > 64 * 1024 * 1024 {
                buf.clear();
                break;
            }
            if buf.len() < 4 + len {
                break;
            }
            let payload = buf[4..4 + len].to_vec();
            buf.drain(..4 + len);
            match handle_frame(&payload, &writer).await {
                Ok(Some(ev)) => {
                    if tx.send(ev).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(%err, "CEF frame handle failed");
                }
            }
        }
    }
}

async fn handle_frame(
    payload: &[u8],
    writer: &Mutex<OwnedWriteHalf>,
) -> anyhow::Result<Option<CefEvent>> {
    if payload.is_empty() || payload[0] != MSG_PROTO {
        bail!("unexpected CEF frame type (expected MSG_PROTO)");
    }
    let env = decode_envelope(&payload[1..]).context("decode Envelope")?;
    if env.protocol_version != ProtocolVersion::ProtocolVersion1 as i32 {
        bail!("unsupported CEF protocol_version={}", env.protocol_version);
    }
    match env.body {
        Some(Body::Hello(_)) => {
            write_envelope(writer, &make_env(Body::HelloAck(HelloAck {})))
                .await
                .context("send HelloAck")?;
            Ok(None)
        }
        Some(Body::Ready(r)) => Ok(Some(CefEvent::Ready {
            chrome_top_px: r.chrome_top_px,
        })),
        Some(Body::Paint(Paint {
            width,
            height,
            nt_handle,
            layout_generation,
            layer,
        })) => {
            if width == 0 || height == 0 {
                return Ok(None);
            }
            Ok(Some(CefEvent::Paint {
                w: width,
                h: height,
                handle: nt_handle,
                layout_generation,
                layer,
            }))
        }
        Some(Body::PaintError(e)) => Ok(Some(CefEvent::PaintError {
            msg: if e.message.is_empty() {
                "unknown".into()
            } else {
                e.message
            },
        })),
        Some(Body::NavState(n)) => Ok(Some(CefEvent::NavState {
            url: n.url,
            title: n.title,
            loading: n.loading,
            can_go_back: n.can_go_back,
            can_go_forward: n.can_go_forward,
        })),
        Some(Body::HostUiAction(a)) => {
            let action = match HostUiActionKind::try_from(a.kind) {
                Ok(HostUiActionKind::Close) => "close",
                Ok(HostUiActionKind::Move) => "move",
                Ok(HostUiActionKind::Minimize) => "minimize",
                Ok(HostUiActionKind::SetBounds) => "setBounds",
                _ => return Ok(None),
            };
            Ok(Some(CefEvent::Host(HostAction {
                action: action.to_string(),
            })))
        }
        Some(Body::BridgeInvoke(b)) => Ok(Some(CefEvent::BridgeInvoke {
            request_id: b.request_id,
            method: b.method,
            args_json: b.args_json,
            plugin_id: b.plugin_id,
        })),
        // Handshake noise / host→CEF echoes — ignore.
        Some(Body::HelloAck(_))
        | Some(Body::Error(_))
        | Some(Body::CreateSession(_))
        | Some(Body::Shutdown(_))
        | Some(Body::SetSurfaceSize(_))
        | Some(Body::SetInnerBounds(_))
        | Some(Body::SetContentRect(_))
        | Some(Body::SetFocus(_))
        | Some(Body::SetHidden(_))
        | Some(Body::Navigate(_))
        | Some(Body::ContentNavigate(_))
        | Some(Body::GoBack(_))
        | Some(Body::GoForward(_))
        | Some(Body::Reload(_))
        | Some(Body::OpenExtensionSatellite(_))
        | Some(Body::CloseExtensionSatellite(_))
        | Some(Body::BridgeResult(_))
        | Some(Body::BridgePush(_))
        | Some(Body::KeyEvent(_))
        | Some(Body::MouseEvent(_))
        | Some(Body::WheelEvent(_))
        | None => Ok(None),
    }
}

pub fn resolve_cef_exe() -> anyhow::Result<PathBuf> {
    if let Ok(env) = std::env::var("GLINT_CEF_EXE") {
        let p = PathBuf::from(&env);
        if p.is_file() {
            return Ok(p);
        }
    }

    let mut candidates = Vec::new();
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("../../../host/native/cef/glint-cef.exe"));
    candidates.push(manifest.join("../../../host/cef/build/Release/glint-cef.exe"));
    candidates.push(manifest.join("../../../host/cef/build/Debug/glint-cef.exe"));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("glint-cef.exe"));
            candidates.push(dir.join("cef/glint-cef.exe"));
        }
    }

    for c in &candidates {
        if let Ok(canon) = c.canonicalize() {
            if canon.is_file() {
                return Ok(canon);
            }
        } else if c.is_file() {
            return Ok(c.clone());
        }
    }

    bail!(
        "glint-cef.exe not found. Set GLINT_CEF_EXE or run scripts/build-cef.ps1 \
         (tried host/native/cef and host/cef/build/Release)"
    )
}

/// Document the OSR surface loads for this session — always `ui/shell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDocument {
    /// `ui/shell` document — the overlay shell owns the whole surface.
    Shell(String),
}

impl SessionDocument {
    pub fn url(&self) -> &str {
        match self {
            Self::Shell(url) => url,
        }
    }

    pub fn kind(&self) -> &'static str {
        "shell"
    }
}

/// Requires `GLINT_UI_URL` (launcher always sets `ui/shell/dist`).
pub fn resolve_session_document(shell_url: Option<String>) -> anyhow::Result<SessionDocument> {
    let value = shell_url.filter(|u| !u.trim().is_empty()).ok_or_else(|| {
        anyhow::anyhow!(
            "GLINT_UI_URL required (path to ui/shell/dist). \
                 Attach via the launcher — cef/ui fallback was removed."
        )
    })?;
    shell_document_url(&value).map(SessionDocument::Shell)
}

/// The env value may be a URL (`http://…` from a dev server, `file://…`) or a
/// path to the `ui/shell` dist dir / its `index.html`. Paths become `file://`
/// URLs: the helper already runs CEF with `--allow-file-access-from-files`, and
/// the dist is built with a relative base so its assets resolve there.
fn shell_document_url(value: &str) -> anyhow::Result<String> {
    if value.contains("://") {
        return Ok(value.to_string());
    }
    let path = PathBuf::from(value);
    let index = if path.is_dir() {
        path.join("index.html")
    } else {
        path
    };
    let index = index
        .canonicalize()
        .with_context(|| format!("shell document not found: {}", index.display()))?;
    Ok(path_to_file_url(&index))
}

fn path_to_file_url(path: &Path) -> String {
    // canonicalize() on Windows may yield \\?\C:\... or \\.\C:\...; CEF rejects file://?/C:/...
    let mut s = path.to_string_lossy().replace('\\', "/");
    for prefix in ["//?/", "//./"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

// Re-export FocusTarget for main input routing.
pub use glint_cef_protocol::ExtensionSatelliteKind;
pub use glint_cef_protocol::FocusTarget as CefFocusTarget;
