//! Achievements bind set + unlock file watchers — Electron
//! `achievements-service` / `achievements-core` parity (F008).

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::apps;
use crate::plugin_fs;

/// Host → UI toast payload (`UiMessage` type `achievement`).
#[derive(Debug, Clone)]
pub struct UnlockToast {
    pub game_name: String,
    pub title: String,
    pub icon_url: Option<String>,
}

static TOAST_TX: OnceLock<UnboundedSender<UnlockToast>> = OnceLock::new();
static LAST_TOAST_SOUND: Mutex<Option<Instant>> = Mutex::new(None);

static WATCH: Mutex<Option<WatchState>> = Mutex::new(None);

struct WatchState {
    seen_mtime: HashMap<String, u64>,
    remedy_seen: HashMap<String, HashSet<String>>,
    eos_offset: u64,
    started: bool,
}

fn watch_state(guard: &mut Option<WatchState>) -> &mut WatchState {
    guard.get_or_insert_with(|| WatchState {
        seen_mtime: HashMap::new(),
        remedy_seen: HashMap::new(),
        eos_offset: 0,
        started: false,
    })
}

/// Install the toast push channel (once per process, from the CEF event loop).
pub fn install_toast_sender(tx: UnboundedSender<UnlockToast>) {
    let _ = TOAST_TX.set(tx);
}

fn emit_unlock(game_name: &str, title: &str, icon_url: Option<&str>) {
    if let Some(tx) = TOAST_TX.get() {
        let _ = tx.send(UnlockToast {
            game_name: game_name.to_string(),
            title: title.to_string(),
            icon_url: icon_url.map(|s| s.to_string()),
        });
    }
}

/// WAV next to the shell dist (`PlaySound` cannot decode MP3).
fn toast_sound_path(rare: bool) -> Option<PathBuf> {
    let name = if rare {
        "XboxOneRareAchievement.wav"
    } else {
        "XboxAchievement.wav"
    };
    let mut candidates = Vec::new();
    if let Some(ui) = std::env::var_os("GLINT_UI_URL") {
        let mut dir = PathBuf::from(ui);
        if dir.is_file() {
            dir.pop();
        }
        candidates.push(dir.join("achievements").join(name));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("ui/shell/dist/achievements").join(name));
            candidates.push(dir.join("achievements").join(name));
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(
        manifest
            .join("../../../ui/shell/public/achievements")
            .join(name),
    );
    candidates.push(
        manifest
            .join("../../../ui/shell/dist/achievements")
            .join(name),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// CEF HTML5 audio is blocked without a user gesture (and HudPinned has none),
/// so unlocks play through winmm instead of `Audio.play()`.
pub(crate) fn play_toast_sound(rare: bool) {
    if let Ok(mut last) = LAST_TOAST_SOUND.lock() {
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < Duration::from_millis(400)) {
            return;
        }
        *last = Some(now);
    }
    let Some(path) = toast_sound_path(rare) else {
        tracing::warn!("achievement toast wav not found");
        return;
    };
    let _ = std::thread::Builder::new()
        .name("ach-toast-snd".into())
        .spawn(move || {
            use windows::Win32::Media::Audio::{
                PlaySoundW, SND_FILENAME, SND_NODEFAULT, SND_SYNC,
            };
            let sound = windows::core::HSTRING::from(path.as_os_str());
            unsafe {
                let _ = PlaySoundW(&sound, None, SND_FILENAME | SND_NODEFAULT | SND_SYNC);
            }
        });
}

fn achievements_dir() -> PathBuf {
    apps::apps_root().join("achievements")
}

fn db_path() -> PathBuf {
    achievements_dir().join("data").join("app.db")
}

fn open_db() -> Result<Connection, String> {
    let data = achievements_dir().join("data");
    fs::create_dir_all(&data).map_err(|e| format!("achievements mkdir: {e}"))?;
    let conn = Connection::open(db_path()).map_err(|e| format!("achievements db open: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS sync_state (
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tracked_games (
           id TEXT PRIMARY KEY,
           name TEXT NOT NULL,
           platform TEXT NOT NULL,
           exe_path TEXT,
           process_name TEXT,
           source TEXT NOT NULL,
           updated_at INTEGER NOT NULL,
           deleted_at INTEGER,
           rev INTEGER NOT NULL DEFAULT 1
         );
         CREATE TABLE IF NOT EXISTS achievement_defs (
           game_id TEXT NOT NULL,
           achievement_id TEXT NOT NULL,
           title TEXT NOT NULL,
           description TEXT,
           icon_locked TEXT,
           icon_unlocked TEXT,
           updated_at INTEGER NOT NULL,
           rev INTEGER NOT NULL DEFAULT 1,
           PRIMARY KEY (game_id, achievement_id)
         );
         CREATE TABLE IF NOT EXISTS achievement_state (
           game_id TEXT NOT NULL,
           achievement_id TEXT NOT NULL,
           unlocked INTEGER NOT NULL DEFAULT 0,
           unlocked_at INTEGER,
           progress REAL,
           updated_at INTEGER NOT NULL,
           deleted_at INTEGER,
           rev INTEGER NOT NULL DEFAULT 1,
           PRIMARY KEY (game_id, achievement_id)
         );",
    )
    .map_err(|e| format!("achievements schema: {e}"))?;
    let has: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_state WHERE key = 'device_id'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("achievements device_id: {e}"))?;
    if has.is_none() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| format!("{:x}", d.as_nanos()))
            .unwrap_or_else(|_| "1".into());
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES ('device_id', ?1)",
            params![id],
        )
        .map_err(|e| format!("achievements device_id insert: {e}"))?;
    }
    Ok(conn)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn upsert_tracked_game(
    conn: &Connection,
    id: &str,
    name: &str,
    platform: &str,
    source: &str,
    exe_path: Option<&str>,
    process_name: Option<&str>,
) -> Result<(), String> {
    let t = now_ms();
    conn.execute(
        "INSERT INTO tracked_games
           (id, name, platform, exe_path, process_name, source, updated_at, deleted_at, rev)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 1)
         ON CONFLICT(id) DO UPDATE SET
           name = excluded.name,
           platform = excluded.platform,
           exe_path = COALESCE(excluded.exe_path, tracked_games.exe_path),
           process_name = COALESCE(excluded.process_name, tracked_games.process_name),
           source = excluded.source,
           updated_at = excluded.updated_at,
           rev = tracked_games.rev + 1,
           deleted_at = NULL",
        params![id, name, platform, exe_path, process_name, source, t],
    )
    .map_err(|e| format!("upsertTrackedGame: {e}"))?;
    Ok(())
}

