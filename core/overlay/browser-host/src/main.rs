//! Standalone browser overlay host. Owns CEF; stamps paint onto overlay layer ≥1
//! (shell session = layer 1; opaque Browser dock helper = layer 2).

#![windows_subsystem = "windows"]

mod apps;
mod bridge;
mod cef_child;
mod etw_reader;
mod plugin_achievements;
mod plugin_db;
mod plugin_fs;
mod plugin_ipc;
mod plugin_saves;
mod plugin_storage;
mod session_mode;

use std::time::{Duration, Instant};

use anyhow::Context;
use cef_child::{
    CefEvent, CefFocusTarget, CefSession, CefShare, resolve_session_document,
};
use glint_cef_protocol::{
    KeyEvent, KeyEventType, MouseButton, MouseEvent, MouseEventType, WheelEvent,
};
use glint_gpu_texture::{GpuLuid as DxgiLuid, find_dxgi_adapter};
use glint_overlay_client::{
    IpcClientConn, connect_pipe, surface::OverlaySurface,
};
use glint_overlay_common::{
    cursor::Cursor,
    request::{
        BlockInput, LayerInputRect, ListenInput, SetBlockingCursor, SetLayerInputRect,
        SetLayerPosition, UpdateLayerHandle,
    },
    size::PercentLength,
};
use glint_overlay_event::{
    OverlayEvent, WindowEvent,
    input::{
        CursorAction, CursorEvent, CursorInputState, InputEvent, KeyInputState, KeyboardInput,
        ScrollAxis,
    },
};
use num::FromPrimitive;
use serde_json::Value;
use session_mode::SessionMode;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, MAPVK_VK_TO_VSC, MapVirtualKeyW, VK_CAPITAL, VK_CONTROL,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_NUMLOCK, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_SHIFT,
};

/// Titlebar-only drag band (matches Electron `OverlayInputRouter` / `--chrome-h`).
const TITLEBAR_H: i32 = 34;
const TITLEBAR_WIN_BTNS_W: i32 = 96;
const RESIZE_EDGE_PX: i32 = 10;
/// Coalesce SetInnerBounds during resize/move drag.
const BOUNDS_THROTTLE: Duration = Duration::from_millis(16);
/// Ignore repeated Shift+Tab within this window (auto-repeat / duplicate source).
const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(400);
/// Metrics bridge push while Interactive / HudPinned (FR-004, ≤1 Hz).
const METRICS_PUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Electron `flashToastHud` default for achievement unlocks (~Xbox toast length).
const TOAST_HUD_FLASH: Duration = Duration::from_secs(12);

/// Electron `toastActive()` — hold HudPinned while the flash window is open.
fn toast_active(toast_until: Option<Instant>) -> bool {
    toast_until.is_some_and(|u| Instant::now() < u)
}

/// CEF `EVENTFLAG_*` bits (`cef_types.h`). The IPC contract requires the full
/// mask on every input message, so these are sampled from the OS per event
/// rather than accumulated locally.
const CEF_EVENTFLAG_CAPS_LOCK_ON: u32 = 1 << 0;
const CEF_EVENTFLAG_SHIFT_DOWN: u32 = 1 << 1;
const CEF_EVENTFLAG_CONTROL_DOWN: u32 = 1 << 2;
const CEF_EVENTFLAG_ALT_DOWN: u32 = 1 << 3;
const CEF_EVENTFLAG_NUM_LOCK_ON: u32 = 1 << 8;
const CEF_EVENTFLAG_IS_REPEAT: u32 = 1 << 13;

#[derive(Debug)]
enum HostCtrl {
    Park,
    Unpark,
    Mode(SessionMode),
    /// Async bridge reply (e.g. `game.saves.*`) — never await rebuild on select.
    BridgeResult {
        request_id: u32,
        result: Result<String, String>,
    },
}

/// Overlay hotkey read from the DLL key stream, same rule as the Electron
/// `OverlayInputRouter`: left Shift held, then Tab pressed.
#[derive(Default)]
struct Hotkey {
    shift_down: bool,
    last_toggle: Option<Instant>,
}

