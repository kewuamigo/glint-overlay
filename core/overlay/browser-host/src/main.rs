//! Standalone browser overlay host. Owns CEF; stamps paint onto overlay layer ≥1
//! (shell session = layer 1; opaque Browser dock helper = layer 2).

#![windows_subsystem = "windows"]

mod apps;
mod bridge;
mod browser_extensions;
mod cef_child;
mod etw_reader;
mod hw_monitor;
mod metrics_prefs;
mod plugin_achievements;
mod plugin_db;
mod plugin_fs;
mod plugin_ipc;
mod plugin_saves;
mod plugin_storage;
mod session_mode;

use std::time::{Duration, Instant};

use anyhow::Context;
use cef_child::{CefEvent, CefFocusTarget, CefSession, CefShare, resolve_session_document};
use glint_cef_protocol::{
    KeyEvent, KeyEventType, MouseButton, MouseEvent, MouseEventType, WheelEvent,
};
use glint_gpu_texture::{GpuLuid as DxgiLuid, find_dxgi_adapter};
use glint_overlay_client::{
    IpcClientConn, connect_pipe,
    surface::OverlaySurface,
    ty::{CopyRect, Rect},
};
use glint_overlay_common::{
    cursor::Cursor,
    paint_cmd::PaintCmd,
    request::{
        BlockInput, HotKeyAndVisibility, HotkeyChord, LayerInputRect, ListenInput,
        SetBlockingCursor, SetLayerInputRect, SetLayerPosition, UpdateLayerHandle,
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
/// Coalesce SetInnerBounds during resize. Dest-only move never set_bounds.
const BOUNDS_THROTTLE: Duration = Duration::from_millis(16);
/// Ignore repeated Shift+Tab within this window (auto-repeat / duplicate source).
const HOTKEY_DEBOUNCE: Duration = Duration::from_millis(400);
/// Visibility + hotkey heartbeat: every 2 s, or immediately on change.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
/// Shift bit on [`HotkeyChord::modifiers`].
const HOTKEY_MOD_SHIFT: u16 = 0x0001;
/// Configured overlay toggle (Shift+Tab). No pad chord — XInput has none.
const OVERLAY_HOTKEY: HotkeyChord = HotkeyChord {
    vk: 0x09,
    modifiers: HOTKEY_MOD_SHIFT,
};
/// Metrics bridge push while Interactive / HudPinned (FR-004, ≤1 Hz).
const METRICS_PUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Electron `flashToastHud` default for achievement unlocks (~Xbox toast length).
const TOAST_HUD_FLASH: Duration = Duration::from_secs(12);

/// Electron `toastActive()` — hold HudPinned while the flash window is open.
fn toast_active(toast_until: Option<Instant>) -> bool {
    toast_until.is_some_and(|u| Instant::now() < u)
}

/// Publish when visibility/hotkey changed, or at least [`HEARTBEAT_INTERVAL`] passed.
fn heartbeat_should_publish(
    last: Option<(Instant, bool, HotkeyChord)>,
    now: Instant,
    visible: bool,
    hotkey: HotkeyChord,
) -> bool {
    match last {
        None => true,
        Some((t, last_vis, last_hk)) => {
            last_vis != visible || last_hk != hotkey || now.duration_since(t) >= HEARTBEAT_INTERVAL
        }
    }
}

async fn publish_visibility_heartbeat(
    conn: &mut IpcClientConn,
    win_id: u32,
    visible: bool,
    last: &mut Option<(Instant, bool, HotkeyChord)>,
) {
    let now = Instant::now();
    if !heartbeat_should_publish(*last, now, visible, OVERLAY_HOTKEY) {
        return;
    }
    let _ = conn
        .window(win_id)
        .request(HotKeyAndVisibility {
            visible,
            hotkey: OVERLAY_HOTKEY,
        })
        .await;
    *last = Some((now, visible, OVERLAY_HOTKEY));
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
    tab_down: bool,
    last_toggle: Option<Instant>,
}

impl Hotkey {
    /// True when this event is the overlay toggle (and must not reach CEF).
    fn consume(&mut self, input: &KeyboardInput) -> bool {
        let KeyboardInput::Key { key, state } = input else {
            return false;
        };
        let down = matches!(state, KeyInputState::Pressed);
        // VK_SHIFT, VK_LSHIFT, VK_RSHIFT — extended VK_SHIFT is still 0x10.
        if matches!(key.code.get(), 0x10 | 0xA0 | 0xA1) {
            self.shift_down = down;
            return false;
        }
        if key.code.get() != 0x09 {
            return false;
        }
        if !down {
            self.tab_down = false;
            return false;
        }
        // WM_KEYDOWN repeats and LL+pump duplicates are extra Pressed events
        // without a Release. A second toggle would close Interactive immediately
        // (FPS pin → HudPinned): cursor flash, overlay blink, HUD remains.
        if self.tab_down || !self.shift_down {
            return false;
        }
        self.tab_down = true;
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
    // Launcher sidecar (`%APPDATA%/Glint/session.json`), read at startup and
    // retried on the metrics pump until a pid match lands: feeds the optional
    // gameName/playtimeSeconds on connection pushes. Missing/stale → None.
    let attach_epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64);
    let mut session_info = load_session_snapshot(game_pid);
    if let Some((name, seconds)) = &session_info {
        info!(name, playtime_seconds = seconds, "session snapshot matched");
    }
    hw_monitor::start();
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
            event:
                WindowEvent::Added {
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
    hw_monitor::set_adapter_luid(gpu_id.low, gpu_id.high);

    let document = resolve_session_document(
        std::env::var("GLINT_UI_URL")
            .ok()
            .filter(|u| !u.trim().is_empty()),
    )?;
    // Shell document — this helper owns the session (no Electron overlay host).
    let owns_session = true;
    // Must match glint-cef topology flags. Product default = dual OSR
    // (shell + content). Shell-only iframe via GLINT_CEF_SHELL_ONLY=1.
    let osr_topology = osr_topology_from_env();
    let overlay_layer: u32 = std::env::var("GLINT_LAYER")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    info!(overlay_layer, owns_session, ?osr_topology, "overlay layer");
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
    let mut content_surface: OverlaySurface = OverlaySurface::new(adapter.as_ref())?;
    let mut promo_surface: OverlaySurface = OverlaySurface::new(adapter.as_ref())?;

    // The shell owns the whole game client; the browser document keeps the
    // floating panel geometry (and with it the host-drawn titlebar/resize).
    let (mut pos_x, mut pos_y, mut browser_w, mut browser_h) = if owns_session {
        (0.0_f32, 0.0_f32, win_w.max(1), win_h.max(1))
    } else {
        floating_panel_geom(win_w, win_h)
    };
    let mut stamped = false;
    let mut content_stamped = false;
    let mut content_stamp_layer = overlay_layer.max(1);
    let mut overlay_last_pos: Option<(f32, f32)> = None;
    let mut content_last_pos: Option<(f32, f32)> = None;
    let mut content_blank = true;
    // Shell `browser.openSession` / `closeSession` — CEF dual OSR stays alive.
    let mut browser_session_open = false;
    let mut focus_target = CefFocusTarget::Chrome;
    let mut mode = SessionMode::Hidden;
    let mut hotkey = Hotkey::default();
    // Shared with bridge `panel.setPinned`; T019/T020 read via `bridge::any_pinned`.
    let pins = bridge::new_pin_map();
    // A helper-owned session starts hidden and is shown by the hotkey.
    let mut parked = owns_session;
    // Shell React BrowserPanel hole — window-space rect for content OSR + hit-test.
    let mut content_hole: Option<(i32, i32, u32, u32)> = None;
    let mut promo: Option<PromoDrag> = None;
    let mut promo_stamped = false;
    let mut last_chrome_share: Option<(u64, u32, u32)> = None;
    // Sticky target while a button is held (shell resize/drag must not lose
    // events when the cursor slips into the content hole).
    let mut mouse_capture: Option<CefFocusTarget> = None;
    // React AppWindow move/resize — force chrome routing over content hole.
    let mut shell_drag = false;
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
        false,
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
        if let Err(err) = cef
            .send_bridge_push(connection_json(pid, session_info.as_ref(), attach_epoch_ms))
            .await
        {
            warn!(%err, "connection push failed");
        }
    }

    let mut keys = KeyState::default();
    // Seed from the OS: a modifier held before the overlay opened is otherwise
    // invisible to the event stream.
    keys.resync();

    let mut metrics_tick = tokio::time::interval(METRICS_PUSH_INTERVAL);
    metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat_tick = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_heartbeat: Option<(Instant, bool, HotkeyChord)> = None;

    loop {
        tokio::select! {
            _ = heartbeat_tick.tick() => {
                if owns_session {
                    publish_visibility_heartbeat(
                        &mut conn,
                        win_id,
                        mode == SessionMode::Interactive,
                        &mut last_heartbeat,
                    )
                    .await;
                }
            }
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
                    if session_info.is_none() {
                        session_info = load_session_snapshot(game_pid);
                    }
                    let _ = cef
                        .send_bridge_push(connection_json(pid, session_info.as_ref(), attach_epoch_ms))
                        .await;
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
                        last_chrome_share = None;
                        info!(overlay_layer, "park — clear layer, keep CEF alive");
                        clear_both_layers(
                            &mut conn,
                            &mut surface,
                            &mut content_surface,
                            &mut promo_surface,
                            win_id,
                            overlay_layer,
                            content_stamp_layer,
                            &mut stamped,
                            &mut content_stamped,
                            &mut promo,
                            &mut promo_stamped,
                            &mut overlay_last_pos,
                            &mut content_last_pos,
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
                            false,
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
                        publish_visibility_heartbeat(
                            &mut conn,
                            win_id,
                            interactive,
                            &mut last_heartbeat,
                        )
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
                        layer,
                    }) => {
                        if parked {
                            continue;
                        }
                        if layout_generation != last_sent_generation {
                            continue;
                        }
                        match layer {
                            None => {
                                // Legacy composite (no Paint.layer): one fullscreen handle.
                                if let Err(err) = stamp_layer(
                                    &mut conn,
                                    &mut surface,
                                    win_id,
                                    overlay_layer,
                                    &mut stamped,
                                    w,
                                    h,
                                    handle,
                                    Some((pos_x, pos_y)),
                                    &mut overlay_last_pos,
                                )
                                .await
                                {
                                    warn!(%err, "stamp failed");
                                    clear_layer(
                                        &mut conn,
                                        &mut surface,
                                        win_id,
                                        overlay_layer,
                                        &mut stamped,
                                    )
                                    .await;
                                    overlay_last_pos = None;
                                }
                            }
                            Some(0) => {
                                // ContentOnly spike: no shell CEF atlas — ignore layer 0.
                                if !expects_shell_cef_layer0(osr_topology) {
                                    continue;
                                }
                                let (chrome_layer, dest) = chrome_dest_publish(
                                    owns_session,
                                    overlay_layer,
                                    pos_x,
                                    pos_y,
                                );
                                if let Err(err) = stamp_layer(
                                    &mut conn,
                                    &mut surface,
                                    win_id,
                                    chrome_layer,
                                    &mut stamped,
                                    w,
                                    h,
                                    handle,
                                    dest,
                                    &mut overlay_last_pos,
                                )
                                .await
                                {
                                    warn!(%err, "stamp failed");
                                    last_chrome_share = None;
                                    clear_layer(
                                        &mut conn,
                                        &mut surface,
                                        win_id,
                                        chrome_layer,
                                        &mut stamped,
                                    )
                                    .await;
                                    overlay_last_pos = None;
                                } else {
                                    last_chrome_share = Some((handle, w, h));
                                }
                            }
                            // content_blank: skip new content paints but keep
                            // last-good stamp (nav / CEF blank gate). Clear only
                            // on close, hole drop, or intentional about:blank.
                            Some(_layer) if content_blank => {}
                            Some(layer) => {
                                // ShellOnly: page is in-shell iframe — no second OSR layer.
                                if !expects_content_cef_layer(osr_topology) {
                                    continue;
                                }
                                content_stamp_layer = content_surface_layer(layer);
                                let draw_pos = promo
                                    .as_ref()
                                    .and_then(|p| p.hole_pos)
                                    .unwrap_or_else(|| {
                                        content_draw_pos(
                                            w,
                                            h,
                                            browser_w,
                                            browser_h,
                                            content_hole,
                                            chrome_top_px,
                                        )
                                    });
                                if let Err(err) = stamp_layer(
                                    &mut conn,
                                    &mut content_surface,
                                    win_id,
                                    layer,
                                    &mut content_stamped,
                                    w,
                                    h,
                                    handle,
                                    Some(draw_pos),
                                    &mut content_last_pos,
                                )
                                .await
                                {
                                    warn!(%err, "stamp failed");
                                    clear_layer(
                                        &mut conn,
                                        &mut content_surface,
                                        win_id,
                                        layer,
                                        &mut content_stamped,
                                    )
                                    .await;
                                    content_last_pos = None;
                                }
                            }
                        }
                    }
                    Some(CefEvent::PaintError { msg }) => {
                        warn!(%msg, "CEF paintError");
                        last_chrome_share = None;
                        clear_both_layers(
                            &mut conn,
                            &mut surface,
                            &mut content_surface,
                            &mut promo_surface,
                            win_id,
                            overlay_layer,
                            content_stamp_layer,
                            &mut stamped,
                            &mut content_stamped,
                            &mut promo,
                            &mut promo_stamped,
                            &mut overlay_last_pos,
                            &mut content_last_pos,
                        )
                        .await;
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
                            false,
                            &mut next_layout_generation,
                            &mut last_sent_generation,
                            &mut last_bounds_sent,
                        )
                        .await;
                    }
                    Some(CefEvent::Host(action)) => {
                        if action.action == "close" {
                            info!("CEF host close");
                            clear_both_layers(
                                &mut conn,
                                &mut surface,
                                &mut content_surface,
                                &mut promo_surface,
                                win_id,
                                overlay_layer,
                                content_stamp_layer,
                                &mut stamped,
                                &mut content_stamped,
                                &mut promo,
                                &mut promo_stamped,
                                &mut overlay_last_pos,
                                &mut content_last_pos,
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
                                false,
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
                        apply_nav_state_blank(
                            browser_session_open,
                            content_hole,
                            &url,
                            &mut content_blank,
                        );
                        debug!(%url, content_blank, "cef navState");
                        // Do not clear content stamp on NavState — keep last-good
                        // until a new paint replaces it (or close / hole / about:blank).
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
                                        &mut browser_session_open,
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
                                    if method == "browser.setContentRect" {
                                        sync_content_hole_stamp(
                                            &mut conn,
                                            &mut content_surface,
                                            win_id,
                                            content_stamp_layer,
                                            content_hole,
                                            content_blank,
                                            &mut content_stamped,
                                            &mut content_last_pos,
                                        )
                                        .await;
                                    } else if content_blank && content_stamped {
                                        // closeSession (plugin path) etc.
                                        clear_layer(
                                            &mut conn,
                                            &mut content_surface,
                                            win_id,
                                            content_stamp_layer,
                                            &mut content_stamped,
                                        )
                                        .await;
                                        content_last_pos = None;
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
                            // Browser nav — bridge → CEF content frame only
                            // (`contracts/host-bridge.md` rule 4). Never chrome Navigate.
                            "browser.navigate" if plugin_id.is_empty() => {
                                match serde_json::from_str::<(String,)>(&args_json) {
                                    Ok((url,)) => {
                                        // Intentional about:blank (new tab) still blanks
                                        // here; apply_nav_state_blank only filters stale
                                        // NavState while a live hole is published.
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
                            // Chrome-style extension satellites (not content_ OSR).
                            // Fail closed: send errors / CEF reject must not clear_both_layers.
                            "browser.extensions.openOptions" if plugin_id.is_empty() => {
                                match browser_extensions::parse_open_satellite_id(
                                    "browser.extensions.openOptions",
                                    &args_json,
                                ) {
                                    Ok(id) => nav_ack(
                                        cef.open_extension_satellite(
                                            id,
                                            cef_child::ExtensionSatelliteKind::Options,
                                        )
                                        .await,
                                    ),
                                    Err(err) => Err(err),
                                }
                            }
                            "browser.extensions.openPopup" if plugin_id.is_empty() => {
                                match browser_extensions::parse_open_satellite_id(
                                    "browser.extensions.openPopup",
                                    &args_json,
                                ) {
                                    Ok(id) => nav_ack(
                                        cef.open_extension_satellite(
                                            id,
                                            cef_child::ExtensionSatelliteKind::Popup,
                                        )
                                        .await,
                                    ),
                                    Err(err) => Err(err),
                                }
                            }
                            "browser.extensions.closeSatellite" if plugin_id.is_empty() => {
                                match browser_extensions::parse_close_satellite(&args_json) {
                                    Ok(()) => nav_ack(cef.close_extension_satellite().await),
                                    Err(err) => Err(err),
                                }
                            }
                            // Shell React Browser AppWindow lifecycle (content hole).
                            "browser.openSession" if plugin_id.is_empty() => {
                                if !owns_session {
                                    Err("browser.openSession requires shell session owner".into())
                                } else {
                                    let _ = browser_session_mark_open(&mut browser_session_open);
                                    if mode != SessionMode::Interactive {
                                        let _ =
                                            ctrl_tx.send(HostCtrl::Mode(SessionMode::Interactive));
                                    }
                                    // Always push (idempotent): chrome re-publishes the
                                    // content hole after close / minimize restore.
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
                            "native.overlay.setShellDrag" if plugin_id.is_empty() => {
                                let args: Vec<Value> =
                                    serde_json::from_str(&args_json).unwrap_or_default();
                                shell_drag =
                                    args.first().and_then(|v| v.as_bool()).unwrap_or(false);
                                if shell_drag {
                                    mouse_capture = Some(CefFocusTarget::Chrome);
                                }
                                Ok(String::new())
                            }
                            "native.overlay.setPosition" if plugin_id.is_empty() => {
                                match parse_set_position(&args_json) {
                                    Ok(args) => {
                                        let op = position_op(
                                            owns_session,
                                            overlay_layer,
                                            promo.as_ref().map(|p| (p.layer, p.size.0, p.size.1)),
                                            args,
                                            (browser_w, browser_h),
                                        );
                                        match apply_position_op(
                                            op,
                                            &mut conn,
                                            &mut cef,
                                            &mut surface,
                                            &mut promo_surface,
                                            win_id,
                                            overlay_layer,
                                            content_stamp_layer,
                                            content_hole,
                                            last_chrome_share,
                                            &mut promo,
                                            &mut promo_stamped,
                                            &mut pos_x,
                                            &mut pos_y,
                                            &mut browser_w,
                                            &mut browser_h,
                                            &mut next_layout_generation,
                                            &mut last_sent_generation,
                                            &mut last_bounds_sent,
                                        )
                                        .await
                                        {
                                            Ok(()) => Ok(String::new()),
                                            Err(err) => Err(err.to_string()),
                                        }
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            "browser.closeSession" if plugin_id.is_empty() => {
                                if !browser_session_mark_close(
                                    &mut browser_session_open,
                                    &mut content_hole,
                                    &mut content_blank,
                                ) {
                                    // Already closed — do not blank chrome or restart CEF.
                                    Ok(String::new())
                                } else {
                                    mouse_capture = None;
                                    let _ = cef.set_content_rect(None).await;
                                    let _ = cef.content_navigate("about:blank".into()).await;
                                    if focus_target != CefFocusTarget::Chrome {
                                        let _ = cef.set_focus(false, focus_target).await;
                                        let _ = cef.set_focus(true, CefFocusTarget::Chrome).await;
                                        focus_target = CefFocusTarget::Chrome;
                                    }
                                    if content_stamped {
                                        clear_layer(
                                            &mut conn,
                                            &mut content_surface,
                                            win_id,
                                            content_stamp_layer,
                                            &mut content_stamped,
                                        )
                                        .await;
                                        content_last_pos = None;
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
                        // Intentional about:blank navigate only — never clear on
                        // reload/back/focus or transient NavState blank flags.
                        if method == "browser.navigate" && content_blank && content_stamped {
                            clear_layer(
                                &mut conn,
                                &mut content_surface,
                                win_id,
                                content_stamp_layer,
                                &mut content_stamped,
                            )
                            .await;
                            content_last_pos = None;
                        }
                    }
                    Some(CefEvent::Exit(code)) => {
                        warn!(?code, "CEF exited");
                        clear_both_layers(
                            &mut conn,
                            &mut surface,
                            &mut content_surface,
                            &mut promo_surface,
                            win_id,
                            overlay_layer,
                            content_stamp_layer,
                            &mut stamped,
                            &mut content_stamped,
                            &mut promo,
                            &mut promo_stamped,
                            &mut overlay_last_pos,
                            &mut content_last_pos,
                        )
                        .await;
                        return Ok(());
                    }
                    None => {
                        warn!("CEF event channel closed");
                        clear_both_layers(
                            &mut conn,
                            &mut surface,
                            &mut content_surface,
                            &mut promo_surface,
                            win_id,
                            overlay_layer,
                            content_stamp_layer,
                            &mut stamped,
                            &mut content_stamped,
                            &mut promo,
                            &mut promo_stamped,
                            &mut overlay_last_pos,
                            &mut content_last_pos,
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
            ev = events.recv() => {
                let Some(ev) = ev else {
                    info!("pipe closed");
                    clear_both_layers(
                        &mut conn,
                        &mut surface,
                        &mut content_surface,
                        &mut promo_surface,
                        win_id,
                        overlay_layer,
                        content_stamp_layer,
                        &mut stamped,
                        &mut content_stamped,
                        &mut promo,
                        &mut promo_stamped,
                        &mut overlay_last_pos,
                        &mut content_last_pos,
                    )
                    .await;
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
                                false,
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
                                    osr_topology,
                                    shell_drag,
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
                                    false,
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
                                    overlay_layer.max(1),
                                    pos_x,
                                    pos_y,
                                    browser_w,
                                    browser_h,
                                    force_flush,
                                    true,
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
                                    osr_topology,
                                    shell_drag,
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
fn connection_json(
    pid: u32,
    session_info: Option<&(String, i64)>,
    attached_epoch_ms: Option<i64>,
) -> String {
    let mut payload = serde_json::json!({
        "type": "connection",
        "connected": true,
        "pid": pid,
    });
    if let Some((name, seconds)) = session_info {
        payload["gameName"] = serde_json::json!(name);
        payload["playtimeSeconds"] = serde_json::json!(seconds);
    }
    if let Some(ms) = attached_epoch_ms {
        payload["attachedAtMs"] = serde_json::json!(ms);
    }
    payload.to_string()
}

/// This game's entry in the launcher-written playtime sidecar
/// (`%APPDATA%/Glint/session.json`). Any failure → None; chips hide.
fn load_session_snapshot(game_pid: Option<u32>) -> Option<(String, i64)> {
    let pid = game_pid?;
    let appdata = std::env::var_os("APPDATA")?;
    let path = std::path::PathBuf::from(appdata)
        .join(glint_overlay_common::product::APP_DATA_DIR_NAME)
        .join("session.json");
    let raw = std::fs::read_to_string(path).ok()?;
    parse_session_snapshot(&raw, pid)
}

fn parse_session_snapshot(raw: &str, pid: u32) -> Option<(String, i64)> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let games = value
        .get("games")?
        .as_array()?
        .iter()
        .find(|g| g.get("pid").and_then(Value::as_u64) == Some(u64::from(pid)))?;
    Some((
        games.get("name")?.as_str()?.to_string(),
        games.get("totalSeconds")?.as_i64()?,
    ))
}

/// CEF OSR topology for overlay paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OsrTopology {
    /// Opt-in (`GLINT_CEF_SHELL_ONLY`): one shell CreateBrowser + iframe page.
    /// Present stamps layer 0 only. Real sites often refuse framing (XFO).
    ShellOnly,
    /// Product default: shell layer 0 + content ≥1 (Steam-like: React chrome +
    /// one content CEF surface for page pixels).
    ShellHostAndContent,
    /// Legacy spike (`GLINT_CEF_SINGLE_CONTENT_OSR`): content only — blanks shell.
    ContentOnly,
}

fn osr_topology_from_env() -> OsrTopology {
    match std::env::var("GLINT_CEF_SHELL_ONLY") {
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("y") => {
            return OsrTopology::ShellOnly;
        }
        _ => {}
    }
    // Legacy alias: GLINT_CEF_DUAL_OSR=1 was the dual opt-in when ShellOnly was
    // default — still accepted as dual (no-op when dual is already default).
    match std::env::var("GLINT_CEF_SINGLE_CONTENT_OSR") {
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("y") => OsrTopology::ContentOnly,
        _ => OsrTopology::ShellHostAndContent,
    }
}

fn expects_shell_cef_layer0(topology: OsrTopology) -> bool {
    matches!(
        topology,
        OsrTopology::ShellOnly | OsrTopology::ShellHostAndContent
    )
}

fn expects_content_cef_layer(topology: OsrTopology) -> bool {
    matches!(
        topology,
        OsrTopology::ShellHostAndContent | OsrTopology::ContentOnly
    )
}

/// Content Present-blit layer index. Always ≥1 so hole compositing never
/// collides with shell atlas layer 0 when both exist.
fn content_surface_layer(paint_layer: u32) -> u32 {
    paint_layer.max(1)
}

/// Whether `browser.setContentRect` may apply. Closed sessions ignore hole
/// updates so unmount cleanup cannot fight a reopen.
fn browser_session_accepts_content_rect(session_open: bool) -> bool {
    session_open
}

/// Mark browser session open. Returns true if this call newly opened it.
fn browser_session_mark_open(session_open: &mut bool) -> bool {
    if *session_open {
        return false;
    }
    *session_open = true;
    true
}

/// Mark browser session closed and clear hole/blank bookkeeping. Returns true
/// if this call newly closed it. Does not touch chrome layers or CEF process.
fn browser_session_mark_close(
    session_open: &mut bool,
    content_hole: &mut Option<(i32, i32, u32, u32)>,
    content_blank: &mut bool,
) -> bool {
    if !*session_open {
        return false;
    }
    *session_open = false;
    *content_hole = None;
    *content_blank = true;
    true
}

/// Apply content NavState URL to `content_blank`. Ignores stale `about:blank`
/// from close while a live session still has a published content hole.
fn apply_nav_state_blank(
    session_open: bool,
    content_hole: Option<(i32, i32, u32, u32)>,
    url: &str,
    content_blank: &mut bool,
) {
    let url_blank = url.is_empty() || url == "about:blank";
    if url_blank && session_open && content_hole.is_some() {
        return;
    }
    *content_blank = url_blank;
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
    browser_session_open: &mut bool,
) -> Option<Result<String, String>> {
    let mode_changing = matches!(
        method,
        "overlay.open"
            | "overlay.close"
            | "native.window.toggleInteractive"
            | "native.window.setMode"
    );
    if mode_changing && !owns_session {
        return Some(Err("overlay session not owned by CEF host".into()));
    }

    let queue_mode = |next: SessionMode, result: Result<String, String>| {
        if next != mode {
            let _ = ctrl_tx.send(HostCtrl::Mode(next));
        }
        let _ = ctrl_tx.send(HostCtrl::BridgeResult { request_id, result });
    };

    match method {
        "overlay.isOpen" => Some(Ok(if mode == SessionMode::Interactive {
            "true"
        } else {
            "false"
        }
        .into())),
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
            let keys = pins.lock().map(|g| g.clone()).unwrap_or_default();
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
            let body = Ok(serde_json::to_string(next.as_str())
                .unwrap_or_else(|_| format!("\"{}\"", next.as_str())));
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
                    return Some(Err("native.window.setMode requires mode string".into()));
                }
            };
            let next = match SessionMode::parse(name) {
                Some(m) => m,
                None => {
                    return Some(Err(format!("invalid overlay mode: {name}")));
                }
            };
            let body = Ok(serde_json::to_string(next.as_str())
                .unwrap_or_else(|_| format!("\"{}\"", next.as_str())));
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
            let blocked = conn.window(win_id).request(BlockInput { block }).await;
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
            // Leave a defined chrome target (URL/toolbar). Blurring content
            // alone left focus_target stale → keys still routed to content.
            if let Some(prev) = blur_grants_chrome(focus_target) {
                let _ = cef.set_focus(false, prev).await;
                let _ = cef.set_focus(true, CefFocusTarget::Chrome).await;
            }
            Some(Ok(String::new()))
        }
        "browser.setContentRect" => {
            let hole = match parse_content_rect(args_json) {
                Ok(h) => h,
                Err(err) => return Some(Err(err)),
            };
            // Unmount cleanup after closeSession must not fight a reopen.
            if !browser_session_accepts_content_rect(*browser_session_open) {
                return Some(Ok(String::new()));
            }
            *content_hole = hole;
            if let Err(err) = cef.set_content_rect(hole).await {
                return Some(Err(err.to_string()));
            }
            // Hole clear parks hit-testing (HudPinned ghost). Hole restore must
            // re-enable Content routing -- previously navigate did that as a side
            // effect; Shift+Tab must not LoadURL just to flip this flag.
            *content_blank = hole.is_none();
            Some(Ok(String::new()))
        }
        "browser.openSession" => {
            if !owns_session {
                return Some(Err(
                    "browser.openSession requires shell session owner".into()
                ));
            }
            let _ = browser_session_mark_open(browser_session_open);
            if mode != SessionMode::Interactive {
                queue_mode(SessionMode::Interactive, Ok(String::new()));
                None
            } else {
                Some(Ok(String::new()))
            }
        }
        "browser.closeSession" => {
            if !browser_session_mark_close(browser_session_open, content_hole, content_blank) {
                return Some(Ok(String::new()));
            }
            let _ = cef.set_content_rect(None).await;
            let _ = cef.content_navigate("about:blank".into()).await;
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
    let args: Vec<Value> =
        serde_json::from_str(args_json).map_err(|e| format!("browser.setContentRect args: {e}"))?;
    match args.first() {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => {
            let x = map
                .get("x")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "setContentRect.x required".to_string())? as i32;
            let y = map
                .get("y")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "setContentRect.y required".to_string())? as i32;
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

/// Titlebar-drag promo: cropped atlas snapshot on a dest-capable layer.
/// Exact window rect only — a shadow pad pulled in neighboring atlas pixels
/// (Metrics/dock) and dragged them with the window.
struct PromoDrag {
    layer: u32,
    origin: (f32, f32),
    size: (u32, u32),
    pad: (f32, f32),
    hole: Option<(i32, i32, u32, u32)>,
    hole_pos: Option<(f32, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SetPositionArgs {
    End,
    Dest { x: f32, y: f32 },
    Size { x: f32, y: f32, w: u32, h: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PositionOp {
    Ack,
    EndPromo,
    Promote {
        layer: u32,
        x: f32,
        y: f32,
        w: u32,
        h: u32,
    },
    DestPromo {
        layer: u32,
        x: f32,
        y: f32,
        w: u32,
        h: u32,
    },
    Floating {
        layer: u32,
        x: f32,
        y: f32,
        w: u32,
        h: u32,
        dest_only: bool,
    },
}

fn promo_layer_id(overlay_layer: u32) -> u32 {
    overlay_layer.saturating_add(1).max(2)
}

fn hole_owned_by_window(hole: (i32, i32, u32, u32), x: f32, y: f32, w: u32, h: u32) -> bool {
    let (hx, hy, hw, hh) = hole;
    let wx = x.round() as i32;
    let wy = y.round() as i32;
    hx >= wx
        && hy >= wy
        && hx.saturating_add_unsigned(hw) <= wx.saturating_add_unsigned(w)
        && hy.saturating_add_unsigned(hh) <= wy.saturating_add_unsigned(h)
}

fn translate_rect(
    start: (i32, i32, u32, u32),
    from: (f32, f32),
    to: (f32, f32),
) -> (f32, f32, u32, u32) {
    (
        start.0 as f32 + (to.0 - from.0),
        start.1 as f32 + (to.1 - from.1),
        start.2,
        start.3,
    )
}

fn visual_window_crop(
    x: f32,
    y: f32,
    w: u32,
    h: u32,
    atlas_w: u32,
    atlas_h: u32,
) -> Option<(Rect, (f32, f32))> {
    let src = clamp_crop_rect(x, y, w, h, atlas_w, atlas_h)?;
    Some((src, (0.0, 0.0)))
}

fn clamp_crop_rect(x: f32, y: f32, w: u32, h: u32, atlas_w: u32, atlas_h: u32) -> Option<Rect> {
    if atlas_w == 0 || atlas_h == 0 || w == 0 || h == 0 {
        return None;
    }
    let x = (x.round() as i32).clamp(0, atlas_w.saturating_sub(1) as i32) as u32;
    let y = (y.round() as i32).clamp(0, atlas_h.saturating_sub(1) as i32) as u32;
    Some(Rect {
        x,
        y,
        width: w.min(atlas_w.saturating_sub(x)).max(1),
        height: h.min(atlas_h.saturating_sub(y)).max(1),
    })
}

/// `native.overlay.setPosition` args: empty/`null` ends promo; `[x,y]` dests;
/// `[x,y,w,h]` promotes or ends after restore.
fn parse_set_position(args_json: &str) -> Result<SetPositionArgs, String> {
    let trimmed = args_json.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(SetPositionArgs::End);
    }
    let args: Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("native.overlay.setPosition args: {e}"))?;
    match args {
        Value::Null => Ok(SetPositionArgs::End),
        Value::Array(items) if items.is_empty() || items.iter().all(|v| v.is_null()) => {
            Ok(SetPositionArgs::End)
        }
        Value::Array(items) => {
            let x = items
                .first()
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "native.overlay.setPosition needs x".to_string())?
                as f32;
            let y = items
                .get(1)
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "native.overlay.setPosition needs y".to_string())?
                as f32;
            match (
                items.get(2).and_then(|v| v.as_f64()),
                items.get(3).and_then(|v| v.as_f64()),
            ) {
                (Some(w), Some(h)) => Ok(SetPositionArgs::Size {
                    x,
                    y,
                    w: w.round().max(1.0) as u32,
                    h: h.round().max(1.0) as u32,
                }),
                _ => Ok(SetPositionArgs::Dest { x, y }),
            }
        }
        _ => Err("native.overlay.setPosition expects an array".into()),
    }
}

/// Shell never writes session origin. Promo dests layer ≥2; floating dests ≥1.
fn position_op(
    owns_session: bool,
    overlay_layer: u32,
    promo: Option<(u32, u32, u32)>,
    args: SetPositionArgs,
    fallback_wh: (u32, u32),
) -> PositionOp {
    if !owns_session {
        return match args {
            SetPositionArgs::End => PositionOp::Ack,
            SetPositionArgs::Dest { x, y } => PositionOp::Floating {
                layer: overlay_layer.max(1),
                x,
                y,
                w: fallback_wh.0,
                h: fallback_wh.1,
                dest_only: true,
            },
            SetPositionArgs::Size { x, y, w, h } => PositionOp::Floating {
                layer: overlay_layer.max(1),
                x,
                y,
                w,
                h,
                dest_only: w == fallback_wh.0 && h == fallback_wh.1,
            },
        };
    }
    match (args, promo) {
        (SetPositionArgs::End, _) | (SetPositionArgs::Size { .. }, Some(_)) => PositionOp::EndPromo,
        (SetPositionArgs::Size { x, y, w, h }, None) => PositionOp::Promote {
            layer: promo_layer_id(overlay_layer),
            x,
            y,
            w,
            h,
        },
        (SetPositionArgs::Dest { x, y }, Some((layer, w, h))) => {
            PositionOp::DestPromo { layer, x, y, w, h }
        }
        (SetPositionArgs::Dest { .. }, None) => PositionOp::Ack,
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
    (0u32..64)
        .find_map(|d| Cursor::from_u32(d).filter(|c| format!("{c:?}").eq_ignore_ascii_case(name)))
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
        .request(PaintCmd::DeleteChromePaintBuffer {
            buffer_id: u64::from(layer),
        })
        .await;
    let _ = conn
        .window(win_id)
        .request(UpdateLayerHandle {
            layer,
            handle: None,
        })
        .await;
}

async fn clear_both_layers(
    conn: &mut IpcClientConn,
    chrome_surface: &mut OverlaySurface,
    content_surface: &mut OverlaySurface,
    promo_surface: &mut OverlaySurface,
    win_id: u32,
    overlay_layer: u32,
    content_layer: u32,
    chrome_stamped: &mut bool,
    content_stamped: &mut bool,
    promo: &mut Option<PromoDrag>,
    promo_stamped: &mut bool,
    overlay_last_pos: &mut Option<(f32, f32)>,
    content_last_pos: &mut Option<(f32, f32)>,
) {
    // Dual-layer chrome is 0. Legacy Paint.layer=None stamps overlay_layer on
    // the same surface — always release that slot too so park/teardown cannot
    // leave the composite stuck.
    clear_layer(conn, chrome_surface, win_id, 0, chrome_stamped).await;
    if overlay_layer != 0 {
        let _ = conn
            .window(win_id)
            .request(UpdateLayerHandle {
                layer: overlay_layer,
                handle: None,
            })
            .await;
    }
    clear_layer(
        conn,
        content_surface,
        win_id,
        content_layer,
        content_stamped,
    )
    .await;
    let promo_layer = promo
        .as_ref()
        .map(|p| p.layer)
        .unwrap_or_else(|| promo_layer_id(overlay_layer));
    clear_layer(conn, promo_surface, win_id, promo_layer, promo_stamped).await;
    *promo = None;
    *overlay_last_pos = None;
    *content_last_pos = None;
}

/// Content layer dest origin. Hole rect wins whenever published (spec 5.1).
fn content_draw_pos(
    w: u32,
    h: u32,
    browser_w: u32,
    browser_h: u32,
    content_hole: Option<(i32, i32, u32, u32)>,
    chrome_top_px: Option<u32>,
) -> (f32, f32) {
    if let Some((hx, hy, _, _)) = content_hole {
        return (hx as f32, hy as f32);
    }
    if w >= browser_w && h >= browser_h {
        return (0.0, 0.0);
    }
    if let Some(top) = chrome_top_px {
        (0.0, top as f32)
    } else {
        (0.0, 0.0)
    }
}

/// After `browser.setContentRect`: drop stamp when hole cleared; otherwise
/// dest-only move content layer to the hole (resize still comes from CEF paint).
async fn sync_content_hole_stamp(
    conn: &mut IpcClientConn,
    content_surface: &mut OverlaySurface,
    win_id: u32,
    content_stamp_layer: u32,
    content_hole: Option<(i32, i32, u32, u32)>,
    content_blank: bool,
    content_stamped: &mut bool,
    content_last_pos: &mut Option<(f32, f32)>,
) {
    if content_blank || content_hole.is_none() {
        if *content_stamped {
            clear_layer(
                conn,
                content_surface,
                win_id,
                content_stamp_layer,
                content_stamped,
            )
            .await;
            *content_last_pos = None;
        }
        return;
    }
    let Some((hx, hy, hw, hh)) = content_hole else {
        return;
    };
    if !*content_stamped {
        return;
    }
    let dest = (hx as f32, hy as f32);
    if *content_last_pos == Some(dest) {
        return;
    }
    if dest_layer_and_input(conn, win_id, content_stamp_layer.max(1), dest.0, dest.1, hw, hh)
        .await
        .is_ok()
    {
        *content_last_pos = Some(dest);
    }
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

/// CEF `Some(0)` / dest-only publish. Shell atlas stays layer 0 with no dest.
/// Floating panel stamps `overlay_layer.max(1)` at `(pos_x, pos_y)`.
fn chrome_dest_publish(
    owns_session: bool,
    overlay_layer: u32,
    pos_x: f32,
    pos_y: f32,
) -> (u32, Option<(f32, f32)>) {
    if owns_session {
        (0, None)
    } else {
        (overlay_layer.max(1), Some((pos_x, pos_y)))
    }
}

/// `(set_bounds, pos+29, input_rect)`. Never dest layer 0.
fn chrome_sync_cmds(layer: u32, dest_only: bool) -> (bool, bool, bool) {
    let dest = layer >= 1;
    (!dest_only, dest_only && dest, dest)
}

/// SetLayerPosition + opcode 29. No handle, no CEF set_bounds.
async fn send_layer_dest(
    conn: &mut IpcClientConn,
    win_id: u32,
    layer: u32,
    x: f32,
    y: f32,
    w: u32,
    h: u32,
) -> anyhow::Result<()> {
    conn.window(win_id)
        .request(SetLayerPosition {
            layer,
            x: PercentLength::Length(x),
            y: PercentLength::Length(y),
        })
        .await?;
    let _ = conn
        .window(win_id)
        .request(PaintCmd::DrawChromePaintBufferRect {
            buffer_id: u64::from(layer),
            x,
            y,
            width: w as f32,
            height: h as f32,
        })
        .await?;
    Ok(())
}

async fn dest_layer_and_input(
    conn: &mut IpcClientConn,
    win_id: u32,
    layer: u32,
    x: f32,
    y: f32,
    w: u32,
    h: u32,
) -> anyhow::Result<()> {
    send_layer_dest(conn, win_id, layer, x, y, w, h).await?;
    let _ = conn
        .window(win_id)
        .request(SetLayerInputRect {
            layer,
            rect: Some(LayerInputRect {
                x: PercentLength::Length(x),
                y: PercentLength::Length(y),
                width: w,
                height: h,
            }),
        })
        .await?;
    Ok(())
}

async fn dest_promo_and_hole(
    conn: &mut IpcClientConn,
    win_id: u32,
    hole_layer: u32,
    promo: &mut PromoDrag,
    x: f32,
    y: f32,
) -> anyhow::Result<()> {
    dest_layer_and_input(
        conn,
        win_id,
        promo.layer,
        x - promo.pad.0,
        y - promo.pad.1,
        promo.size.0,
        promo.size.1,
    )
    .await?;
    if let Some(hole) = promo.hole {
        let (hx, hy, hw, hh) = translate_rect(hole, promo.origin, (x, y));
        dest_layer_and_input(conn, win_id, hole_layer.max(1), hx, hy, hw, hh).await?;
        promo.hole_pos = Some((hx, hy));
    }
    Ok(())
}

fn crop_promo_update(
    chrome: &OverlaySurface,
    promo_surface: &mut OverlaySurface,
    src: Rect,
    last_chrome_share: Option<(u64, u32, u32)>,
) -> anyhow::Result<Option<glint_overlay_common::request::UpdateSharedHandle>> {
    match chrome.crop_rect(promo_surface, src) {
        Ok(update) => Ok(update),
        Err(err) => {
            let Some((handle, _, _)) = last_chrome_share else {
                return Err(err);
            };
            promo_surface.update_from_nt_shared(
                src.width,
                src.height,
                handle,
                Some(CopyRect {
                    dst_x: 0,
                    dst_y: 0,
                    src,
                }),
            )
        }
    }
}

async fn apply_position_op(
    op: PositionOp,
    conn: &mut IpcClientConn,
    cef: &mut CefSession,
    chrome: &mut OverlaySurface,
    promo_surface: &mut OverlaySurface,
    win_id: u32,
    overlay_layer: u32,
    content_stamp_layer: u32,
    content_hole: Option<(i32, i32, u32, u32)>,
    last_chrome_share: Option<(u64, u32, u32)>,
    promo: &mut Option<PromoDrag>,
    promo_stamped: &mut bool,
    pos_x: &mut f32,
    pos_y: &mut f32,
    browser_w: &mut u32,
    browser_h: &mut u32,
    next_layout_generation: &mut u32,
    last_sent_generation: &mut u32,
    last_bounds_sent: &mut Option<Instant>,
) -> anyhow::Result<()> {
    match op {
        PositionOp::Ack => Ok(()),
        PositionOp::EndPromo => {
            let layer = promo
                .as_ref()
                .map(|p| p.layer)
                .unwrap_or_else(|| promo_layer_id(overlay_layer));
            clear_layer(conn, promo_surface, win_id, layer, promo_stamped).await;
            *promo = None;
            Ok(())
        }
        PositionOp::Promote { layer, x, y, w, h } => {
            if layer < 2 {
                anyhow::bail!("promo layer must be ≥ 2, got {layer}");
            }
            let (atlas_w, atlas_h) = chrome
                .current_size()
                .or_else(|| last_chrome_share.map(|(_, aw, ah)| (aw, ah)))
                .ok_or_else(|| anyhow::anyhow!("no layer-0 mailbox to crop"))?;
            let (src, pad) = visual_window_crop(x, y, w, h, atlas_w, atlas_h)
                .ok_or_else(|| anyhow::anyhow!("promo crop empty"))?;
            let update = crop_promo_update(chrome, promo_surface, src, last_chrome_share)?;
            if let Some(update) = update {
                let ok = conn
                    .window(win_id)
                    .request(UpdateLayerHandle {
                        layer,
                        handle: update.handle,
                    })
                    .await?;
                if !ok {
                    anyhow::bail!("UpdateLayerHandle promo layer {layer} returned false");
                }
            }
            let hole = content_hole.filter(|rect| hole_owned_by_window(*rect, x, y, w, h));
            let mut drag = PromoDrag {
                layer,
                origin: (x, y),
                size: (src.width, src.height),
                pad,
                hole,
                hole_pos: hole.map(|(hx, hy, _, _)| (hx as f32, hy as f32)),
            };
            dest_promo_and_hole(conn, win_id, content_stamp_layer, &mut drag, x, y).await?;
            *promo_stamped = true;
            *promo = Some(drag);
            Ok(())
        }
        PositionOp::DestPromo {
            layer: _,
            x,
            y,
            w: _,
            h: _,
        } => {
            let Some(drag) = promo.as_mut() else {
                return Ok(());
            };
            dest_promo_and_hole(conn, win_id, content_stamp_layer, drag, x, y).await
        }
        PositionOp::Floating {
            layer,
            x,
            y,
            w,
            h,
            dest_only,
        } => {
            *pos_x = x;
            *pos_y = y;
            *browser_w = w;
            *browser_h = h;
            sync_inner_bounds(
                conn,
                cef,
                win_id,
                layer,
                x,
                y,
                w,
                h,
                true,
                dest_only,
                next_layout_generation,
                last_sent_generation,
                last_bounds_sent,
            )
            .await
        }
    }
}

/// Drive CEF inner window + overlay hit-test rect.
/// Dest-only: pos+29 if `layer ≥ 1`. Never `set_bounds`. Resize: throttle
/// `set_bounds` (16 ms; `force_flush` on release). Input rect when `layer ≥ 1`.
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
    dest_only: bool,
    next_layout_generation: &mut u32,
    last_sent_generation: &mut u32,
    last_bounds_sent: &mut Option<Instant>,
) -> anyhow::Result<()> {
    let (set_bounds, send_dest, input_rect) = chrome_sync_cmds(layer, dest_only);
    if set_bounds {
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
            cef.set_bounds(pos_x as i32, pos_y as i32, browser_w, browser_h, layout_gen)
                .await?;
        }
    } else if send_dest {
        send_layer_dest(conn, win_id, layer, pos_x, pos_y, browser_w, browser_h).await?;
    }

    if input_rect {
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
    }
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
    pos: Option<(f32, f32)>,
    last_pos: &mut Option<(f32, f32)>,
) -> anyhow::Result<()> {
    if handle == 0 {
        anyhow::bail!("invalid CEF paint handle {handle}");
    }
    let stamp_w = w.max(1);
    let stamp_h = h.max(1);

    if let Some(update) = surface.update_from_nt_shared(stamp_w, stamp_h, handle, None)? {
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
    }

    let dest = pos.unwrap_or((0.0, 0.0));
    if pos.is_some() && *last_pos != pos {
        send_layer_dest(conn, win_id, layer, dest.0, dest.1, stamp_w, stamp_h).await?;
        *last_pos = pos;
    } else {
        let _ = conn
            .window(win_id)
            .request(PaintCmd::DrawChromePaintBufferRect {
                buffer_id: u64::from(layer),
                x: dest.0,
                y: dest.1,
                width: stamp_w as f32,
                height: stamp_h as f32,
            })
            .await?;
    }
    if !*stamped {
        *stamped = true;
        info!(stamp_w, stamp_h, handle, layer, ?pos, "layer stamped");
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
    if edges.any() { Some(edges) } else { None }
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

/// `browser.blur` → chrome. Returns the previous non-chrome target to blur in CEF.
fn blur_grants_chrome(focus_target: &mut CefFocusTarget) -> Option<CefFocusTarget> {
    if *focus_target == CefFocusTarget::Chrome {
        return None;
    }
    let prev = *focus_target;
    *focus_target = CefFocusTarget::Chrome;
    Some(prev)
}

/// Hit-test window coords against the content hole / chrome-top strip.
/// Toolbar / URL / tabs (outside hole) → Chrome; interior of hole → Content.
///
/// ShellOnly: page is an in-shell iframe — **never** route Content via the
/// legacy `chrome_top_px` strip (that subtracts Y and remaps Content→chrome_
/// OsrClient → hit rides high above the visual cursor).
fn resolve_cursor_target(
    wx: i32,
    wy: i32,
    cx: i32,
    cy: i32,
    content_blank: bool,
    content_hole: Option<(i32, i32, u32, u32)>,
    chrome_top_px: Option<u32>,
    topology: OsrTopology,
) -> (CefFocusTarget, i32, i32) {
    match topology {
        OsrTopology::ShellOnly => (CefFocusTarget::Chrome, wx, wy),
        OsrTopology::ContentOnly => (CefFocusTarget::Content, wx, wy),
        OsrTopology::ShellHostAndContent => {
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
            // Dual OSR before hole arrives: crude toolbar strip (legacy).
            match chrome_top_px {
                Some(top) if !content_blank && cy >= top as i32 => {
                    (CefFocusTarget::Content, cx, cy - top as i32)
                }
                _ => (CefFocusTarget::Chrome, wx, wy),
            }
        }
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
    topology: OsrTopology,
    shell_drag: bool,
    focus_target: &mut CefFocusTarget,
    mouse_capture: &mut Option<CefFocusTarget>,
) {
    // client = panel-local (input-rect origin); window = game/surface space.
    let cx = cursor.client.x;
    let cy = cursor.client.y;
    let wx = cursor.window.x;
    let wy = cursor.window.y;
    // AppWindow move/resize: never hand the cursor to content OSR mid-drag.
    let geometric = if shell_drag {
        (CefFocusTarget::Chrome, wx, wy)
    } else {
        resolve_cursor_target(
            wx,
            wy,
            cx,
            cy,
            content_blank,
            content_hole,
            chrome_top_px,
            topology,
        )
    };
    let (target, lx, ly) = apply_mouse_capture(*mouse_capture, geometric, wx, wy, content_hole);
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
            if (0x01..=0x1A).contains(&code)
                && code != 0x08
                && code != 0x09
                && code != 0x0A
                && code != 0x0D
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
mod hotkey_tests {
    use super::{HOTKEY_DEBOUNCE, Hotkey};
    use glint_overlay_event::input::{Key, KeyInputState, KeyboardInput};
    use std::thread;
    use std::time::Duration;

    fn key(vk: u8, down: bool) -> KeyboardInput {
        KeyboardInput::Key {
            key: Key::new(vk, false).unwrap(),
            state: if down {
                KeyInputState::Pressed
            } else {
                KeyInputState::Released
            },
        }
    }

    #[test]
    fn shift_then_tab_toggles_once() {
        let mut h = Hotkey::default();
        assert!(!h.consume(&key(0x10, true)));
        assert!(h.consume(&key(0x09, true)));
        assert!(!h.consume(&key(0x09, true)));
        assert!(!h.consume(&key(0x09, false)));
    }

    #[test]
    fn right_shift_extended_still_toggles() {
        let mut h = Hotkey::default();
        let shift = KeyboardInput::Key {
            key: Key::new(0x10, true).unwrap(),
            state: KeyInputState::Pressed,
        };
        assert!(!h.consume(&shift));
        assert!(h.consume(&key(0x09, true)));
    }

    #[test]
    fn left_shift_vk_toggles() {
        let mut h = Hotkey::default();
        assert!(!h.consume(&key(0xA0, true)));
        assert!(h.consume(&key(0x09, true)));
    }

    #[test]
    fn right_shift_vk_toggles() {
        let mut h = Hotkey::default();
        assert!(!h.consume(&key(0xA1, true)));
        assert!(h.consume(&key(0x09, true)));
    }

    #[test]
    fn tab_repeat_does_not_close_after_debounce() {
        let mut h = Hotkey::default();
        assert!(!h.consume(&key(0x10, true)));
        assert!(h.consume(&key(0x09, true)));
        thread::sleep(HOTKEY_DEBOUNCE + Duration::from_millis(50));
        assert!(
            !h.consume(&key(0x09, true)),
            "held Tab auto-repeat must not toggle again"
        );
    }
}

#[cfg(test)]
mod heartbeat_tests {
    use super::{HEARTBEAT_INTERVAL, OVERLAY_HOTKEY, heartbeat_should_publish};
    use glint_overlay_common::request::HotkeyChord;
    use std::time::{Duration, Instant};

    #[test]
    fn first_publish_is_due() {
        let now = Instant::now();
        assert!(heartbeat_should_publish(None, now, false, OVERLAY_HOTKEY));
    }

    #[test]
    fn unchanged_before_interval_is_not_due() {
        let t0 = Instant::now();
        let last = Some((t0, false, OVERLAY_HOTKEY));
        assert!(!heartbeat_should_publish(
            last,
            t0 + Duration::from_millis(500),
            false,
            OVERLAY_HOTKEY
        ));
    }

    #[test]
    fn interval_elapsed_is_due() {
        let t0 = Instant::now();
        let last = Some((t0, true, OVERLAY_HOTKEY));
        assert!(heartbeat_should_publish(
            last,
            t0 + HEARTBEAT_INTERVAL,
            true,
            OVERLAY_HOTKEY
        ));
    }

    #[test]
    fn visibility_change_is_due() {
        let t0 = Instant::now();
        let last = Some((t0, false, OVERLAY_HOTKEY));
        assert!(heartbeat_should_publish(
            last,
            t0 + Duration::from_millis(10),
            true,
            OVERLAY_HOTKEY
        ));
    }

    #[test]
    fn hotkey_change_is_due() {
        let t0 = Instant::now();
        let last = Some((t0, true, OVERLAY_HOTKEY));
        let other = HotkeyChord {
            vk: 0x1B,
            modifiers: 0,
        };
        assert!(heartbeat_should_publish(
            last,
            t0 + Duration::from_millis(10),
            true,
            other
        ));
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
        assert_eq!(parse_blocking_cursor(Some(&json!(13))), Some(Cursor::Grab));
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
mod content_paint_tests {
    use super::content_draw_pos;

    #[test]
    fn content_dest_prefers_hole_even_when_paint_fills_browser() {
        let hole = Some((120, 80, 640, 400));
        // Previously returned (0,0) when paint >= browser — wrong for shell hole.
        assert_eq!(
            content_draw_pos(1920, 1080, 1920, 1080, hole, Some(48)),
            (120.0, 80.0)
        );
    }

    #[test]
    fn content_dest_falls_back_to_chrome_top_without_hole() {
        assert_eq!(
            content_draw_pos(800, 500, 900, 700, None, Some(48)),
            (0.0, 48.0)
        );
    }

    #[test]
    fn content_dest_origin_when_full_size_no_hole() {
        assert_eq!(
            content_draw_pos(900, 700, 900, 700, None, Some(48)),
            (0.0, 0.0)
        );
    }

    #[test]
    fn hole_translate_same_size_is_dest_only() {
        let prev = (40, 80, 800, 600);
        let next = (120, 100, 800, 600);
        assert_eq!(prev.2, next.2);
        assert_eq!(prev.3, next.3);
        assert_ne!((prev.0, prev.1), (next.0, next.1));
    }
}

#[cfg(test)]
mod browser_session_tests {
    use super::{
        OsrTopology, apply_nav_state_blank, browser_session_accepts_content_rect,
        browser_session_mark_close, browser_session_mark_open, content_surface_layer,
        expects_content_cef_layer, expects_shell_cef_layer0,
    };

    #[test]
    fn set_content_rect_ignored_when_session_closed() {
        assert!(!browser_session_accepts_content_rect(false));
        assert!(browser_session_accepts_content_rect(true));
    }

    #[test]
    fn open_close_are_idempotent() {
        let mut open = false;
        let mut hole = Some((10, 20, 300, 200));
        let mut blank = false;

        assert!(browser_session_mark_open(&mut open));
        assert!(open);
        assert!(!browser_session_mark_open(&mut open));

        assert!(browser_session_mark_close(&mut open, &mut hole, &mut blank));
        assert!(!open);
        assert!(hole.is_none());
        assert!(blank);
        assert!(!browser_session_mark_close(&mut open, &mut hole, &mut blank));
    }

    #[test]
    fn stale_blank_nav_ignored_with_live_hole() {
        let mut blank = false;
        apply_nav_state_blank(true, Some((0, 0, 100, 100)), "about:blank", &mut blank);
        assert!(!blank, "stale close blank must not blank a live hole");

        apply_nav_state_blank(true, None, "about:blank", &mut blank);
        assert!(blank, "intentional blank (no hole) still applies");

        blank = false;
        apply_nav_state_blank(false, Some((0, 0, 100, 100)), "about:blank", &mut blank);
        assert!(blank, "closed session still accepts blank navState");
    }

    #[test]
    fn open_navigate_close_reopen_cycle() {
        let mut open = false;
        let mut hole = Some((40, 80, 800, 600));
        let mut blank = false;

        // open → hole published → navigate (non-blank)
        assert!(browser_session_mark_open(&mut open));
        apply_nav_state_blank(open, hole, "https://example.com/", &mut blank);
        assert!(!blank);

        // close clears content bookkeeping only
        assert!(browser_session_mark_close(&mut open, &mut hole, &mut blank));
        assert!(blank && hole.is_none() && !open);

        // reopen accepts a new hole + navigate without needing CEF restart
        assert!(browser_session_mark_open(&mut open));
        hole = Some((40, 80, 800, 600));
        blank = false;
        // late about:blank from prior close must not stick once hole is back
        apply_nav_state_blank(open, hole, "about:blank", &mut blank);
        assert!(!blank);
        apply_nav_state_blank(open, hole, "https://example.com/reopen", &mut blank);
        assert!(!blank);
    }

    /// Opt-in ShellOnly helpers (GLINT_CEF_SHELL_ONLY): iframe page, no content twin.
    #[test]
    fn shell_only_iframe_topology_helpers() {
        let topology = OsrTopology::ShellOnly;
        assert!(expects_shell_cef_layer0(topology));
        assert!(!expects_content_cef_layer(topology));
        assert!(expects_shell_cef_layer0(OsrTopology::ShellHostAndContent));
        assert!(expects_content_cef_layer(OsrTopology::ShellHostAndContent));
        assert!(!expects_shell_cef_layer0(OsrTopology::ContentOnly));
        assert!(expects_content_cef_layer(OsrTopology::ContentOnly));

        let mut open = false;
        let mut hole = None;
        let mut blank = true;

        assert!(browser_session_mark_open(&mut open));
        apply_nav_state_blank(open, hole, "https://example.com/", &mut blank);
        assert!(!blank);

        assert!(browser_session_mark_close(&mut open, &mut hole, &mut blank));
        assert!(blank && hole.is_none() && !open);

        assert!(browser_session_mark_open(&mut open));
        apply_nav_state_blank(open, hole, "https://example.com/reopen", &mut blank);
        assert!(!blank);
    }

    /// Product-default dual OSR: shell atlas + content hole surface.
    #[test]
    fn dual_osr_product_default_expects_shell_and_content() {
        let topology = OsrTopology::ShellHostAndContent;
        assert!(expects_shell_cef_layer0(topology));
        assert!(expects_content_cef_layer(topology));
        assert_eq!(content_surface_layer(0), 1);
        assert_eq!(content_surface_layer(1), 1);
    }

    /// Legacy content-only spike topology helpers (env rollback path).
    #[test]
    fn single_osr_open_navigate_close_on_content_surface() {
        let topology = OsrTopology::ContentOnly;
        assert!(!expects_shell_cef_layer0(topology));
        assert!(expects_content_cef_layer(topology));
        assert_eq!(content_surface_layer(0), 1);
        assert_eq!(content_surface_layer(1), 1);
        assert_eq!(content_surface_layer(2), 2);

        let mut open = false;
        let mut hole = Some((40, 80, 800, 600));
        let mut blank = false;
        let mut content_layer = content_surface_layer(1);

        assert!(browser_session_mark_open(&mut open));
        apply_nav_state_blank(open, hole, "https://example.com/", &mut blank);
        assert!(!blank);
        assert_eq!(content_layer, 1);

        assert!(browser_session_mark_close(&mut open, &mut hole, &mut blank));
        assert!(blank && hole.is_none() && !open);

        assert!(browser_session_mark_open(&mut open));
        hole = Some((40, 80, 800, 600));
        content_layer = content_surface_layer(1);
        apply_nav_state_blank(open, hole, "https://example.com/reopen", &mut blank);
        assert!(!blank);
        assert_eq!(content_layer, 1);
        assert!(!expects_shell_cef_layer0(topology));
    }
}

#[cfg(test)]
mod mouse_capture_tests {
    use super::{
        CefFocusTarget, OsrTopology, apply_mouse_capture, blur_grants_chrome, resolve_cursor_target,
    };

    #[test]
    fn hole_hit_without_capture_goes_to_content() {
        let hole = Some((100, 100, 400, 300));
        let (t, lx, ly) = resolve_cursor_target(
            150,
            200,
            150,
            200,
            false,
            hole,
            None,
            OsrTopology::ShellHostAndContent,
        );
        assert_eq!(t, CefFocusTarget::Content);
        assert_eq!((lx, ly), (50, 100));
    }

    #[test]
    fn toolbar_above_hole_goes_to_chrome() {
        let hole = Some((100, 120, 400, 300));
        let (t, lx, ly) = resolve_cursor_target(
            150,
            80,
            150,
            80,
            false,
            hole,
            None,
            OsrTopology::ShellHostAndContent,
        );
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!((lx, ly), (150, 80));
    }

    #[test]
    fn beside_hole_goes_to_chrome() {
        let hole = Some((100, 100, 400, 300));
        let (t, ..) = resolve_cursor_target(
            50,
            200,
            50,
            200,
            false,
            hole,
            None,
            OsrTopology::ShellHostAndContent,
        );
        assert_eq!(t, CefFocusTarget::Chrome);
    }

    #[test]
    fn no_hole_routes_chrome() {
        let (t, ..) = resolve_cursor_target(
            150,
            200,
            150,
            200,
            true,
            None,
            Some(80),
            OsrTopology::ShellHostAndContent,
        );
        assert_eq!(t, CefFocusTarget::Chrome);
    }

    /// ShellOnly + iframe: no content hole; after navigate `content_blank` is false.
    /// Legacy chrome_top→Content strip must NOT fire (would SendMouse to chrome_
    /// with Y -= kChromeTopPx ≈ 130 → hit rides high above the visual cursor).
    #[test]
    fn shell_only_no_hole_always_chrome_surface_coords() {
        let (t, lx, ly) = resolve_cursor_target(
            400,
            500,
            50,
            200,
            false,
            None,
            Some(130),
            OsrTopology::ShellOnly,
        );
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!((lx, ly), (400, 500));
    }

    #[test]
    fn blank_flag_blocks_hole_even_when_rect_present() {
        // Shift+Tab hide sets content_blank via setContentRect(null); if restore
        // only brings the rect back and leaves blank=true, clicks miss Content.
        let hole = Some((100, 100, 400, 300));
        let (t, lx, ly) = resolve_cursor_target(
            150,
            200,
            150,
            200,
            true,
            hole,
            None,
            OsrTopology::ShellHostAndContent,
        );
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!((lx, ly), (150, 200));
    }

    #[test]
    fn chrome_capture_keeps_chrome_over_hole() {
        let hole = Some((100, 100, 400, 300));
        let geometric = resolve_cursor_target(
            150,
            200,
            150,
            200,
            false,
            hole,
            None,
            OsrTopology::ShellHostAndContent,
        );
        assert_eq!(geometric.0, CefFocusTarget::Content);
        let (t, lx, ly) =
            apply_mouse_capture(Some(CefFocusTarget::Chrome), geometric, 150, 200, hole);
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!((lx, ly), (150, 200));
    }

    #[test]
    fn blur_from_content_grants_chrome() {
        let mut t = CefFocusTarget::Content;
        assert_eq!(blur_grants_chrome(&mut t), Some(CefFocusTarget::Content));
        assert_eq!(t, CefFocusTarget::Chrome);
        assert_eq!(blur_grants_chrome(&mut t), None);
    }
}

#[cfg(test)]
mod chrome_dest_move_tests {
    use super::{
        PositionOp, SetPositionArgs, chrome_dest_publish, chrome_sync_cmds, clamp_crop_rect,
        hole_owned_by_window, parse_set_position, position_op, promo_layer_id, translate_rect,
        visual_window_crop,
    };

    #[test]
    fn shell_publish_is_layer_0_no_dest() {
        let (layer, dest) = chrome_dest_publish(true, 1, 40.0, 80.0);
        assert_eq!((layer, dest), (0, None));
        assert_eq!(chrome_sync_cmds(layer, true), (false, false, false));
    }

    #[test]
    fn floating_publish_is_dest_capable_not_layer_0() {
        let overlay_layer = 1u32;
        let (layer, dest) = chrome_dest_publish(false, overlay_layer, 40.0, 80.0);
        assert_eq!(layer, overlay_layer.max(1));
        assert_eq!(dest, Some((40.0, 80.0)));
        assert_ne!(layer, 0);
        assert_eq!(chrome_dest_publish(false, 0, 40.0, 80.0).0, 1);
        assert_eq!(chrome_sync_cmds(layer, true), (false, true, true));
    }

    #[test]
    fn dest_only_layer_ge_1_skips_set_bounds() {
        let (set_bounds, send_dest, input_rect) = chrome_sync_cmds(1, true);
        assert!(!set_bounds);
        assert!(send_dest);
        assert!(input_rect);
    }

    #[test]
    fn dest_only_promo_layer_2_skips_set_bounds() {
        let (set_bounds, send_dest, input_rect) = chrome_sync_cmds(2, true);
        assert!(!set_bounds);
        assert!(send_dest);
        assert!(input_rect);
    }

    #[test]
    fn dest_only_layer_0_sends_no_dest_cmds() {
        let (set_bounds, send_dest, input_rect) = chrome_sync_cmds(0, true);
        assert!(!set_bounds);
        assert!(!send_dest, "no SetLayerPosition / opcode 29 on layer 0");
        assert!(!input_rect, "no SetLayerInputRect on layer 0");
    }

    #[test]
    fn parse_set_position_origin_only() {
        assert_eq!(
            parse_set_position("[40,80]").unwrap(),
            SetPositionArgs::Dest { x: 40.0, y: 80.0 }
        );
    }

    #[test]
    fn parse_set_position_origin_and_size() {
        assert_eq!(
            parse_set_position("[10,20,400,300]").unwrap(),
            SetPositionArgs::Size {
                x: 10.0,
                y: 20.0,
                w: 400,
                h: 300
            }
        );
    }

    #[test]
    fn parse_set_position_empty_ends_promo() {
        assert_eq!(parse_set_position("[]").unwrap(), SetPositionArgs::End);
        assert_eq!(parse_set_position("null").unwrap(), SetPositionArgs::End);
        assert_eq!(parse_set_position("").unwrap(), SetPositionArgs::End);
    }

    #[test]
    fn promo_layer_never_zero_or_content() {
        assert_eq!(promo_layer_id(0), 2);
        assert_eq!(promo_layer_id(1), 2);
        assert_eq!(promo_layer_id(2), 3);
        assert!(promo_layer_id(1) != 0 && promo_layer_id(1) != 1);
    }

    #[test]
    fn shell_set_position_no_promo_xy_is_ack() {
        let op = position_op(
            true,
            1,
            None,
            SetPositionArgs::Dest { x: 400.0, y: 200.0 },
            (1920, 1080),
        );
        assert_eq!(op, PositionOp::Ack);
    }

    #[test]
    fn shell_set_position_size_promotes_layer_2() {
        let op = position_op(
            true,
            1,
            None,
            SetPositionArgs::Size {
                x: 80.0,
                y: 40.0,
                w: 640,
                h: 480,
            },
            (1920, 1080),
        );
        assert_eq!(
            op,
            PositionOp::Promote {
                layer: 2,
                x: 80.0,
                y: 40.0,
                w: 640,
                h: 480
            }
        );
    }

    #[test]
    fn shell_set_position_xy_while_promo_dests_promo() {
        let op = position_op(
            true,
            1,
            Some((2, 640, 480)),
            SetPositionArgs::Dest { x: 120.0, y: 60.0 },
            (1920, 1080),
        );
        assert_eq!(
            op,
            PositionOp::DestPromo {
                layer: 2,
                x: 120.0,
                y: 60.0,
                w: 640,
                h: 480
            }
        );
    }

    #[test]
    fn shell_set_position_size_while_promo_ends() {
        let op = position_op(
            true,
            1,
            Some((2, 640, 480)),
            SetPositionArgs::Size {
                x: 120.0,
                y: 60.0,
                w: 640,
                h: 480,
            },
            (1920, 1080),
        );
        assert_eq!(op, PositionOp::EndPromo);
    }

    #[test]
    fn floating_set_position_writes_origin_and_dests() {
        let op = position_op(
            false,
            0,
            None,
            SetPositionArgs::Size {
                x: 40.0,
                y: 80.0,
                w: 400,
                h: 300,
            },
            (400, 300),
        );
        assert_eq!(
            op,
            PositionOp::Floating {
                layer: 1,
                x: 40.0,
                y: 80.0,
                w: 400,
                h: 300,
                dest_only: true
            }
        );
    }

    #[test]
    fn hole_inside_window_is_owned() {
        assert!(hole_owned_by_window(
            (100, 80, 640, 360),
            80.0,
            40.0,
            800,
            500
        ));
        assert!(!hole_owned_by_window(
            (100, 80, 640, 360),
            400.0,
            200.0,
            200,
            150
        ));
    }

    #[test]
    fn hole_follows_origin_delta() {
        let (x, y, w, h) = translate_rect((100, 80, 640, 360), (80.0, 40.0), (120.0, 70.0));
        assert_eq!((x, y, w, h), (140.0, 110.0, 640, 360));
    }

    #[test]
    fn crop_rect_stays_inside_atlas() {
        let r = clamp_crop_rect(1800.0, 20.0, 400, 300, 1920, 1080).unwrap();
        assert_eq!(r.x, 1800);
        assert_eq!(r.width, 120);
        assert_eq!(r.height, 300);
    }

    #[test]
    fn visual_crop_is_exact_window_rect() {
        let (src, pad) = visual_window_crop(80.0, 40.0, 640, 480, 1920, 1080).unwrap();
        assert_eq!(pad, (0.0, 0.0));
        assert_eq!((src.x, src.y, src.width, src.height), (80, 40, 640, 480));
    }

    #[test]
    fn park_always_clears_promo_layer() {
        let overlay_layer = 1u32;
        let promo = Some(2u32);
        let layer = promo.unwrap_or_else(|| promo_layer_id(overlay_layer));
        assert_eq!(layer, 2);
        assert_ne!(layer, 0);
        assert_ne!(layer, 1);
    }
}

#[cfg(test)]
mod connection_json_tests {
    use super::{connection_json, parse_session_snapshot};

    const SNAPSHOT: &str = r#"{"games":[
        {"pid":1234,"name":"Hollow Knight","totalSeconds":45060},
        {"pid":555,"name":"Other","totalSeconds":10}
    ]}"#;

    #[test]
    fn matched_pid_carries_name_and_seconds() {
        let info = parse_session_snapshot(SNAPSHOT, 1234);
        assert_eq!(info, Some(("Hollow Knight".to_string(), 45060)));
        let value: serde_json::Value =
            serde_json::from_str(&connection_json(1234, info.as_ref(), None)).unwrap();
        assert_eq!(value["type"], "connection");
        assert_eq!(value["connected"], true);
        assert_eq!(value["pid"], 1234);
        assert_eq!(value["gameName"], "Hollow Knight");
        assert_eq!(value["playtimeSeconds"], 45060);
    }

    #[test]
    fn unmatched_or_broken_snapshot_omits_fields() {
        for raw in [
            SNAPSHOT,
            r#"{"games":[{"pid":999,"name":"X","totalSeconds":1}]}"#,
            r#"{"games":"nope"}"#,
            "not json",
            "",
        ] {
            assert_eq!(
                parse_session_snapshot(raw, 1234),
                if raw == SNAPSHOT { Some(("Hollow Knight".into(), 45060)) } else { None }
            );
            let value: serde_json::Value =
                serde_json::from_str(&connection_json(1234, None, None)).unwrap();
            assert!(value.get("gameName").is_none());
            assert!(value.get("playtimeSeconds").is_none());
        }
    }
}