fn upsert_def(
    conn: &Connection,
    game_id: &str,
    achievement_id: &str,
    title: &str,
    description: Option<&str>,
    icon_unlocked: Option<&str>,
) -> Result<(), String> {
    let t = now_ms();
    conn.execute(
        "INSERT INTO achievement_defs
           (game_id, achievement_id, title, description, icon_locked, icon_unlocked, updated_at, rev)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, 1)
         ON CONFLICT(game_id, achievement_id) DO UPDATE SET
           title = excluded.title,
           description = COALESCE(excluded.description, achievement_defs.description),
           icon_unlocked = COALESCE(excluded.icon_unlocked, achievement_defs.icon_unlocked),
           updated_at = excluded.updated_at,
           rev = achievement_defs.rev + 1",
        params![game_id, achievement_id, title, description, icon_unlocked, t],
    )
    .map_err(|e| format!("upsertDef: {e}"))?;
    Ok(())
}

/// Returns true if this is a newly recorded unlock.
fn record_unlock(
    conn: &Connection,
    game_id: &str,
    achievement_id: &str,
    title: &str,
    description: Option<&str>,
    icon_unlocked: Option<&str>,
    unlocked_at: Option<i64>,
) -> Result<bool, String> {
    upsert_def(
        conn,
        game_id,
        achievement_id,
        title,
        description,
        icon_unlocked,
    )?;
    let prev: Option<i64> = conn
        .query_row(
            "SELECT unlocked FROM achievement_state
             WHERE game_id = ?1 AND achievement_id = ?2 AND deleted_at IS NULL",
            params![game_id, achievement_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| format!("recordUnlock prev: {e}"))?;
    let was = prev == Some(1);
    let t = now_ms();
    let at = unlocked_at.unwrap_or(t);
    conn.execute(
        "INSERT INTO achievement_state
           (game_id, achievement_id, unlocked, unlocked_at, progress, updated_at, deleted_at, rev)
         VALUES (?1, ?2, 1, ?3, NULL, ?4, NULL, 1)
         ON CONFLICT(game_id, achievement_id) DO UPDATE SET
           unlocked = 1,
           unlocked_at = COALESCE(achievement_state.unlocked_at, excluded.unlocked_at),
           updated_at = excluded.updated_at,
           rev = achievement_state.rev + 1,
           deleted_at = NULL",
        params![game_id, achievement_id, at, t],
    )
    .map_err(|e| format!("recordUnlock: {e}"))?;
    Ok(!was)
}

fn list_games(conn: &Connection) -> Result<Value, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, platform, exe_path, process_name, source
             FROM tracked_games WHERE deleted_at IS NULL ORDER BY updated_at DESC",
        )
        .map_err(|e| format!("listGames: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "platform": r.get::<_, String>(2)?,
                "exe_path": r.get::<_, Option<String>>(3)?,
                "process_name": r.get::<_, Option<String>>(4)?,
                "source": r.get::<_, String>(5)?,
            }))
        })
        .map_err(|e| format!("listGames map: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("listGames row: {e}"))?);
    }
    Ok(Value::Array(out))
}