impl Hotkey {
    /// True when this event is the overlay toggle (and must not reach CEF).
    fn consume(&mut self, input: &KeyboardInput) -> bool {
        let KeyboardInput::Key { key, state } = input else {
            return false;
        };
        let down = matches!(state, KeyInputState::Pressed);
        if key.code.get() == 0x10 && !key.extended {
            self.shift_down = down;
            return false;
        }
        if key.code.get() != 0x09 || !down || !self.shift_down {
            return false;
        }
        // Auto-repeat (~30ms) and LL+pump duplicates would each flip the mode.
        // Same window as the Electron host's SHIFT_TAB_DEBOUNCE_MS.
        let now = Instant::now();
        if self
            .last_toggle
            .is_some_and(|t| now.duration_since(t) < HOTKEY_DEBOUNCE)
        {
            return false;
        }
        self.last_toggle = Some(now);
        true
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ResizeEdges {
    n: bool,
    s: bool,
    e: bool,
    w: bool,
}

impl ResizeEdges {
    fn any(self) -> bool {
        self.n || self.s || self.e || self.w
    }

    fn cursor(self) -> Cursor {
        match (self.n, self.s, self.e, self.w) {
            (true, false, true, false) | (false, true, false, true) => {
                Cursor::NorthEastSouthWestResize
            }
            (true, false, false, true) | (false, true, true, false) => {
                Cursor::NorthWestSouthEastResize
            }
            (true, _, false, false) | (_, true, false, false) => Cursor::NorthSouthResize,
            (false, false, true, _) | (false, false, _, true) => Cursor::EastWestResize,
            _ => Cursor::Default,
        }
    }
}

struct ResizeDrag {
    edges: ResizeEdges,
    last_wx: i32,
    last_wy: i32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Always log stamp path — Electron pipes stderr as native-browser.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    // This process relays every input event to CEF, so its scheduling latency
    // is directly on the hover/keystroke path. At the default NORMAL class it
    // loses quanta to games running ABOVE_NORMAL or HIGH. Not fatal if it
    // fails — it only costs latency.
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
    }
    info!(build = "KEYDIAG-1", "browser-host starting");

    let pipe = std::env::var("GLINT_PIPE").context("GLINT_PIPE required")?;
    // Must be the dedicated CEF pipe (`…-cef`). Reject accidental shell-pipe connect.
    if !pipe.ends_with("-cef") {
        anyhow::bail!(
            "GLINT_PIPE must be the CEF pipe (…-cef), got {pipe} — refuse shell-pipe crosstalk"
        );
    }
    let game_pid: Option<u32> = std::env::var("GLINT_GAME_PID")
        .ok()
        .and_then(|s| s.parse().ok());
    if let Some(pid) = game_pid {
        info!(%pid, "GLINT_GAME_PID");
        // Electron overlay-session: EtwReader.start(pid) + metrics pump.
        etw_reader::start(pid);
    }
    info!(%pipe, "connecting CEF-only overlay pipe");

    let (mut conn, mut events) = connect_pipe(&pipe, Some(Duration::from_secs(10)))
        .await
        .with_context(|| {
            format!(
                "CEF pipe connect failed ({pipe}). Injected overlay DLL must listen on ...-cef — restart the game after rebuilding the DLL"
            )
        })?;

    let (win_id, mut win_w, mut win_h, gpu_id) = loop {
        let ev = events.recv().await.context("event stream closed")?;
        if let OverlayEvent::Window {
            id,
            event: WindowEvent::Added {
                width,
                height,
                gpu_id,
            },
        } = ev
        {
            break (id, width, height, gpu_id);
        }
    };
    info!(win_id, win_w, win_h, ?gpu_id, "window added");

    let document = resolve_session_document(
        std::env::var("GLINT_UI_URL")
            .ok()
            .filter(|u| !u.trim().is_empty()),
    )?;
    // Shell document — this helper owns the session (no Electron overlay host).
    let owns_session = true;
    let overlay_layer: u32 = std::env::var("GLINT_LAYER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    info!(overlay_layer, owns_session, "overlay layer");
    if owns_session {
        // Keyboard stays armed in every mode so Shift+Tab still reaches us
        // while the overlay is hidden.
        let armed = conn
            .window(win_id)
            .request(ListenInput {
                cursor: false,
                keyboard: true,
            })
            .await;
        if !matches!(armed, Ok(true)) {
            warn!(
                ?armed,
                "DLL rejected ListenInput on the CEF pipe — Shift+Tab cannot reach the helper. \
                 Rebuild and stage the overlay DLL."
            );
        }
    }

    let adapter = find_dxgi_adapter(DxgiLuid {
        low: gpu_id.low,
        high: gpu_id.high,
    })?;
    let mut surface: OverlaySurface = OverlaySurface::new(adapter.as_ref())?;

    // The shell owns the whole game client; the browser document keeps the
    // floating panel geometry (and with it the host-drawn titlebar/resize).
    let (mut pos_x, mut pos_y, mut browser_w, mut browser_h) = if owns_session {
        (0.0_f32, 0.0_f32, win_w.max(1), win_h.max(1))
    } else {
        floating_panel_geom(win_w, win_h)
    };
    let mut stamped = false;
    let mut content_blank = true;
    let mut focus_target = CefFocusTarget::Chrome;
    let mut mode = SessionMode::Hidden;
    let mut hotkey = Hotkey::default();
    // Shared with bridge `panel.setPinned`; T019/T020 read via `bridge::any_pinned`.
    let pins = bridge::new_pin_map();
    // A helper-owned session starts hidden and is shown by the hotkey.
    let mut parked = owns_session;
    // Shell React BrowserPanel hole — window-space rect for content OSR + hit-test.
    let mut content_hole: Option<(i32, i32, u32, u32)> = None;
    // Sticky target while a button is held (shell resize/drag must not lose
    // events when the cursor slips into the content hole).
    let mut mouse_capture: Option<CefFocusTarget> = None;
    let mut titlebar_drag: Option<(i32, i32)> = None;
    let mut resize_drag: Option<ResizeDrag> = None;
    let mut last_cursor = Cursor::Default;
    let mut next_layout_generation: u32 = 1;
    let mut last_sent_generation: u32 = 0;
    let mut last_bounds_sent: Option<Instant> = None;

    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel::<HostCtrl>();
    let (toast_tx, mut toast_rx) = mpsc::unbounded_channel::<plugin_achievements::UnlockToast>();
    plugin_achievements::install_toast_sender(toast_tx);
    plugin_achievements::spawn_watchers();
    let mut toast_until: Option<Instant> = None;
    tokio::spawn({
        let ctrl_tx = ctrl_tx.clone();
        async move {
            let mut lines = BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match v.get("op").and_then(|o| o.as_str()) {
                    Some("park") => {
                        let _ = ctrl_tx.send(HostCtrl::Park);
                    }
                    Some("unpark") => {
                        let _ = ctrl_tx.send(HostCtrl::Unpark);
                    }
                    Some("mode") => {
                        if let Some(mode) = v
                            .get("mode")
                            .and_then(|m| m.as_str())
                            .and_then(SessionMode::parse)
                        {
                            let _ = ctrl_tx.send(HostCtrl::Mode(mode));
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    let parent_pid = std::process::id();
    let (mut cef, mut cef_rx) = CefSession::start(CefShare {
        parent_pid,
        luid_low: gpu_id.low,
        luid_high: gpu_id.high,
    })
    .await?;
    // SSOT from CEF Ready; Option kept for late Ready updates / pre-Ready safety.
    let mut chrome_top_px = Some(cef.wait_ready(&mut cef_rx, Duration::from_secs(20)).await?);

    // Kept for closeSession chrome restore (R9); same URL used at create.
    let shell_url = document.url().to_string();
    info!(
        win_w,
        win_h,
        browser_w,
        browser_h,
        %shell_url,
        document = document.kind(),
        parent_pid,
        "CEF create (surface=game)"
    );
    // Surface = game client; then place inner window.
    cef.create(win_w, win_h, &shell_url).await?;
    sync_inner_bounds(
        &mut conn,
        &mut cef,
        win_id,
        overlay_layer,
        pos_x,
        pos_y,
        browser_w,
        browser_h,
        true,
        &mut next_layout_generation,
        &mut last_sent_generation,
        &mut last_bounds_sent,
    )
    .await?;
    // The OSR widget starts unfocused, and nothing else grants focus: without
    // this the chrome browser has no focused frame until the cursor happens to
    // cross into content, so early keystrokes go nowhere.
    let _ = cef.set_focus(true, focus_target).await;

    // Shell UiMessage `connection` — same shape as former Electron overlay-session.
    // Best-effort early push; metrics tick re-pushes once the page is listening.
    if let Some(pid) = game_pid {
        if let Err(err) = cef.send_bridge_push(connection_json(pid)).await {
            warn!(%err, "connection push failed");
        }
    }

    let mut keys = KeyState::default();
    // Seed from the OS: a modifier held before the overlay opened is otherwise
    // invisible to the event stream.
    keys.resync();

    let mut metrics_tick = tokio::time::interval(METRICS_PUSH_INTERVAL);
    metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = metrics_tick.tick() => {
                // Electron flashToastHud expiry → drop back to Hidden when no pins.
                if let Some(until) = toast_until {
                    if Instant::now() >= until {
                        toast_until = None;
                        if owns_session
                            && mode == SessionMode::HudPinned
                            && !bridge::any_pinned(&pins)
                        {
                            let _ = ctrl_tx.send(HostCtrl::Mode(SessionMode::Hidden));
                        }
                    }
                }
                // Electron parity: connection rides the metrics pump so a late
                // shell mount still sees pid/connected (all modes; ≤1 Hz).
                if let Some(pid) = game_pid {
                    let _ = cef.send_bridge_push(connection_json(pid)).await;
                }
                if !matches!(mode, SessionMode::Interactive | SessionMode::HudPinned) {
                    continue;
                }
                // Same snapshot JSON as native.metrics.getSnapshot; skip when
                // SHM/pid unavailable (FR-004 "when feasible").
                if let Ok(payload) = plugin_ipc::try_metrics_snapshot() {
                    if let Err(err) = cef
                        .send_bridge_push(
                            serde_json::json!({
                                "type": "metrics",
                                "payload": payload,
                            })
                            .to_string(),
                        )
                        .await
                    {
                        warn!(%err, "metrics push failed");
                    }
                }
            }
            toast = toast_rx.recv() => {
                let Some(t) = toast else { continue };
                // Hidden stops painting — flash HudPinned so the Xbox toast shows.
                let now = Instant::now();
                toast_until = Some(match toast_until {
                    Some(prev) if prev > now => prev + TOAST_HUD_FLASH,
                    _ => now + TOAST_HUD_FLASH,
                });
                if owns_session && mode == SessionMode::Hidden {
                    let _ = ctrl_tx.send(HostCtrl::Mode(SessionMode::HudPinned));
                }
                if let Err(err) = cef
                    .send_bridge_push(
                        serde_json::json!({
                            "type": "achievement",
                            "gameName": t.game_name,
                            "title": t.title,
                            "iconUrl": t.icon_url,
                        })
                        .to_string(),
                    )
                    .await
                {
                    warn!(%err, "achievement toast push failed");
                }
            }
            ctrl = ctrl_rx.recv() => {
                match ctrl {
                    Some(HostCtrl::Park) => {
                        if parked {
                            continue;
                        }
                        parked = true;
                        resize_drag = None;
                        titlebar_drag = None;
                        info!(overlay_layer, "park — clear layer, keep CEF alive");
                        clear_layer(
                            &mut conn,
                            &mut surface,
                            win_id,
                            overlay_layer,
                            &mut stamped,
                        )
                        .await;
                        last_cursor = Cursor::Default;
                        let _ = conn
                            .window(win_id)
                            .request(SetBlockingCursor {
                                cursor: Some(Cursor::Default),
                            })
                            .await;
                    }
                    Some(HostCtrl::Unpark) => {
                        if !parked {
                            continue;
                        }
                        parked = false;
                        // Keys released during the park were never observed, so
                        // held state (and with it the modifier mask) is stale.
                        keys.resync();
                        info!(browser_w, browser_h, "unpark — resume stamps");
                        // set_bounds → Invalidate (not panel set_size nudge).
                        let _ = sync_inner_bounds(
                            &mut conn,
                            &mut cef,
                            win_id,
                            overlay_layer,
                            pos_x,
                            pos_y,
                            browser_w,
                            browser_h,
                            true,
                            &mut next_layout_generation,
                            &mut last_sent_generation,
                            &mut last_bounds_sent,
                        )
                        .await;
                    }
                    Some(HostCtrl::Mode(next)) => {
                        if !owns_session {
                            warn!(?next, "ignoring mode — shell owns this session");
                            continue;
                        }
                        if next == mode {
                            continue;
                        }
                        mode = next;
                        // session-mode-hud.md: Hidden parks; HudPinned+Interactive
                        // stamp; BlockInput/cursor/overlayOpen only Interactive.
                        let interactive = mode == SessionMode::Interactive;
                        let stamp =
                            matches!(mode, SessionMode::HudPinned | SessionMode::Interactive);
                        info!(?mode, "apply session mode");
                        // Keyboard listen stays armed in all modes (Shift+Tab).
                        let blocked = conn
                            .window(win_id)
                            .request(BlockInput { block: interactive })
                            .await;
                        let listened = conn
                            .window(win_id)
                            .request(ListenInput {
                                cursor: interactive,
                                keyboard: true,
                            })
                            .await;
                        if !matches!(blocked, Ok(true)) || !matches!(listened, Ok(true)) {
                            warn!(
                                ?mode,
                                ?blocked,
                                ?listened,
                                "DLL rejected BlockInput/ListenInput — game input state does not \
                                 match the overlay mode. Rebuild and stage the overlay DLL."
                            );
                        }
                        // The shell renders from this push (`UiMessage`), so it
                        // must go out for every mode change, not just the first.
                        if let Err(err) = cef
                            .send_bridge_push(
                                serde_json::json!({
                                    "type": "chrome",
                                    "overlayOpen": interactive,
                                })
                                .to_string(),
                            )
                            .await
                        {
                            warn!(%err, "chrome push failed");
                        }
                        // HudPinned still stamps the chrome layer. Shell React is
                        // visibility:hidden — without clearing the content hole,
                        // the page OSR floats alone over the game (YouTube ghost).
                        if interactive {
                            if let Some(hole) = content_hole {
                                let _ = cef.set_content_rect(Some(hole)).await;
                            }
                        } else {
                            let _ = cef.set_content_rect(None).await;
                        }
                        let _ = ctrl_tx.send(if stamp {
                            HostCtrl::Unpark
                        } else {
                            HostCtrl::Park
                        });
                    }
                    Some(HostCtrl::BridgeResult { request_id, result }) => {
                        if let Err(ref err) = result {
                            warn!(%err, "bridge invoke rejected");
                        }
                        if let Err(err) = cef.send_bridge_result(request_id, result).await {
                            warn!(%err, "bridge result send failed");
                        }
                    }
                    None => {}
                }
            }
            ev = cef_rx.recv() => {
                match ev {
                    Some(CefEvent::Paint {
                        w,
                        h,
                        handle,
                        layout_generation,
                    }) => {
                        if parked {
                            continue;
                        }
                        if layout_generation != last_sent_generation {
                            continue;
                        }
                        // Same path as Electron layer 0: shared texture → UpdateLayerHandle.
                        if let Err(err) = stamp_layer(
                            &mut conn,
                            &mut surface,
                            win_id,
                            overlay_layer,
                            &mut stamped,
                            w,
                            h,
                            handle,
                        )
                        .await
                        {
                            warn!(%err, "stamp failed");
                            clear_layer(&mut conn, &mut surface, win_id, overlay_layer, &mut stamped).await;
                        }
                    }
                    Some(CefEvent::PaintError { msg }) => {
                        warn!(%msg, "CEF paintError");
                        clear_layer(&mut conn, &mut surface, win_id, overlay_layer, &mut stamped).await;
                        // UpdateLayerHandle(None) cleared input_rect — restore before next stamp.
                        let _ = sync_inner_bounds(
                            &mut conn,
                            &mut cef,
                            win_id,
                            overlay_layer,
                            pos_x,
                            pos_y,
                            browser_w,
                            browser_h,
                            true,
                            &mut next_layout_generation,
                            &mut last_sent_generation,
                            &mut last_bounds_sent,
                        )
                        .await;
                    }
                    Some(CefEvent::Host(action)) => {
                        if action.action == "close" {
                            info!("CEF host close");
                            clear_layer(
                                &mut conn,
                                &mut surface,
                                win_id,
                                overlay_layer,
                                &mut stamped,
                            )
                            .await;
                            let _ = cef.shutdown().await;
                            return Ok(());
                        }
                    }
                    Some(CefEvent::Ready { chrome_top_px: v }) => {
                        chrome_top_px = Some(v);
                        // Navigate remounts #app — re-push SetInnerBounds so
                        // floating panel geometry sticks (PushWindowBoundsToUi).
                        if !parked {
                            let _ = sync_inner_bounds(
                                &mut conn,
                                &mut cef,
                                win_id,
                                overlay_layer,
                                pos_x,
                                pos_y,
                                browser_w,
                                browser_h,
                                true,
                                &mut next_layout_generation,
                                &mut last_sent_generation,
                                &mut last_bounds_sent,
                            )
                            .await;
                        }
                    }
                    Some(CefEvent::NavState {
                        url,
                        title,
                        loading,
                        can_go_back,
                        can_go_forward,
                    }) => {
                        content_blank = url.is_empty() || url == "about:blank";
                        debug!(%url, content_blank, "cef navState");
                        // Shell/SDK UiMessage shape (`sdk/bridge` browser.navState).
                        if let Err(err) = cef
                            .send_bridge_push(
                                serde_json::json!({
                                    "type": "browser.navState",
                                    "url": url,
                                    "title": title,
                                    "loading": loading,
                                    "canGoBack": can_go_back,
                                    "canGoForward": can_go_forward,
                                })
                                .to_string(),
                            )
                            .await
                        {
                            warn!(%err, "browser.navState push failed");
                        }
                    }
                    Some(CefEvent::BridgeInvoke { request_id, method, args_json, plugin_id }) => {
                        // Ludusavi rebuild can take tens of seconds — spawn off
                        // the select task; reply via HostCtrl::BridgeResult.
                        if method.starts_with("game.saves.") && !plugin_id.is_empty() {
                            let ctrl_tx = ctrl_tx.clone();
                            let plugin_id = plugin_id.clone();
                            let method = method.clone();
                            let args_json = args_json.clone();
                            tokio::spawn(async move {
                                let result = plugin_ipc::dispatch_saves_async(
                                    &plugin_id, &method, &args_json,
                                )
                                .await;
                                let _ = ctrl_tx.send(HostCtrl::BridgeResult { request_id, result });
                            });
                            continue;
                        }
                        // overlay.* / native.window|overlay.* / browser.focus|blur —
                        // mode changes reply via HostCtrl so Mode applies first.
                        if !plugin_id.is_empty() && plugin_ipc::needs_host_session(&method) {
                            match plugin_ipc::gate_plugin(&plugin_id, &method) {
                                Err(err) => {
                                    warn!(%method, %err, "bridge invoke rejected");
                                    if let Err(err) =
                                        cef.send_bridge_result(request_id, Err(err)).await
                                    {
                                        warn!(%err, "bridge result send failed");
                                    }
                                }
                                Ok(_) => {
                                    let hold = bridge::any_pinned(&pins)
                                        || toast_active(toast_until);
                                    if let Some(result) = dispatch_plugin_session(
                                        &method,
                                        &args_json,
                                        mode,
                                        hold,
                                        owns_session,
                                        &pins,
                                        win_id,
                                        request_id,
                                        &ctrl_tx,
                                        &mut conn,
                                        &mut cef,
                                        &mut focus_target,
                                        &mut content_hole,
                                        &mut content_blank,
                                    )
                                    .await
                                    {
                                        if let Err(ref err) = result {
                                            warn!(%method, %err, "bridge invoke rejected");
                                        }
                                        if let Err(err) =
                                            cef.send_bridge_result(request_id, result).await
                                        {
                                            warn!(%err, "bridge result send failed");
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                        let result = match method.as_str() {
                            "ui.toggleInteractive" if plugin_id.is_empty() => {
                                let next = if mode == SessionMode::Interactive {
                                    session_mode::next_mode(
                                        mode,
                                        bridge::any_pinned(&pins) || toast_active(toast_until),
                                    )
                                } else {
                                    SessionMode::Interactive
                                };
                                let _ = ctrl_tx.send(HostCtrl::Mode(next));
                                Ok(String::new())
                            }
                            // Browser nav — bridge → CEF content frame
                            // (`contracts/host-bridge.md` rule 4).
                            "browser.navigate" if plugin_id.is_empty() => {
                                match serde_json::from_str::<(String,)>(&args_json) {
                                    Ok((url,)) => {
                                        content_blank =
                                            url.is_empty() || url == "about:blank";
                                        nav_ack(cef.content_navigate(url).await)
                                    }
                                    Err(err) => {
                                        Err(format!("browser.navigate needs a url: {err}"))
                                    }
                                }
                            }
                            "browser.reload" if plugin_id.is_empty() => {
                                nav_ack(cef.reload().await)
                            }
                            "browser.goBack" if plugin_id.is_empty() => {
                                nav_ack(cef.go_back().await)
                            }
                            "browser.goForward" if plugin_id.is_empty() => {
                                nav_ack(cef.go_forward().await)
                            }
                            // Shell React Browser AppWindow lifecycle (content hole).
                            "browser.openSession" if plugin_id.is_empty() => {
                                if !owns_session {
                                    Err("browser.openSession requires shell session owner".into())
                                } else {
                                    if mode != SessionMode::Interactive {
                                        let _ =
                                            ctrl_tx.send(HostCtrl::Mode(SessionMode::Interactive));
                                    }
                                    if let Err(err) = cef
                                        .send_bridge_push(
                                            serde_json::json!({
                                                "type": "browserSession",
                                                "open": true,
                                            })
                                            .to_string(),
                                        )
                                        .await
                                    {
                                        warn!(%err, "browserSession open push failed");
                                    }
                                    Ok(String::new())
                                }
                            }
                            "browser.closeSession" if plugin_id.is_empty() => {
                                content_hole = None;
                                mouse_capture = None;
                                let _ = cef.set_content_rect(None).await;
                                let _ = cef.content_navigate("about:blank".into()).await;
                                content_blank = true;
                                if focus_target != CefFocusTarget::Chrome {
                                    let _ = cef.set_focus(false, focus_target).await;
                                    let _ = cef.set_focus(true, CefFocusTarget::Chrome).await;
                                    focus_target = CefFocusTarget::Chrome;
                                }
                                if let Err(err) = cef
                                    .send_bridge_push(
                                        serde_json::json!({
                                            "type": "browserSession",
                                            "open": false,
                                        })
                                        .to_string(),
                                    )
                                    .await
                                {
                                    warn!(%err, "browserSession close push failed");
                                }
                                Ok(String::new())
                            }
                            _ => {
                                let result =
                                    bridge::dispatch_sync(&method, &args_json, &plugin_id, &pins);
                                // session-mode-hud.md transitions 3–4: pin map
                                // while Hidden/HudPinned (not Interactive).
                                if method == "panel.setPinned"
                                    && result.is_ok()
                                    && matches!(
                                        mode,
                                        SessionMode::Hidden | SessionMode::HudPinned
                                    )
                                {
                                    let next = session_mode::next_mode(
                                        mode,
                                        bridge::any_pinned(&pins) || toast_active(toast_until),
                                    );
                                    if next != mode {
                                        let _ = ctrl_tx.send(HostCtrl::Mode(next));
                                    }
                                }
                                result
                            }
                        };
                        if let Err(ref err) = result {
                            warn!(%method, %err, "bridge invoke rejected");
                        }
                        if let Err(err) = cef.send_bridge_result(request_id, result).await {
                            warn!(%err, "bridge result send failed");
                        }
                    }
                    Some(CefEvent::Exit(code)) => {
                        warn!(?code, "CEF exited");
                        clear_layer(&mut conn, &mut surface, win_id, overlay_layer, &mut stamped).await;
                        return Ok(());
                    }
                    None => {
                        warn!("CEF event channel closed");
                        clear_layer(&mut conn, &mut surface, win_id, overlay_layer, &mut stamped).await;
                        return Ok(());
                    }
                }
            }
            ev = events.recv() => {
                let Some(ev) = ev else {
                    info!("pipe closed");
                    clear_layer(&mut conn, &mut surface, win_id, overlay_layer, &mut stamped).await;
                    let _ = cef.shutdown().await;
                    return Ok(());
                };
                match ev {
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::Resized { width, height },
                    } if id == win_id => {
                        win_w = width;
                        win_h = height;
                        if owns_session {
                            // Shell tracks the game client.
                            browser_w = win_w.max(1);
                            browser_h = win_h.max(1);
                        } else {
                            // Keep inner window inside the new game client.
                            pos_x = pos_x.clamp(0.0, (win_w as f32 - browser_w as f32).max(0.0));
                            pos_y = pos_y.clamp(0.0, (win_h as f32 - browser_h as f32).max(0.0));
                            browser_w = browser_w.min(win_w.max(1));
                            browser_h = browser_h.min(win_h.max(1));
                        }
                        if let Err(err) = cef.set_surface_size(win_w, win_h).await {
                            warn!(%err, "CEF set_surface_size failed");
                        }
                        if !parked {
                            let _ = sync_inner_bounds(
                                &mut conn,
                                &mut cef,
                                win_id,
                                overlay_layer,
                                pos_x,
                                pos_y,
                                browser_w,
                                browser_h,
                                true,
                                &mut next_layout_generation,
                                &mut last_sent_generation,
                                &mut last_bounds_sent,
                            )
                            .await;
                        }
                    }
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::Input(input),
                    } if id == win_id => match input {
                        InputEvent::Cursor(cursor) => {
                            if parked {
                                continue;
                            }
                            if owns_session {
                                // Shell fills the surface — no host-drawn
                                // titlebar drag or edge resize to hit-test.
                                forward_cursor(
                                    &mut cef,
                                    &cursor,
                                    keys.modifier_flags(),
                                    content_blank,
                                    content_hole,
                                    chrome_top_px,
                                    &mut focus_target,
                                    &mut mouse_capture,
                                )
                                .await;
                                continue;
                            }
                            let mut consumed = false;
                            if let Some(force_flush) = apply_host_resize(
                                &cursor,
                                win_w,
                                win_h,
                                &mut pos_x,
                                &mut pos_y,
                                &mut browser_w,
                                &mut browser_h,
                                &mut resize_drag,
                                &mut titlebar_drag,
                            ) {
                                consumed = true;
                                let _ = sync_inner_bounds(
                                    &mut conn,
                                    &mut cef,
                                    win_id,
                                    overlay_layer,
                                    pos_x,
                                    pos_y,
                                    browser_w,
                                    browser_h,
                                    force_flush,
                                    &mut next_layout_generation,
                                    &mut last_sent_generation,
                                    &mut last_bounds_sent,
                                )
                                .await;
                                if force_flush {
                                    last_cursor = Cursor::Default;
                                    let _ = conn
                                        .window(win_id)
                                        .request(SetBlockingCursor {
                                            cursor: Some(Cursor::Default),
                                        })
                                        .await;
                                }
                            } else if let Some((dx, dy, force_flush)) = apply_titlebar_drag(
                                &cursor,
                                browser_w as i32,
                                &mut titlebar_drag,
                            ) {
                                consumed = true;
                                if !force_flush {
                                    pos_x = (pos_x + dx).max(0.0);
                                    pos_y = (pos_y + dy).max(0.0);
                                }
                                let _ = sync_inner_bounds(
                                    &mut conn,
                                    &mut cef,
                                    win_id,
                                    overlay_layer,
                                    pos_x,
                                    pos_y,
                                    browser_w,
                                    browser_h,
                                    force_flush,
                                    &mut next_layout_generation,
                                    &mut last_sent_generation,
                                    &mut last_bounds_sent,
                                )
                                .await;
                            }

                            let hover = resize_drag
                                .as_ref()
                                .map(|d| d.edges)
                                .or_else(|| {
                                    hit_resize_edges(
                                        cursor.client.x,
                                        cursor.client.y,
                                        browser_w as i32,
                                        browser_h as i32,
                                    )
                                })
                                .unwrap_or_default();
                            let want = if hover.any() {
                                hover.cursor()
                            } else {
                                Cursor::Default
                            };
                            if want != last_cursor {
                                last_cursor = want;
                                let _ = conn
                                    .window(win_id)
                                    .request(SetBlockingCursor {
                                        cursor: Some(want),
                                    })
                                    .await;
                            }

                            if !consumed && resize_drag.is_none() {
                                forward_cursor(
                                    &mut cef,
                                    &cursor,
                                    keys.modifier_flags(),
                                    content_blank,
                                    content_hole,
                                    chrome_top_px,
                                    &mut focus_target,
                                    &mut mouse_capture,
                                )
                                .await;
                            }
                        }
                        InputEvent::Keyboard(key) => {
                            if owns_session && hotkey.consume(&key) {
                                let next = if mode == SessionMode::Interactive {
                                    session_mode::next_mode(
                                        mode,
                                        bridge::any_pinned(&pins) || toast_active(toast_until),
                                    )
                                } else {
                                    SessionMode::Interactive
                                };
                                let _ = ctrl_tx.send(HostCtrl::Mode(next));
                                continue;
                            }
                            if parked {
                                continue;
                            }
                            forward_key(&mut cef, &key, &mut keys, focus_target).await;
                        }
                    },
                    // User dropped the input block (Alt+F4, Escape route); our
                    // own unblock echoes here after mode already left Interactive.
                    OverlayEvent::Window {
                        id,
                        event: WindowEvent::InputBlockingEnded,
                    } if id == win_id && owns_session && mode == SessionMode::Interactive => {
                        let _ = ctrl_tx.send(HostCtrl::Mode(SessionMode::Hidden));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Nav invokes carry no result — settle the promise once the op is queued.
fn connection_json(pid: u32) -> String {
    serde_json::json!({
        "type": "connection",
        "connected": true,
        "pid": pid,
    })
    .to_string()
}

/// Plugin overlay/native/browser session methods. `None` = reply already queued
/// on `ctrl_tx` (Mode then BridgeResult) so the select loop stays free.
async fn dispatch_plugin_session(
    method: &str,
    args_json: &str,
    mode: SessionMode,
    hold: bool,
    owns_session: bool,
    pins: &bridge::PinMap,
    win_id: u32,
    request_id: u32,
    ctrl_tx: &mpsc::UnboundedSender<HostCtrl>,
    conn: &mut IpcClientConn,
    cef: &mut CefSession,
    focus_target: &mut CefFocusTarget,
    content_hole: &mut Option<(i32, i32, u32, u32)>,
    content_blank: &mut bool,
) -> Option<Result<String, String>> {
    let mode_changing = matches!(
        method,
        "overlay.open"
            | "overlay.close"
            | "native.window.toggleInteractive"
            | "native.window.setMode"
    );
    if mode_changing && !owns_session {
        return Some(Err(
            "overlay session not owned by CEF host".into(),
        ));
    }

    let queue_mode = |next: SessionMode, result: Result<String, String>| {
        if next != mode {
            let _ = ctrl_tx.send(HostCtrl::Mode(next));
        }
        let _ = ctrl_tx.send(HostCtrl::BridgeResult { request_id, result });
    };

    match method {
        "overlay.isOpen" => Some(Ok(
            if mode == SessionMode::Interactive {
                "true"
            } else {
                "false"
            }
            .into(),
        )),
        "overlay.open" => {
            if mode != SessionMode::Interactive {
                queue_mode(SessionMode::Interactive, Ok(String::new()));
                None
            } else {
                Some(Ok(String::new()))
            }
        }
        "overlay.close" => {
            if mode == SessionMode::Interactive {
                let next = session_mode::next_mode(mode, hold);
                queue_mode(next, Ok(String::new()));
                None
            } else {
                Some(Ok(String::new()))
            }
        }
        "native.window.getSnapshot" => {
            let keys = pins
                .lock()
                .map(|g| g.clone())
                .unwrap_or_default();
            Some(Ok(plugin_ipc::window_snapshot_json(
                mode.as_str(),
                &keys,
                win_id,
            )))
        }
        "native.window.toggleInteractive" => {
            let next = if mode == SessionMode::Interactive {
                session_mode::next_mode(mode, hold)
            } else {
                SessionMode::Interactive
            };
            let body = Ok(serde_json::to_string(next.as_str()).unwrap_or_else(|_| {
                format!("\"{}\"", next.as_str())
            }));
            queue_mode(next, body);
            None
        }
        "native.window.setMode" => {
            let args: Vec<Value> = match serde_json::from_str(args_json) {
                Ok(v) => v,
                Err(err) => return Some(Err(format!("native.window.setMode args: {err}"))),
            };
            let name = match args.first().and_then(|v| v.as_str()) {
                Some(s) => s,
                None => {
                    return Some(Err(
                        "native.window.setMode requires mode string".into(),
                    ));
                }
            };
            let next = match SessionMode::parse(name) {
                Some(m) => m,
                None => {
                    return Some(Err(format!("invalid overlay mode: {name}")));
                }
            };
            let body = Ok(serde_json::to_string(next.as_str()).unwrap_or_else(|_| {
                format!("\"{}\"", next.as_str())
            }));
            queue_mode(next, body);
            None
        }
        "native.overlay.getGameWindowId" => Some(Ok(win_id.to_string())),
        "native.overlay.listenInput" => {
            let args: Vec<Value> = serde_json::from_str(args_json).unwrap_or_default();
            let cursor = args.first().and_then(|v| v.as_bool()).unwrap_or(false);
            let keyboard = args.get(1).and_then(|v| v.as_bool()).unwrap_or(false);
            let listened = conn
                .window(win_id)
                .request(ListenInput { cursor, keyboard })
                .await;
            if !matches!(listened, Ok(true)) {
                warn!(?listened, "DLL rejected ListenInput (plugin)");
            }
            Some(Ok(String::new()))
        }
        "native.overlay.blockInput" => {
            let args: Vec<Value> = serde_json::from_str(args_json).unwrap_or_default();
            let block = args.first().and_then(|v| v.as_bool()).unwrap_or(false);
            let blocked = conn
                .window(win_id)
                .request(BlockInput { block })
                .await;
            if !matches!(blocked, Ok(true)) {
                warn!(?blocked, "DLL rejected BlockInput (plugin)");
            }
            Some(Ok(String::new()))
        }
        "native.overlay.setBlockingCursor" => {
            let args: Vec<Value> = serde_json::from_str(args_json).unwrap_or_default();
            let cursor = parse_blocking_cursor(args.first());
            let set = conn
                .window(win_id)
                .request(SetBlockingCursor { cursor })
                .await;
            if !matches!(set, Ok(true)) {
                warn!(?set, "DLL rejected SetBlockingCursor (plugin)");
            }
            Some(Ok(String::new()))
        }
        "browser.focus" => {
            if *focus_target != CefFocusTarget::Content {
                let _ = cef.set_focus(false, *focus_target).await;
            }
            let _ = cef.set_focus(true, CefFocusTarget::Content).await;
            *focus_target = CefFocusTarget::Content;
            Some(Ok(String::new()))
        }
        "browser.blur" => {
            let _ = cef.set_focus(false, *focus_target).await;
            Some(Ok(String::new()))
        }
        "browser.setContentRect" => {
            let hole = match parse_content_rect(args_json) {
                Ok(h) => h,
                Err(err) => return Some(Err(err)),
            };
            *content_hole = hole;
            if let Err(err) = cef.set_content_rect(hole).await {
                return Some(Err(err.to_string()));
            }
            // Hole clear parks hit-testing (HudPinned ghost). Hole restore must
            // re-enable Content routing — previously navigate did that as a side
            // effect; Shift+Tab must not LoadURL just to flip this flag.
            *content_blank = hole.is_none();
            Some(Ok(String::new()))
        }
        "browser.openSession" => {
            if !owns_session {
                return Some(Err(
                    "browser.openSession requires shell session owner".into(),
                ));
            }
            if mode != SessionMode::Interactive {
                queue_mode(SessionMode::Interactive, Ok(String::new()));
                None
            } else {
                Some(Ok(String::new()))
            }
        }
        "browser.closeSession" => {
            *content_hole = None;
            let _ = cef.set_content_rect(None).await;
            let _ = cef.content_navigate("about:blank".into()).await;
            *content_blank = true;
            if *focus_target != CefFocusTarget::Chrome {
                let _ = cef.set_focus(false, *focus_target).await;
                let _ = cef.set_focus(true, CefFocusTarget::Chrome).await;
                *focus_target = CefFocusTarget::Chrome;
            }
            Some(Ok(String::new()))
        }
        other => Some(Err(format!("unknown plugin method: {other}"))),
    }
}

fn parse_content_rect(args_json: &str) -> Result<Option<(i32, i32, u32, u32)>, String> {
    let args: Vec<Value> = serde_json::from_str(args_json)
        .map_err(|e| format!("browser.setContentRect args: {e}"))?;
    match args.first() {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => {
            let x = map
                .get("x")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "setContentRect.x required".to_string())?
                as i32;
            let y = map
                .get("y")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "setContentRect.y required".to_string())?
                as i32;
            let width = map
                .get("width")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "setContentRect.width required".to_string())?
                .round()
                .max(1.0) as u32;
            let height = map
                .get("height")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "setContentRect.height required".to_string())?
                .round()
                .max(1.0) as u32;
            Ok(Some((x, y, width, height)))
        }
        Some(_) => Err("browser.setContentRect expects null or {x,y,width,height}".into()),
    }
}

fn parse_blocking_cursor(arg: Option<&Value>) -> Option<Cursor> {
    match arg {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() || s.eq_ignore_ascii_case("none") => None,
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .and_then(Cursor::from_u32),
        Some(Value::String(s)) => cursor_from_name(s),
        Some(_) => None,
    }
}

/// Map Electron/napi `Cursor` enum names (case-insensitive) via `Debug` labels.
fn cursor_from_name(name: &str) -> Option<Cursor> {
    (0u32..64).find_map(|d| {
        Cursor::from_u32(d).filter(|c| format!("{c:?}").eq_ignore_ascii_case(name))
    })
}

fn nav_ack(sent: anyhow::Result<()>) -> Result<String, String> {
    sent.map(|()| String::new()).map_err(|err| err.to_string())
}

async fn clear_layer(
    conn: &mut IpcClientConn,
    surface: &mut OverlaySurface,
    win_id: u32,
    layer: u32,
    stamped: &mut bool,
) {
    surface.clear();
    *stamped = false;
    let _ = conn
        .window(win_id)
        .request(UpdateLayerHandle {
            layer,
            handle: None,
        })
        .await;
}

/// Default floating overlay panel size (matches non-shell browser host startup).
fn floating_panel_geom(win_w: u32, win_h: u32) -> (f32, f32, u32, u32) {
    (
        80.0_f32,
        80.0_f32,
        ((win_w as f32) * 0.52).clamp(360.0, 1280.0).round() as u32,
        ((win_h as f32) * 0.62).clamp(260.0, 900.0).round() as u32,
    )
}

/// Drive CEF inner window + overlay hit-test rect. Texture stays at (0,0).
/// During drag, `force_flush=false` coalesces SetInnerBounds to ≤1 / 16ms;
/// overlay input rect always updates. `force_flush=true` always sends.
async fn sync_inner_bounds(
    conn: &mut IpcClientConn,
    cef: &mut CefSession,
    win_id: u32,
    layer: u32,
    pos_x: f32,
    pos_y: f32,
    browser_w: u32,
    browser_h: u32,
    force_flush: bool,
    next_layout_generation: &mut u32,
    last_sent_generation: &mut u32,
    last_bounds_sent: &mut Option<Instant>,
) -> anyhow::Result<()> {
    let now = Instant::now();
    let should_send = force_flush
        || last_bounds_sent
            .map(|t| now.duration_since(t) >= BOUNDS_THROTTLE)
            .unwrap_or(true);

    if should_send {
        let layout_gen = *next_layout_generation;
        *next_layout_generation = next_layout_generation.wrapping_add(1);
        if *next_layout_generation == 0 {
            *next_layout_generation = 1; // 0 = legacy on the wire
        }
        *last_sent_generation = layout_gen;
        *last_bounds_sent = Some(now);
        cef.set_bounds(
            pos_x as i32,
            pos_y as i32,
            browser_w,
            browser_h,
            layout_gen,
        )
        .await?;
    }

    let _ = conn
        .window(win_id)
        .request(SetLayerInputRect {
            layer,
            rect: Some(LayerInputRect {
                x: PercentLength::Length(pos_x),
                y: PercentLength::Length(pos_y),
                width: browser_w,
                height: browser_h,
            }),
        })
        .await?;
    Ok(())
}

async fn stamp_layer(
    conn: &mut IpcClientConn,
    surface: &mut OverlaySurface,
    win_id: u32,
    layer: u32,
    stamped: &mut bool,
    w: u32,
    h: u32,
    handle: u64,
) -> anyhow::Result<()> {
    if handle == 0 {
        anyhow::bail!("invalid CEF paint handle {handle}");
    }
    let stamp_w = w.max(1);
    let stamp_h = h.max(1);

    let Some(update) = surface.update_from_nt_shared(stamp_w, stamp_h, handle, None)? else {
        // Same shared texture updated in place — game already holds the handle.
        return Ok(());
    };

    // Fullscreen texture pinned at origin; hit-test uses SetLayerInputRect.
    // Position sent once on first stamp; position never changes.

    let ok = conn
        .window(win_id)
        .request(UpdateLayerHandle {
            layer,
            handle: update.handle,
        })
        .await?;
    if !ok {
        anyhow::bail!("UpdateLayerHandle layer {layer} returned false");
    }
    if !*stamped {
        *stamped = true;
        // Positioned at origin once — never changes. Avoid sending this IPC on
        // every paint when the surface handle updates.
        conn.window(win_id)
            .request(SetLayerPosition {
                layer,
                x: PercentLength::Length(0.0),
                y: PercentLength::Length(0.0),
            })
            .await?;
        info!(
            stamp_w,
            stamp_h, handle, layer, "layer stamped fullscreen at (0,0)"
        );
    }
    Ok(())
}

/// Hit-test panel edges for host-owned resize (window-space drag, not CEF movementX).
fn hit_resize_edges(cx: i32, cy: i32, bw: i32, bh: i32) -> Option<ResizeEdges> {
    if bw <= 0 || bh <= 0 {
        return None;
    }
    // Don't steal titlebar window buttons.
    if cy < TITLEBAR_H && cx >= bw - TITLEBAR_WIN_BTNS_W {
        return None;
    }
    let edges = ResizeEdges {
        n: cy >= 0 && cy < RESIZE_EDGE_PX,
        s: cy >= bh - RESIZE_EDGE_PX && cy < bh,
        w: cx >= 0 && cx < RESIZE_EDGE_PX,
        e: cx >= bw - RESIZE_EDGE_PX && cx < bw,
    };
    if edges.any() {
        Some(edges)
    } else {
        None
    }
}

/// Returns `Some(force_flush)` while a resize drag is active / ends.
/// `true` = LMB release (final bounds); `false` = position/size updated.
fn apply_host_resize(
    cursor: &glint_overlay_event::input::CursorInput,
    win_w: u32,
    win_h: u32,
    pos_x: &mut f32,
    pos_y: &mut f32,
    browser_w: &mut u32,
    browser_h: &mut u32,
    resize: &mut Option<ResizeDrag>,
    titlebar_drag: &mut Option<(i32, i32)>,
) -> Option<bool> {
    let cx = cursor.client.x;
    let cy = cursor.client.y;
    let wx = cursor.window.x;
    let wy = cursor.window.y;
    match &cursor.event {
        CursorEvent::Leave => {
            // Keep drag alive — grow-resize leaves the texture rect; overlay captures LMB.
            if resize.is_some() {
                return Some(false);
            }
            None
        }
        CursorEvent::Action {
            state: CursorInputState::Pressed { .. },
            action: CursorAction::Left,
        } => {
            if titlebar_drag.is_some() {
                return None;
            }
            // Titlebar move zone (not edges) wins over resize.
            if cy < TITLEBAR_H
                && cx < *browser_w as i32 - TITLEBAR_WIN_BTNS_W
                && cx >= RESIZE_EDGE_PX
                && cy >= RESIZE_EDGE_PX
            {
                return None;
            }
            if let Some(edges) = hit_resize_edges(cx, cy, *browser_w as i32, *browser_h as i32) {
                *titlebar_drag = None;
                *resize = Some(ResizeDrag {
                    edges,
                    last_wx: wx,
                    last_wy: wy,
                });
                // Consumed — do not also arm titlebar_drag on north edge.
                return Some(false);
            }
            None
        }
        CursorEvent::Action {
            state: CursorInputState::Released,
            action: CursorAction::Left,
        } => {
            if resize.take().is_some() {
                *titlebar_drag = None;
                // Force final bounds sync on release.
                return Some(true);
            }
            None
        }
        CursorEvent::Move | CursorEvent::Enter => {
            let Some(drag) = resize.as_mut() else {
                return None;
            };
            let dx = (wx - drag.last_wx) as f32;
            let dy = (wy - drag.last_wy) as f32;
            drag.last_wx = wx;
            drag.last_wy = wy;
            if dx == 0.0 && dy == 0.0 {
                return Some(false);
            }

            let mut x = *pos_x;
            let mut y = *pos_y;
            let mut w = *browser_w as f32;
            let mut h = *browser_h as f32;
            let edges = drag.edges;

            if edges.e {
                w = (w + dx).max(360.0);
            }
            if edges.s {
                h = (h + dy).max(260.0);
            }
            if edges.w {
                let nw = (w - dx).max(360.0);
                x += w - nw;
                w = nw;
            }
            if edges.n {
                let nh = (h - dy).max(260.0);
                y += h - nh;
                h = nh;
            }

            x = x.clamp(0.0, (win_w as f32 - w).max(0.0));
            y = y.clamp(0.0, (win_h as f32 - h).max(0.0));
            w = w.min((win_w as f32 - x).max(360.0));
            h = h.min((win_h as f32 - y).max(260.0));

            *pos_x = x;
            *pos_y = y;
            *browser_w = w.round() as u32;
            *browser_h = h.round() as u32;
            Some(false)
        }
        _ => None,
    }
}

/// Synthesize titlebar window drag like Electron `OverlayInputRouter`.
/// Returns `Some((dx, dy, force_flush))` — on LMB release `force_flush` is true.
fn apply_titlebar_drag(
    cursor: &glint_overlay_event::input::CursorInput,
    browser_w: i32,
    drag: &mut Option<(i32, i32)>,
) -> Option<(f32, f32, bool)> {
    let cx = cursor.client.x;
    let cy = cursor.client.y;
    let wx = cursor.window.x;
    let wy = cursor.window.y;
    match &cursor.event {
        CursorEvent::Leave => {
            if drag.take().is_some() {
                return Some((0.0, 0.0, true)); // force_flush final bounds
            }
            None
        }
        CursorEvent::Action {
            state: CursorInputState::Pressed { .. },
            action: CursorAction::Left,
        } => {
            if cy < TITLEBAR_H && cx < browser_w - TITLEBAR_WIN_BTNS_W {
                *drag = Some((wx, wy));
            }
            None
        }
        CursorEvent::Action {
            state: CursorInputState::Released,
            action: CursorAction::Left,
        } => {
            if drag.take().is_some() {
                return Some((0.0, 0.0, true));
            }
            None
        }
        CursorEvent::Move | CursorEvent::Enter => {
            let Some((last_x, last_y)) = drag.as_mut() else {
                return None;
            };
            let dx = (wx - *last_x) as f32;
            let dy = (wy - *last_y) as f32;
            *last_x = wx;
            *last_y = wy;
            if dx == 0.0 && dy == 0.0 {
                None
            } else {
                Some((dx, dy, false))
            }
        }
        _ => None,
    }
}

/// Per-VK held state, and the source of truth for modifier flags.
///
/// Modifiers are derived from this stream rather than sampled live from the OS:
/// events are captured by a hook in the *game* process and drained here after a
/// pipe hop, so an OS sample describes "now" while the event describes "some
/// time ago". Under load that races — Ctrl can already be released by the time
/// `A` is drained, which is exactly how Ctrl+A goes missing. The stream is
/// ordering-correct; the OS is only used to re-seed.
struct KeyState {
    held: Box<[bool; 256]>,
    /// Last Key transition forwarded to CEF — drops LL+pump duplicates.
    last_fwd_vk: u8,
    last_fwd_down: bool,
    last_fwd_ms: u64,
}

impl Default for KeyState {
    fn default() -> Self {
        Self {
            held: Box::new([false; 256]),
            last_fwd_vk: 0,
            last_fwd_down: false,
            last_fwd_ms: 0,
        }
    }
}

impl KeyState {
    /// Drop all held state and re-seed modifiers from the OS. The event stream
    /// cannot know about keys held before the overlay started receiving input,
    /// and keys released while parked are never observed at all.
    fn resync(&mut self) {
        self.held.fill(false);
        for vk in [VK_SHIFT.0, VK_CONTROL.0, VK_MENU.0] {
            self.held[vk as usize] = key_is_down(vk);
        }
        self.last_fwd_vk = 0;
        self.last_fwd_down = false;
        self.last_fwd_ms = 0;
    }

    /// Record a transition; returns `None` when this is a dual-source duplicate
    /// (LL hook + pump both saw the same keystroke), `Some(repeat)` otherwise.
    fn set(&mut self, vk: u8, down: bool) -> Option<bool> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // LL fires before the pump dequeues the same keystroke; both reach us
        // within a couple of ms. OS auto-repeat is ≥30ms apart, so a tight
        // window drops duplicates without eating holds.
        if vk == self.last_fwd_vk
            && down == self.last_fwd_down
            && now.saturating_sub(self.last_fwd_ms) < 8
        {
            return None;
        }
        let repeat = down && self.held[vk as usize];
        self.held[vk as usize] = down;
        self.last_fwd_vk = vk;
        self.last_fwd_down = down;
        self.last_fwd_ms = now;
        Some(repeat)
    }

    fn ctrl(&self) -> bool {
        self.held[0x11]
    }

    fn alt(&self) -> bool {
        self.held[0x12]
    }

    /// CEF modifier mask at the current position in the event stream.
    fn modifier_flags(&self) -> u32 {
        let mut f = 0u32;
        if self.held[0x10] {
            f |= CEF_EVENTFLAG_SHIFT_DOWN;
        }
        if self.ctrl() {
            f |= CEF_EVENTFLAG_CONTROL_DOWN;
        }
        if self.alt() {
            f |= CEF_EVENTFLAG_ALT_DOWN;
        }
        if key_is_toggled(VK_CAPITAL.0) {
            f |= CEF_EVENTFLAG_CAPS_LOCK_ON;
        }
        if key_is_toggled(VK_NUMLOCK.0) {
            f |= CEF_EVENTFLAG_NUM_LOCK_ON;
        }
        f
    }
}

fn key_is_down(vk: u16) -> bool {
    // GetAsyncKeyState reads physical global state, so the mask stays correct
    // for a modifier that was already held before the overlay took input focus
    // — the case that silently dropped CONTROL_DOWN and broke Ctrl+A.
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

fn key_is_toggled(vk: u16) -> bool {
    (unsafe { GetKeyState(vk as i32) } & 1) != 0
}

// Deliberately not sent: IS_LEFT/IS_RIGHT have no branch in CEF's
// TranslateUiEventModifiers and never reach Blink, IS_KEY_PAD is mistranslated
// to EF_IS_EXTENDED_KEY, and ALTGR_DOWN needs a layout check via VkKeyScanExW
// on CHAR only. All three are derived correctly from the DomCode instead, once
// native_key_code carries a real scan code.

/// CEF's `native_key_code` on Windows is the **bare hardware scan code**,
/// `0xE0`-prefixed for extended keys — not a Win32 lParam and not a virtual-key
/// code. CEF feeds it straight to `KeycodeConverter::NativeKeycodeToDomCode`,
/// which matches by exact equality against a table whose Windows column holds
/// values like `0x001E` (KeyA), `0x000E` (Backspace), `0xE01D` (ControlRight).
/// Anything else yields `DomCode::NONE`, which leaves `KeyboardEvent.code`
/// empty and stops Blink resolving editing commands and shortcuts at all.
fn native_scan_code(vk: u8, extended: bool) -> u32 {
    // Resolve the side-specific VK first: the hook reports the generic
    // VK_SHIFT/VK_CONTROL/VK_MENU, and each side has its own scan code.
    let scan_vk = match (vk, extended) {
        (0x10, false) => VK_LSHIFT.0,
        (0x10, true) => VK_RSHIFT.0,
        (0x11, false) => VK_LCONTROL.0,
        (0x11, true) => VK_RCONTROL.0,
        (0x12, false) => VK_LMENU.0,
        (0x12, true) => VK_RMENU.0,
        _ => vk as u16,
    };
    let scan = unsafe { MapVirtualKeyW(scan_vk as u32, MAPVK_VK_TO_VSC) } & 0xFF;
    // Right Shift is the one side that is *not* flagged extended (its scan code
    // 0x36 carries the side), so the hook cannot distinguish it and it resolves
    // as ShiftLeft. Fixing that needs the real KBDLLHOOKSTRUCT.scanCode.
    if extended { scan | 0xE000 } else { scan }
}

/// Hit-test window coords against the content hole / chrome-top strip.
fn resolve_cursor_target(
    wx: i32,
    wy: i32,
    cx: i32,
    cy: i32,
    content_blank: bool,
    content_hole: Option<(i32, i32, u32, u32)>,
    chrome_top_px: Option<u32>,
) -> (CefFocusTarget, i32, i32) {
    if let Some((hx, hy, hw, hh)) = content_hole {
        if !content_blank
            && wx >= hx
            && wy >= hy
            && wx < hx + hw as i32
            && wy < hy + hh as i32
        {
            return (CefFocusTarget::Content, wx - hx, wy - hy);
        }
        return (CefFocusTarget::Chrome, wx, wy);
    }
    match chrome_top_px {
        Some(top) if !content_blank && cy >= top as i32 => {
            (CefFocusTarget::Content, cx, cy - top as i32)
        }
        _ => (CefFocusTarget::Chrome, wx, wy),
    }
}

/// While a button is held, keep delivering to the press target even if the
/// cursor crosses the content hole (shell window resize/drag).
fn apply_mouse_capture(
    capture: Option<CefFocusTarget>,
    geometric: (CefFocusTarget, i32, i32),
    wx: i32,
    wy: i32,
    content_hole: Option<(i32, i32, u32, u32)>,
) -> (CefFocusTarget, i32, i32) {
    match capture {
        Some(CefFocusTarget::Chrome) => (CefFocusTarget::Chrome, wx, wy),
        Some(CefFocusTarget::Content) => {
            if let Some((hx, hy, _, _)) = content_hole {
                (CefFocusTarget::Content, wx - hx, wy - hy)
            } else {
                geometric
            }
        }
        Some(CefFocusTarget::Unspecified) | None => geometric,
    }
}

async fn forward_cursor(
    cef: &mut CefSession,
    cursor: &glint_overlay_event::input::CursorInput,
    modifiers: u32,
    content_blank: bool,
    content_hole: Option<(i32, i32, u32, u32)>,
    chrome_top_px: Option<u32>,
    focus_target: &mut CefFocusTarget,
    mouse_capture: &mut Option<CefFocusTarget>,
) {
    // client = panel-local (input-rect origin); window = game/surface space.
    let cx = cursor.client.x;
    let cy = cursor.client.y;
    let wx = cursor.window.x;
    let wy = cursor.window.y;
    let geometric = resolve_cursor_target(
        wx,
        wy,
        cx,
        cy,
        content_blank,
        content_hole,
        chrome_top_px,
    );
    let (target, lx, ly) =
        apply_mouse_capture(*mouse_capture, geometric, wx, wy, content_hole);
    if *focus_target != target {
        // Hand the render-widget focus to the newly targeted OSR browser. These
        // are two independent browsers, so the old one must be blurred or both
        // keep a caret and compete for keyboard routing.
        let _ = cef.set_focus(false, *focus_target).await;
        let _ = cef.set_focus(true, target).await;
        *focus_target = target;
    }

    match &cursor.event {
        CursorEvent::Enter | CursorEvent::Move => {
            let _ = cef
                .send_mouse(MouseEvent {
                    r#type: MouseEventType::Move as i32,
                    target: target as i32,
                    modifiers,
                    button: MouseButton::Unspecified as i32,
                    x: lx,
                    y: ly,
                    click_count: 0,
                })
                .await;
        }
        CursorEvent::Leave => {
            let _ = cef
                .send_mouse(MouseEvent {
                    r#type: MouseEventType::Leave as i32,
                    target: target as i32,
                    modifiers,
                    button: MouseButton::Unspecified as i32,
                    x: lx,
                    y: ly,
                    click_count: 0,
                })
                .await;
        }
        CursorEvent::Scroll { axis, delta } => {
            let (dx, dy) = match axis {
                ScrollAxis::X => (*delta as i32, 0),
                ScrollAxis::Y => (0, *delta as i32),
            };
            let _ = cef
                .send_wheel(WheelEvent {
                    target: target as i32,
                    modifiers,
                    x: lx,
                    y: ly,
                    delta_x: dx,
                    delta_y: dy,
                })
                .await;
        }
        CursorEvent::Action { state, action } => {
            let (button, button_flag) = match action {
                CursorAction::Left => (MouseButton::Left, 1u32 << 4),
                CursorAction::Middle => (MouseButton::Middle, 1u32 << 5),
                CursorAction::Right => (MouseButton::Right, 1u32 << 6),
                _ => return,
            };
            let (ty, click_count) = match state {
                CursorInputState::Pressed { double_click } => {
                    // LMB capture: shell resize/drag must keep Chrome events when
                    // the cursor crosses the content hole.
                    if matches!(action, CursorAction::Left) {
                        *mouse_capture = Some(target);
                    }
                    (MouseEventType::Down, 1 + u32::from(*double_click))
                }
                CursorInputState::Released => {
                    if matches!(action, CursorAction::Left) {
                        *mouse_capture = None;
                    }
                    (MouseEventType::Up, 1u32)
                }
            };
            let _ = cef
                .send_mouse(MouseEvent {
                    r#type: ty as i32,
                    target: target as i32,
                    modifiers: modifiers | button_flag,
                    button: button as i32,
                    x: lx,
                    y: ly,
                    click_count,
                })
                .await;
        }
    }
}

async fn forward_key(
    cef: &mut CefSession,
    key: &KeyboardInput,
    keys: &mut KeyState,
    target: CefFocusTarget,
) {
    match key {
        KeyboardInput::Key { key, state } => {
            let vk = key.code.get();
            let extended = key.extended;
            let down = matches!(state, KeyInputState::Pressed);
            // Record before the hotkey guard so Tab's held state cannot go stale.
            let Some(repeat) = keys.set(vk, down) else {
                // Dual-source duplicate (LL + pump) — already forwarded.
                return;
            };
            // Overlay hotkey — never forward Tab-with-Shift to CEF.
            if vk == 0x09 && keys.held[0x10] {
                return;
            }

            let mut modifiers = keys.modifier_flags();
            if repeat {
                modifiers |= CEF_EVENTFLAG_IS_REPEAT;
            }
            // WM_SYSKEY* equivalent: Alt held without Ctrl, since Windows
            // suppresses WM_SYSKEY* when both are down to accommodate AltGr.
            let is_system_key = keys.alt() && !keys.ctrl();

            // One CEF event per physical transition, mirroring cefclient's OSR
            // handler: WM_KEYDOWN -> RAWKEYDOWN, WM_KEYUP -> KEYUP. Also
            // sending KEYDOWN produced a second DOM keydown for every press.
            let ty = if down {
                KeyEventType::RawDown
            } else {
                KeyEventType::KeyUp
            };
            let native = native_scan_code(vk, extended);
            info!(
                vk = format_args!("0x{vk:02X}"),
                down,
                extended,
                repeat,
                modifiers = format_args!("0x{modifiers:04X}"),
                native = format_args!("0x{native:04X}"),
                is_system_key,
                target = target as i32,
                "KEYDIAG key->cef"
            );
            let _ = cef
                .send_key(KeyEvent {
                    r#type: ty as i32,
                    target: target as i32,
                    modifiers,
                    windows_key_code: vk as u32,
                    native_key_code: native,
                    is_system_key,
                    character: String::new(),
                })
                .await;
        }
        KeyboardInput::Char(ch) => {
            let code = *ch as u32;
            // TranslateMessage turns Ctrl+letter into WM_CHAR 0x01..0x1A. When
            // Key events never arrive (starved LL + old pump skip), held[] has
            // no CONTROL_DOWN — sample the OS as a last resort and synthesize
            // the accelerator key so Ctrl+A still selects-all in CEF.
            if (0x01..=0x1A).contains(&code) && code != 0x08 && code != 0x09 && code != 0x0A && code != 0x0D
            {
                let ctrl = keys.ctrl() || key_is_down(VK_CONTROL.0);
                if ctrl {
                    let vk = (code as u8) + 0x40; // 0x01 → 'A'
                    keys.held[0x11] = true;
                    let modifiers = keys.modifier_flags();
                    info!(
                        vk = format_args!("0x{vk:02X}"),
                        ch = format_args!("U+{code:04X}"),
                        modifiers = format_args!("0x{modifiers:04X}"),
                        target = target as i32,
                        "KEYDIAG ctrl-char->key"
                    );
                    let native = native_scan_code(vk, false);
                    let _ = cef
                        .send_key(KeyEvent {
                            r#type: KeyEventType::RawDown as i32,
                            target: target as i32,
                            modifiers,
                            windows_key_code: vk as u32,
                            native_key_code: native,
                            is_system_key: false,
                            character: String::new(),
                        })
                        .await;
                    return;
                }
            }

            // Backspace arrives as WM_CHAR 0x08 when only the char path works.
            // Prefer a real key event so contenteditable / input handle it.
            if code == 0x08 && !keys.ctrl() && !key_is_down(VK_CONTROL.0) {
                let modifiers = keys.modifier_flags();
                info!(
                    modifiers = format_args!("0x{modifiers:04X}"),
                    target = target as i32,
                    "KEYDIAG backspace-char->key"
                );
                let native = native_scan_code(0x08, false);
                let _ = cef
                    .send_key(KeyEvent {
                        r#type: KeyEventType::RawDown as i32,
                        target: target as i32,
                        modifiers,
                        windows_key_code: 0x08,
                        native_key_code: native,
                        is_system_key: false,
                        character: String::new(),
                    })
                    .await;
                return;
            }

            let modifiers = keys.modifier_flags();
            let (ctrl, alt) = (
                keys.ctrl() || key_is_down(VK_CONTROL.0),
                keys.alt() || key_is_down(VK_MENU.0),
            );
            // TranslateMessage turns Ctrl+A into WM_CHAR 0x01. Forwarding that
            // inserts a control code into the focused field and competes with
            // the accelerator, so shortcuts travel on the key events alone.
            // AltGr arrives as Ctrl+Alt and *does* produce printable text on
            // European layouts, so exempt that combination.
            if (ctrl || alt) && !(ctrl && alt) {
                info!(
                    ch = format_args!("U+{:04X}", *ch as u32),
                    ctrl, alt, "KEYDIAG char suppressed (shortcut)"
                );
                return;
            }
            info!(
                ch = format_args!("U+{:04X}", *ch as u32),
                modifiers = format_args!("0x{modifiers:04X}"),
                target = target as i32,
                "KEYDIAG char->cef"
            );
            let _ = cef
                .send_key(KeyEvent {
                    r#type: KeyEventType::Char as i32,
                    target: target as i32,
                    modifiers,
                    windows_key_code: *ch as u32,
                    native_key_code: 0,
                    is_system_key: false,
                    character: ch.to_string(),
                })
                .await;
        }
        KeyboardInput::Ime(_) => {}
    }
}

#[cfg(test)]
mod parse_blocking_cursor_tests {
    use super::{cursor_from_name, parse_blocking_cursor};
    use glint_overlay_common::cursor::Cursor;
    use serde_json::json;

    #[test]
    fn null_empty_none_clear() {
        assert_eq!(parse_blocking_cursor(None), None);
        assert_eq!(parse_blocking_cursor(Some(&json!(null))), None);
        assert_eq!(parse_blocking_cursor(Some(&json!(""))), None);
        assert_eq!(parse_blocking_cursor(Some(&json!("none"))), None);
        assert_eq!(parse_blocking_cursor(Some(&json!("None"))), None);
    }

    #[test]
    fn numeric_discriminants() {
        assert_eq!(
            parse_blocking_cursor(Some(&json!(0))),
            Some(Cursor::Default)
        );
        assert_eq!(
            parse_blocking_cursor(Some(&json!(2))),
            Some(Cursor::Pointer)
        );
        assert_eq!(
            parse_blocking_cursor(Some(&json!(13))),
            Some(Cursor::Grab)
        );
        assert_eq!(parse_blocking_cursor(Some(&json!(999))), None);
    }

    #[test]
    fn string_enum_names() {
        assert_eq!(
            parse_blocking_cursor(Some(&json!("Pointer"))),
            Some(Cursor::Pointer)
        );
        assert_eq!(
            parse_blocking_cursor(Some(&json!("pointer"))),
            Some(Cursor::Pointer)
        );
        assert_eq!(
            parse_blocking_cursor(Some(&json!("NotAllowed"))),
            Some(Cursor::NotAllowed)
        );
        assert_eq!(
            parse_blocking_cursor(Some(&json!("EastWestResize"))),
            Some(Cursor::EastWestResize)
        );
        assert_eq!(cursor_from_name("PanWest"), Some(Cursor::PanWest));
        assert_eq!(parse_blocking_cursor(Some(&json!("nope"))), None);
    }
}

#[cfg(test)]
mod mouse_capture_tests {
    use super::{apply_mouse_capture, resolve_cursor_target, CefFocusTarget};

    #[test]
    fn hole_hit_without_capture_goes_to_content() {
        let hole = Some((100, 100, 400, 300));
        let (t, lx, ly) = resolve_cursor_target(150, 200, 150, 200, false, hole, None);
        assert_eq!(t, CefFocusTarget::Content);
        assert_eq!((lx, ly), (50, 100));
    }

    #[test]
    fn blank_flag_blocks_hole_even_when_rect_present() {
        // Shift+Tab hide sets content_blank via setContentRect(null); if restore
        // only brings the rect back and leaves blank=true, clicks miss Content.
        let hole = Some((100, 100, 400, 300));
        let (t, lx, ly) = resolve_cursor_target(150, 200, 150, 200, true, hole, None);
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!((lx, ly), (150, 200));
    }

    #[test]
    fn chrome_capture_keeps_chrome_over_hole() {
        let hole = Some((100, 100, 400, 300));
        let geometric = resolve_cursor_target(150, 200, 150, 200, false, hole, None);
        assert_eq!(geometric.0, CefFocusTarget::Content);
        let (t, lx, ly) =
            apply_mouse_capture(Some(CefFocusTarget::Chrome), geometric, 150, 200, hole);
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!((lx, ly), (150, 200));
    }
}