fn list_for_game(conn: &Connection, game_id: &str) -> Result<Value, String> {
    let mut stmt = conn
        .prepare(
            "SELECT
               d.game_id, d.achievement_id, d.title, d.description, d.icon_unlocked,
               COALESCE(s.unlocked, 0), s.unlocked_at, s.progress
             FROM achievement_defs d
             LEFT JOIN achievement_state s
               ON s.game_id = d.game_id AND s.achievement_id = d.achievement_id
             WHERE d.game_id = ?1
             ORDER BY COALESCE(s.unlocked, 0) DESC, d.title",
        )
        .map_err(|e| format!("listForGame: {e}"))?;
    let rows = stmt
        .query_map(params![game_id], |r| {
            Ok(json!({
                "game_id": r.get::<_, String>(0)?,
                "achievement_id": r.get::<_, String>(1)?,
                "title": r.get::<_, String>(2)?,
                "description": r.get::<_, Option<String>>(3)?,
                "icon_unlocked": r.get::<_, Option<String>>(4)?,
                "unlocked": r.get::<_, i64>(5)?,
                "unlocked_at": r.get::<_, Option<i64>>(6)?,
                "progress": r.get::<_, Option<f64>>(7)?,
            }))
        })
        .map_err(|e| format!("listForGame map: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("listForGame row: {e}"))?);
    }
    Ok(Value::Array(out))
}

fn guide(kind: &str) -> Value {
    let (kind, title, url, steps): (&str, &str, &str, &[&str]) = match kind {
        "gse" => (
            "gse",
            "Install Goldberg / GSE for Steam achievements",
            "https://github.com/Detanup01/gbe_fork/releases",
            &[
                "Download a GSE / Goldberg Steam Emulator release.",
                "In the game folder, rename steam_api64.dll to steam_api64.dll.bak (or steam_api.dll).",
                "Copy the emulator DLLs and config next to the game executable.",
                "Generate achievements schema (generate_emu_config / GSE tools) for the Steam AppID.",
                "Play once; Glint watches %APPDATA%\\GSE Saves for unlocks.",
            ],
        ),
        "epic-emu" => (
            "epic-emu",
            "Epic emulator achievements (Nemirtingas)",
            "https://pserban93.github.io/Achievements-Docs/platforms.html",
            &[
                "Nemirtingas EOSSDK detected (nepice_settings / NemirtingasEpicEmu.json).",
                "Launch with EpicEmu sandbox args when the launcher offers them.",
                "Play once; unlocks appear under %APPDATA%\\NemirtingasEpicEmu.",
                "Docs: https://pserban93.github.io/Achievements-Docs/platforms.html",
            ],
        ),
        "epic-codex" => (
            "epic-codex",
            "Epic CODEX / EOS stub achievements",
            "https://pserban93.github.io/Achievements-Docs/platforms.html",
            &[
                "CODEX (or similar) EOSSDK stub detected — no Nemirtingas AppData path.",
                "Sync achievement definitions from the library (Epic public API).",
                "Play with the overlay/metrics inject; unlocks are hooked from EOS_Achievements_UnlockAchievements.",
                "Do not use Nemirtingas launch args with CODEX.",
            ],
        ),
        "steam-official" => (
            "steam-official",
            "Steam official local stats",
            "https://store.steampowered.com/",
            &[
                "Install and sign in to Steam.",
                "Play the game through Steam so appcache\\stats is written.",
                "For cracked/offline builds without Steam, use the GSE guide instead.",
            ],
        ),
        "epic-official" => (
            "epic-official",
            "Epic Games Launcher (official)",
            "https://store.epicgames.com/",
            &[
                "Install Epic Games Launcher and own the title.",
                "Epic account linking / polling lands in a later pass; for offline/emu builds use NemirtingasEpicEmu.",
                "See https://pserban93.github.io/Achievements-Docs/platforms.html for Epic official flow.",
            ],
        ),
        _ => (
            "unknown",
            "Unsupported or unknown achievement backend",
            "https://pserban93.github.io/Achievements-Docs/platforms.html",
            &[
                "No Steamworks or EOS SDK DLLs were found next to the executable.",
                "If this is a Steam game, install GSE/Goldberg.",
                "If this is an Epic game, install NemirtingasEpicEmu.",
                "Docs: https://pserban93.github.io/Achievements-Docs/platforms.html",
            ],
        ),
    };
    json!({
        "kind": kind,
        "title": title,
        "downloadUrl": url,
        "steps": steps,
    })
}

fn detect_eos_vendor(game_dir: &Path, names_lower: &HashSet<String>) -> &'static str {
    if names_lower.contains("nemirtingasepicemu.json") || game_dir.join("nepice_settings").exists()
    {
        return "nemirtingas";
    }
    if names_lower.contains("eossdk-win64-shipping.cdx")
        || names_lower.contains("eossdk-win32-shipping.cdx")
    {
        return "codex";
    }
    "unknown"
}

fn detect_exe(exe_path: &str) -> Value {
    let dir = Path::new(exe_path).parent().unwrap_or(Path::new("."));
    let names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let lower: HashSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mut markers = Vec::new();

    let has_steam = lower.contains("steam_api64.dll")
        || lower.contains("steam_api.dll")
        || lower.contains("steam_api64.dll.bak")
        || lower.contains("steam_api.dll.bak");
    let has_gse = lower.contains("steam_settings")
        || names.iter().any(|n| n.to_lowercase().contains("gse"))
        || dir.join("steam_settings").exists();
    let has_eos = lower.contains("eossdk-win64-shipping.dll")
        || lower.contains("eossdk-win64-shipping.cdx")
        || lower.contains("eossdk-win32-shipping.dll")
        || dir.join("nepice_settings").exists();
    let has_epic_emu = lower.contains("epic_emu.ini")
        || lower.contains("nemirtingasepicemu.json")
        || lower.contains("fetch_epic_achievements.bat")
        || dir.join("nepice_settings").exists()
        || dir.join("epic_emu.ini").exists();

    if has_steam {
        markers.push("steam_api");
    }
    if has_gse {
        markers.push("gse");
    }
    if has_eos {
        markers.push("eossdk");
    }
    if has_epic_emu {
        markers.push("epic_emu");
    }

    if has_steam && has_gse {
        return json!({
            "platform": "steam", "supported": true, "markers": markers, "guide": guide("gse")
        });
    }
    if has_steam && !has_gse {
        return json!({
            "platform": "steam", "supported": false, "markers": markers, "guide": guide("gse")
        });
    }
    if has_eos || has_epic_emu {
        let vendor = detect_eos_vendor(dir, &lower);
        if vendor == "codex" {
            markers.push("codex");
        }
        if vendor == "nemirtingas" {
            markers.push("nemirtingas");
        }
        let supported = vendor == "codex" || vendor == "nemirtingas" || (has_eos && has_epic_emu);
        let g = if vendor == "codex" {
            guide("epic-codex")
        } else {
            guide("epic-emu")
        };
        return json!({
            "platform": "epic",
            "supported": supported,
            "markers": markers,
            "guide": g,
            "epicVendor": vendor,
            "offerEpicEmuLaunch": vendor == "nemirtingas",
        });
    }
    json!({
        "platform": "unknown", "supported": false, "markers": markers, "guide": guide("unknown")
    })
}

fn track_exe(exe_path: &str) -> Result<Value, String> {
    let detect = detect_exe(exe_path);
    let base = Path::new(exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(exe_path);
    let name = base
        .strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base);
    let platform = detect["platform"].as_str().unwrap_or("unknown");
    let supported = detect["supported"].as_bool().unwrap_or(false);
    let (source, id, plat) = if platform == "steam" {
        (
            if supported { "gse" } else { "steam-official" },
            format!("steam-exe:{}", base.to_lowercase()),
            "steam",
        )
    } else if platform == "epic" {
        (
            if supported {
                "epic-emu"
            } else {
                "epic-official"
            },
            format!("epic-exe:{}", base.to_lowercase()),
            "epic",
        )
    } else {
        (
            "steam-official",
            format!("custom:{}", base.to_lowercase()),
            "unknown",
        )
    };
    let conn = open_db()?;
    upsert_tracked_game(&conn, &id, name, plat, source, Some(exe_path), Some(base))?;
    Ok(detect)
}

fn find_game_for_process(
    conn: &Connection,
    exe_path: &str,
    exe_name: &str,
) -> Result<Value, String> {
    let Value::Array(arr) = list_games(conn)? else {
        return Ok(Value::Null);
    };
    let path_l = exe_path.to_lowercase();
    let name_l = exe_name.to_lowercase();
    let mut matches: Vec<Value> = arr
        .into_iter()
        .filter(|g| {
            let ep = g["exe_path"].as_str().unwrap_or("").to_lowercase();
            let pn = g["process_name"].as_str().unwrap_or("").to_lowercase();
            (!ep.is_empty() && ep == path_l) || (!pn.is_empty() && pn == name_l)
        })
        .collect();
    if matches.is_empty() {
        return Ok(Value::Null);
    }
    let with_defs = matches.iter().position(|g| {
        g["id"]
            .as_str()
            .and_then(|id| list_for_game(conn, id).ok())
            .and_then(|v| v.as_array().map(|a| !a.is_empty()))
            .unwrap_or(false)
    });
    Ok(matches.remove(with_defs.unwrap_or(0)))
}

fn open_url(url: &str) -> Result<String, String> {
    if std::env::var("GLINT_OPENURL_STUB").is_ok() {
        return Ok(String::new());
    }
    let err = plugin_fs::shell_execute_open(url);
    if err.is_empty() {
        Ok(String::new())
    } else {
        Err(err)
    }
}

fn resolve_exe_for_toast() -> (String, String) {
    if let Ok(p) = std::env::var("GLINT_GAME_EXE") {
        if !p.is_empty() {
            let n = Path::new(&p)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            return (p, n);
        }
    }
    let Ok(pid) = std::env::var("GLINT_GAME_PID") else {
        return (String::new(), String::new());
    };
    let Ok(pid) = pid.parse::<u32>() else {
        return (String::new(), String::new());
    };
    // Same QueryFullProcessImageNameW path as F004 (local copy — avoid ipc cycle).
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (String::new(), String::new());
        };
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if ok.is_err() {
            return (String::new(), String::new());
        }
        let p = String::from_utf16_lossy(&buf[..size as usize]);
        let n = Path::new(&p)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        (p, n)
    }
}

fn test_toast() -> Result<String, String> {
    let conn = open_db()?;
    let (exe_path, exe_name) = resolve_exe_for_toast();

    let game = find_game_for_process(&conn, &exe_path, &exe_name)?;
    let game = if game.is_null() {
        find_game_for_process(&conn, "", &exe_name)?
    } else {
        game
    };

    let unlocked: Vec<Value> = if let Some(id) = game["id"].as_str() {
        list_for_game(&conn, id)?
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r["unlocked"].as_i64().unwrap_or(0) == 1)
            .collect()
    } else {
        Vec::new()
    };

    let mut unlocked = unlocked;
    unlocked.sort_by(|a, b| {
        b["unlocked_at"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&a["unlocked_at"].as_i64().unwrap_or(0))
    });
    unlocked.truncate(4);

    if unlocked.is_empty() {
        emit_unlock(
            game["name"].as_str().unwrap_or("Toast test"),
            "No unlocked achievements to replay",
            None,
        );
        return Ok(String::new());
    }
    let game_name = game["name"].as_str().unwrap_or("Game").to_string();
    for row in unlocked {
        emit_unlock(
            &game_name,
            row["title"].as_str().unwrap_or("Achievement unlocked"),
            row["icon_unlocked"].as_str(),
        );
    }
    Ok(String::new())
}

/// Dispatch `achievements.*` (caller already gated to achievements app).
pub fn dispatch(method: &str, args_json: &str) -> Result<String, String> {
    let args: Vec<Value> =
        serde_json::from_str(args_json).map_err(|e| format!("invalid achievements args: {e}"))?;
    let s = |i: usize| -> String {
        match args.get(i) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        }
    };
    match method {
        "achievements.listGames" => {
            let conn = open_db()?;
            list_games(&conn).map(|v| v.to_string())
        }
        "achievements.listForGame" => {
            let conn = open_db()?;
            list_for_game(&conn, &s(0)).map(|v| v.to_string())
        }
        "achievements.detectExe" => Ok(detect_exe(&s(0)).to_string()),
        "achievements.trackExe" => track_exe(&s(0)).map(|v| v.to_string()),
        "achievements.forProcess" => {
            let exe = s(0);
            let name = if args.get(1).is_some() {
                s(1)
            } else {
                Path::new(&exe)
                    .file_name()
                    .and_then(|x| x.to_str())
                    .unwrap_or("")
                    .to_string()
            };
            let conn = open_db()?;
            find_game_for_process(&conn, &exe, &name).map(|v| v.to_string())
        }
        "achievements.getGuide" => Ok(guide(&s(0)).to_string()),
        "achievements.openUrl" => open_url(&s(0)),
        "achievements.testToast" => test_toast(),
        _ => Err(format!("unknown achievements method: {method}")),
    }
}

// ── Unlock file watchers (poll pump; Electron used fs.watch + Remedy poll) ──

fn appdata() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn gse_dir() -> PathBuf {
    appdata().join("GSE Saves")
}
fn epic_emu_dir() -> PathBuf {
    appdata().join("NemirtingasEpicEmu")
}
fn eos_hooks_path() -> PathBuf {
    achievements_dir().join("eos-hooks.jsonl")
}
fn remedy_dir() -> PathBuf {
    local_appdata().join("Remedy")
}

fn mtime_ms(path: &Path) -> Option<u64> {
    path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

fn should_scan(path: &Path, state: &mut WatchState) -> bool {
    let key = path.to_string_lossy().into_owned();
    let Some(mt) = mtime_ms(path) else {
        return false;
    };
    if state.seen_mtime.get(&key) == Some(&mt) {
        return false;
    }
    state.seen_mtime.insert(key, mt);
    true
}

fn resolve_gse_unlock_target(conn: &Connection, app_id: &str) -> (String, String) {
    let mapped: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_state WHERE key = 'library_by_appid:' || ?1",
            params![app_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let Some(game_id) = mapped else {
        return (format!("gse:{app_id}"), format!("Steam {app_id}"));
    };
    let toast = conn
        .query_row(
            "SELECT name FROM tracked_games WHERE id = ?1",
            params![&game_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or_else(|| game_id.clone());
    (game_id, toast)
}

fn parse_gse(app_id: &str, file: &Path) {
    let Ok(raw) = fs::read_to_string(file) else {
        return;
    };
    let Ok(val) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    let Ok(conn) = open_db() else {
        return;
    };
    let (game_id, toast_name) = resolve_gse_unlock_target(&conn, app_id);
    if game_id == format!("gse:{app_id}") {
        let _ = upsert_tracked_game(&conn, &game_id, &toast_name, "steam", "gse", None, None);
    }
    let Some(obj) = val.as_object() else {
        return;
    };
    let defs = list_for_game(&conn, &game_id)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    for (ach_id, value) in obj {
        let Some(v) = value.as_object() else {
            continue;
        };
        let earned = v.get("earned").and_then(|x| x.as_bool()).unwrap_or(false)
            || v.get("unlocked").and_then(|x| x.as_bool()).unwrap_or(false)
            || v.get("earned").and_then(|x| x.as_i64()) == Some(1)
            || v.get("earned_time").and_then(|x| x.as_f64()).unwrap_or(0.0) > 0.0;
        if !earned {
            continue;
        }
        let save_title = v
            .get("displayName")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("name").and_then(|x| x.as_str()));
        let (title, icon) = toast_from_def(&defs, ach_id, save_title);
        let unlocked_at = v
            .get("earned_time")
            .and_then(|x| x.as_f64())
            .map(|t| (t * 1000.0) as i64);
        let desc = defs
            .iter()
            .find(|d| d["achievement_id"].as_str() == Some(ach_id.as_str()))
            .and_then(|d| d["description"].as_str())
            .or_else(|| v.get("description").and_then(|x| x.as_str()));
        if let Ok(true) = record_unlock(
            &conn,
            &game_id,
            ach_id,
            &title,
            desc,
            icon.as_deref(),
            unlocked_at,
        ) {
            emit_unlock(&toast_name, &title, icon.as_deref());
        }
    }
}

/// Prefer Steam schema title/icon over GSE save keys (API ids, no CDN URL).
fn toast_from_def(
    defs: &[Value],
    ach_id: &str,
    save_title: Option<&str>,
) -> (String, Option<String>) {
    let def = defs
        .iter()
        .find(|d| d["achievement_id"].as_str() == Some(ach_id));
    let title = def
        .and_then(|d| d["title"].as_str())
        .or(save_title)
        .unwrap_or(ach_id)
        .to_string();
    let icon = def
        .and_then(|d| d["icon_unlocked"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (title, icon)
}

fn scan_gse(state: &mut WatchState) {
    let dir = gse_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for ent in entries.flatten() {
        let app_id = ent.file_name().to_string_lossy().into_owned();
        let ach = ent.path().join("achievements.json");
        if !ach.is_file() || !should_scan(&ach, state) {
            continue;
        }
        parse_gse(&app_id, &ach);
    }
}

fn parse_epic(namespace: &str, file: &Path) {
    let Ok(raw) = fs::read_to_string(file) else {
        return;
    };
    let Ok(val) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    let Ok(conn) = open_db() else {
        return;
    };
    let game_id = format!("epic-emu:{namespace}");
    let short = format!("Epic {}…", &namespace.chars().take(8).collect::<String>());
    let _ = upsert_tracked_game(&conn, &game_id, &short, "epic", "epic-emu", None, None);

    let list: Vec<Value> = if let Some(a) = val.as_array() {
        a.clone()
    } else if let Some(a) = val.get("achievements").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(obj) = val.as_object() {
        obj.iter()
            .map(|(id, v)| {
                let mut row = json!({"id": id});
                if let Some(o) = v.as_object() {
                    for (k, vv) in o {
                        row[k] = vv.clone();
                    }
                }
                row
            })
            .collect()
    } else {
        return;
    };

    for item in list {
        let Some(row) = item.as_object() else {
            continue;
        };
        let ach_id = row
            .get("id")
            .or_else(|| row.get("achievementId"))
            .or_else(|| row.get("name"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if ach_id.is_empty() {
            continue;
        }
        let unlocked = row
            .get("unlocked")
            .and_then(|x| x.as_bool())
            .unwrap_or(false)
            || row.get("unlocked").and_then(|x| x.as_i64()) == Some(1)
            || row.get("progress").and_then(|x| x.as_i64()) == Some(100)
            || row.contains_key("UnlockTime");
        if !unlocked {
            continue;
        }
        let title = row
            .get("title")
            .or_else(|| row.get("displayName"))
            .or_else(|| row.get("name"))
            .and_then(|x| x.as_str())
            .unwrap_or(&ach_id);
        let icon = row
            .get("icon")
            .or_else(|| row.get("UnlockedIcon_URL"))
            .and_then(|x| x.as_str());
        let desc = row.get("description").and_then(|x| x.as_str());
        if let Ok(true) = record_unlock(&conn, &game_id, &ach_id, title, desc, icon, None) {
            emit_unlock(&short, title, icon);
        }
    }
}

fn scan_epic(state: &mut WatchState) {
    let dir = epic_emu_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for ent in entries.flatten() {
        let base = ent.path();
        if !base.is_dir() {
            continue;
        }
        let ns = ent.file_name().to_string_lossy().into_owned();
        for name in ["achievements.json", "achievements_db.json", "stats.json"] {
            let file = base.join(name);
            if file.is_file() && should_scan(&file, state) {
                parse_epic(&ns, &file);
            }
        }
    }
}

fn scan_eos_hooks(state: &mut WatchState) {
    let path = eos_hooks_path();
    if !path.is_file() {
        return;
    }
    let Ok(meta) = path.metadata() else {
        return;
    };
    let size = meta.len();
    if size < state.eos_offset {
        state.eos_offset = 0;
    }
    if size == state.eos_offset {
        return;
    }
    let Ok(mut f) = File::open(&path) else {
        return;
    };
    if f.seek(SeekFrom::Start(state.eos_offset)).is_err() {
        return;
    }
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_err() {
        return;
    }
    state.eos_offset = size;
    let Ok(conn) = open_db() else {
        return;
    };
    for line in buf.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let exe = row["exe"].as_str().unwrap_or("").to_string();
        let ids: Vec<String> = row["ids"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if exe.is_empty() || ids.is_empty() {
            continue;
        }
        let base = Path::new(&exe)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let game = find_game_for_process(&conn, "", &exe)
            .ok()
            .filter(|g| !g.is_null())
            .or_else(|| {
                find_game_for_process(&conn, &exe, &base)
                    .ok()
                    .filter(|g| !g.is_null())
            });
        let Some(game) = game else {
            continue;
        };
        let Some(gid) = game["id"].as_str() else {
            continue;
        };
        let defs = list_for_game(&conn, gid).unwrap_or(Value::Array(vec![]));
        let defs = defs.as_array().cloned().unwrap_or_default();
        let gname = game["name"].as_str().unwrap_or("Game");
        for ach_id in ids {
            let def = defs
                .iter()
                .find(|d| d["achievement_id"].as_str() == Some(&ach_id));
            let title = def.and_then(|d| d["title"].as_str()).unwrap_or(&ach_id);
            let icon = def.and_then(|d| d["icon_unlocked"].as_str());
            let desc = def.and_then(|d| d["description"].as_str());
            if let Ok(true) = record_unlock(&conn, gid, &ach_id, title, desc, icon, None) {
                emit_unlock(gname, title, icon);
            }
        }
    }
}

fn map_remedy_ach_id(game_folder: &str, id: u32) -> String {
    if game_folder.eq_ignore_ascii_case("alanwake2") {
        if (201..=210).contains(&id) {
            return (id - 122).to_string();
        }
        if (101..=112).contains(&id) {
            return (id - 34).to_string();
        }
    }
    id.to_string()
}

fn parse_remedy_chunk(file: &Path, state: &mut WatchState) {
    let Ok(buf) = fs::read(file) else {
        return;
    };
    if buf.len() < 8 {
        return;
    }
    let count = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if count == 0 || count > 512 || buf.len() < 8 + count * 16 {
        return;
    }
    let parts: Vec<&str> = file.to_str().unwrap_or("").split(['/', '\\']).collect();
    let ach_idx = parts
        .iter()
        .position(|p| p.eq_ignore_ascii_case("achievements"));
    let game_folder = ach_idx
        .and_then(|i| i.checked_sub(2).and_then(|j| parts.get(j).copied()))
        .unwrap_or("");

    let key = file.to_string_lossy().into_owned();
    let seen = state.remedy_seen.entry(key).or_default();
    let mut fresh = Vec::new();
    for i in 0..count {
        let off = 8 + i * 16;
        let id = u32::from_le_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
        let status = u32::from_le_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]]);
        if status == 0 {
            continue;
        }
        let ach_id = map_remedy_ach_id(game_folder, id);
        if seen.insert(ach_id.clone()) {
            fresh.push(ach_id);
        }
    }
    if fresh.is_empty() {
        return;
    }
    let process_guess = if game_folder.eq_ignore_ascii_case("alanwake2") {
        "AlanWake2.exe".into()
    } else {
        format!("{game_folder}.exe")
    };
    let Ok(conn) = open_db() else {
        return;
    };
    let Ok(game) = find_game_for_process(&conn, "", &process_guess) else {
        return;
    };
    if game.is_null() {
        return;
    }
    let Some(gid) = game["id"].as_str() else {
        return;
    };
    let defs = list_for_game(&conn, gid).unwrap_or(Value::Array(vec![]));
    let defs = defs.as_array().cloned().unwrap_or_default();
    let gname = game["name"].as_str().unwrap_or("Game");
    for ach_id in fresh {
        let def = defs
            .iter()
            .find(|d| d["achievement_id"].as_str() == Some(&ach_id));
        let title = def.and_then(|d| d["title"].as_str()).unwrap_or(&ach_id);
        let icon = def.and_then(|d| d["icon_unlocked"].as_str());
        let desc = def.and_then(|d| d["description"].as_str());
        if let Ok(true) = record_unlock(&conn, gid, &ach_id, title, desc, icon, None) {
            emit_unlock(gname, title, icon);
        }
    }
}

fn scan_remedy(state: &mut WatchState) {
    let root = remedy_dir();
    if !root.is_dir() {
        return;
    }
    fn walk(dir: &Path, depth: usize, state: &mut WatchState) {
        if depth > 5 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for ent in entries.flatten() {
            let full = ent.path();
            if full.is_dir() {
                walk(&full, depth + 1, state);
                continue;
            }
            if !ent
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("data.chunk")
            {
                continue;
            }
            if !dir
                .file_name()
                .map(|n| n.to_string_lossy().eq_ignore_ascii_case("achievements"))
                .unwrap_or(false)
            {
                continue;
            }
            if should_scan(&full, state) {
                parse_remedy_chunk(&full, state);
            }
        }
    }
    walk(&root, 0, state);
}

fn scan_all(remedy: bool) {
    let Ok(mut guard) = WATCH.lock() else {
        return;
    };
    let state = watch_state(&mut guard);
    scan_gse(state);
    scan_epic(state);
    scan_eos_hooks(state);
    if remedy {
        scan_remedy(state);
    }
}

/// Start the unlock poll pump (idempotent). Call after [`install_toast_sender`].
pub fn spawn_watchers() {
    {
        let Ok(mut guard) = WATCH.lock() else {
            return;
        };
        let state = watch_state(&mut guard);
        if state.started {
            return;
        }
        state.started = true;
        // Skip historical EOS hook lines (Electron parity).
        state.eos_offset = eos_hooks_path().metadata().map(|m| m.len()).unwrap_or(0);
    }
    scan_all(true);
    tokio::spawn(async move {
        let mut ticks: u64 = 0;
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            ticks += 1;
            // Remedy every ~10s (Electron interval).
            scan_all(ticks % 5 == 0);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sync_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE tracked_games (
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL,
               platform TEXT NOT NULL,
               exe_path TEXT,
               process_name TEXT,
               source TEXT NOT NULL,
               updated_at INTEGER NOT NULL,
               deleted_at INTEGER,
               rev INTEGER NOT NULL DEFAULT 1
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn mapped_appid_uses_tracked_name() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES ('library_by_appid:480', 'custom:game')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracked_games (id, name, platform, source, updated_at, rev)
             VALUES ('custom:game', 'Half-Life 2', 'steam', 'library', 1, 1)",
            [],
        )
        .unwrap();
        assert_eq!(
            resolve_gse_unlock_target(&conn, "480"),
            ("custom:game".into(), "Half-Life 2".into())
        );
    }

    #[test]
    fn mapped_appid_without_tracked_row_uses_library_id() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES ('library_by_appid:480', 'custom:game')",
            [],
        )
        .unwrap();
        assert_eq!(
            resolve_gse_unlock_target(&conn, "480"),
            ("custom:game".into(), "custom:game".into())
        );
    }

    #[test]
    fn unmapped_appid_falls_back_to_gse() {
        let conn = mem_db();
        assert_eq!(
            resolve_gse_unlock_target(&conn, "480"),
            ("gse:480".into(), "Steam 480".into())
        );
    }

    #[test]
    fn gse_toast_uses_schema_title_and_cdn_icon() {
        let defs = vec![json!({
            "achievement_id": "NoviceAdventurer",
            "title": "Novice Adventurer",
            "icon_unlocked": "https://cdn.example/icon.jpg",
        })];
        let (title, icon) = toast_from_def(&defs, "NoviceAdventurer", Some("NoviceAdventurer"));
        assert_eq!(title, "Novice Adventurer");
        assert_eq!(icon.as_deref(), Some("https://cdn.example/icon.jpg"));
    }

    #[test]
    fn gse_toast_without_def_has_no_icon() {
        let (title, icon) = toast_from_def(&[], "ACH_X", Some("Saved"));
        assert_eq!(title, "Saved");
        assert_eq!(icon, None);
    }
}
