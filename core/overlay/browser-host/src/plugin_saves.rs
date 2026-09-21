//! Save Manager `game.saves.*` — Electron `SaveManifestService` / `packages/save-manifest` parity.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

use crate::apps;

const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const MIN_FUZZY_LEN: usize = 5;

static STORE: Mutex<Option<StoreState>> = Mutex::new(None);

struct StoreState {
    cache_dir: PathBuf,
    db_path: PathBuf,
    conn: Connection,
    game_dir: String,
    exe_path: String,
    exe_name: String,
    /// `None` = unresolved; `Some(None)` = no match; `Some(Some(title))` = matched.
    manifest_key: Option<Option<String>>,
    placeholder: Option<PlaceholderContext>,
}

#[derive(Clone)]
struct PlaceholderContext {
    home: String,
    win_local_app_data: String,
    win_app_data: String,
    win_documents: String,
    store_game_dir: String,
    store_user_dir: String,
    store_user_id: String,
    root: String,
    steam_id: Option<String>,
}

/// Dispatch `game.saves.*` (permission already gated).
///
/// May rebuild the Ludusavi DB (YAML parse + ~50k inserts). CEF BridgeInvoke
/// must call [`dispatch_async`] so that work runs on `spawn_blocking`.
pub fn dispatch(method: &str, args_json: &str) -> Result<String, String> {
    match method {
        "game.saves.getGameDir" => with_store(|s| Ok(json!(s.game_dir).to_string())),
        "game.saves.getManifestEntry" => with_store(|s| {
            Ok(match get_manifest_entry(s)? {
                Some(v) => v.to_string(),
                None => "null".into(),
            })
        }),
        "game.saves.getSaveLocations" => with_store(|s| Ok(get_save_locations(s)?.to_string())),
        "game.saves.findGame" => {
            let args: Vec<Value> = serde_json::from_str(args_json)
                .map_err(|e| format!("invalid findGame args: {e}"))?;
            let query = match args.first() {
                Some(Value::String(s)) => s.clone(),
                Some(v) => {
                    return Err(format!("findGame query must be string, got {v}"));
                }
                None => String::new(),
            };
            with_store(|s| Ok(find_game(s, &query)?.to_string()))
        }
        "game.saves.updateManifest" => Ok(update_manifest()?.to_string()),
        _ => Err(format!("unknown saves method: {method}")),
    }
}

/// Offload saves dispatch (incl. Ludusavi rebuild) off the CEF/tokio UI loop.
pub async fn dispatch_async(method: &str, args_json: &str) -> Result<String, String> {
    let method = method.to_string();
    let args_json = args_json.to_string();
    tokio::task::spawn_blocking(move || dispatch(&method, &args_json))
        .await
        .map_err(|e| format!("saves worker join: {e}"))?
}

/// Game dir + resolved save location prefixes for `build_allowed_paths` (Electron parity).
///
/// Opens an existing `manifest.db` only — never triggers a Ludusavi rebuild
/// (rebuild belongs on the async `game.saves.*` path).
pub fn allowlist_roots(plugin_id: &str, access: &apps::AppAccess) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let is_save_manager = plugin_id == "save-manager";
    let has_game = access.permissions.iter().any(|p| p == "game:process") || is_save_manager;
    let has_fs = access
        .permissions
        .iter()
        .any(|p| p == "fs:read" || p == "fs:write")
        || is_save_manager;

    let _ = with_existing_store(|s| {
        if has_game && !s.game_dir.is_empty() {
            roots.push(PathBuf::from(&s.game_dir));
        }
        if has_fs {
            if let Ok(locs) = get_save_locations(s) {
                if let Some(arr) = locs.as_array() {
                    for loc in arr {
                        if let Some(p) = loc.get("path").and_then(Value::as_str) {
                            let pb = PathBuf::from(p);
                            if let Some(parent) = pb.parent() {
                                roots.push(parent.to_path_buf());
                            }
                            roots.push(pb);
                        }
                    }
                }
            }
        }
        Ok(())
    });
    roots
}

fn with_store<T>(f: impl FnOnce(&mut StoreState) -> Result<T, String>) -> Result<T, String> {
    let mut guard = STORE
        .lock()
        .map_err(|_| "save manifest store lock poisoned".to_string())?;
    open_into(&mut guard)?;
    let state = guard.as_mut().ok_or("save store missing")?;
    f(state)
}

fn with_existing_store<T>(
    f: impl FnOnce(&mut StoreState) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = STORE
        .lock()
        .map_err(|_| "save manifest store lock poisoned".to_string())?;
    open_existing_into(&mut guard)?;
    let state = guard.as_mut().ok_or("save store missing")?;
    f(state)
}

fn open_into(guard: &mut Option<StoreState>) -> Result<(), String> {
    let (game_dir, exe_path, exe_name) = attached_game_context();
    let cache = cache_dir();
    let (_, _, meta_path) = cache_paths();
    let needs_open = match guard.as_ref() {
        None => true,
        Some(s) => {
            s.cache_dir != cache
                || s.game_dir != game_dir
                || s.exe_path != exe_path
                || !s.db_path.is_file()
                || !read_meta_fresh(&meta_path)
        }
    };
    if needs_open {
        // Drop any open read-only handle before rebuild may replace the file.
        *guard = None;
        let db_path = ensure_database(false)?;
        let conn = open_connection(&db_path)?;
        *guard = Some(StoreState {
            cache_dir: cache,
            db_path,
            conn,
            game_dir,
            exe_path,
            exe_name,
            manifest_key: None,
            placeholder: None,
        });
    }
    Ok(())
}

/// Open on-disk DB if present; never fetch/rebuild (fs allowlist path).
fn open_existing_into(guard: &mut Option<StoreState>) -> Result<(), String> {
    if guard.is_some() {
        return Ok(());
    }
    migrate_legacy_cache();
    let (_, db_path, _) = cache_paths();
    if !db_path.is_file() {
        return Ok(());
    }
    let (game_dir, exe_path, exe_name) = attached_game_context();
    let conn = open_connection(&db_path)?;
    *guard = Some(StoreState {
        cache_dir: cache_dir(),
        db_path,
        conn,
        game_dir,
        exe_path,
        exe_name,
        manifest_key: None,
        placeholder: None,
    });
    Ok(())
}

fn cache_dir() -> PathBuf {
    apps::apps_root().join("save-manager").join("data")
}

fn legacy_cache_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join("Glint").join("save-manifest"))
        .unwrap_or_else(|| PathBuf::from("save-manifest"))
}

fn cache_paths() -> (PathBuf, PathBuf, PathBuf) {
    let dir = cache_dir();
    (
        dir.join("manifest.yaml"),
        dir.join("manifest.db"),
        dir.join("manifest.meta.json"),
    )
}

fn migrate_legacy_cache() {
    let legacy = legacy_cache_dir();
    let next = cache_dir();
    let names = ["manifest.db", "manifest.yaml", "manifest.meta.json"];
    if !names.iter().any(|n| legacy.join(n).is_file()) {
        return;
    }
    let _ = std::fs::create_dir_all(&next);
    for name in names {
        let from = legacy.join(name);
        let to = next.join(name);
        if from.is_file() && !to.exists() {
            if std::fs::rename(&from, &to).is_err() {
                let _ = std::fs::copy(&from, &to);
            }
        }
    }
}

fn read_meta_fresh(meta_path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(meta_path) else {
        return false;
    };
    let Ok(meta) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let Some(downloaded) = meta.get("downloadedAt").and_then(Value::as_str) else {
        return false;
    };
    let Ok(parsed) = chrono_lite_parse(downloaded) else {
        return false;
    };
    SystemTime::now()
        .duration_since(parsed)
        .map(|age| age < CACHE_MAX_AGE)
        .unwrap_or(false)
}

fn chrono_lite_parse(s: &str) -> Result<SystemTime, ()> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T').ok_or(())?;
    let mut dp = date.split('-');
    let y: i64 = dp.next().ok_or(())?.parse().map_err(|_| ())?;
    let mo: u32 = dp.next().ok_or(())?.parse().map_err(|_| ())?;
    let d: u32 = dp.next().ok_or(())?.parse().map_err(|_| ())?;
    let time = time.split('.').next().unwrap_or(time);
    let mut tp = time.split(':');
    let h: u32 = tp.next().ok_or(())?.parse().map_err(|_| ())?;
    let mi: u32 = tp.next().ok_or(())?.parse().map_err(|_| ())?;
    let se: u32 = tp.next().ok_or(())?.parse().map_err(|_| ())?;
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe as i64 - 719468;
    let secs = days * 86400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(se);
    if secs < 0 {
        return Err(());
    }
    Ok(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

fn write_meta(meta_path: &Path) -> Result<(), String> {
    if let Some(parent) = meta_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("manifest meta mkdir: {e}"))?;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let rem = now % 86400;
    let (y, mo, d) = civil_from_days(days as i64);
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let se = rem % 60;
    let iso = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}.000Z");
    let body = json!({ "downloadedAt": iso });
    std::fs::write(meta_path, body.to_string()).map_err(|e| format!("manifest meta write: {e}"))
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn find_fixture_path() -> Result<PathBuf, String> {
    if let Ok(root) = std::env::var("GLINT_REPO_ROOT") {
        let p = PathBuf::from(root).join("data/save-manifest/fixture.yaml");
        if p.is_file() {
            return Ok(p);
        }
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..6 {
        let candidate = dir.join("data/save-manifest/fixture.yaml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    Err("Could not find data/save-manifest/fixture.yaml".into())
}

fn fetch_manifest_yaml() -> Result<String, String> {
    // Tests / offline: skip network (Electron falls back to fixture on fetch failure).
    if std::env::var_os("GLINT_SAVE_MANIFEST_OFFLINE").is_some() {
        return Err("offline".into());
    }
    let resp = ureq::get(MANIFEST_URL)
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|e| format!("Failed to fetch manifest: {e}"))?;
    if !(200..300).contains(&resp.status()) {
        return Err(format!(
            "Failed to fetch manifest: {} {}",
            resp.status(),
            resp.status_text()
        ));
    }
    resp.into_string()
        .map_err(|e| format!("Failed to read manifest body: {e}"))
}

fn ensure_yaml_on_disk(force_network: bool) -> Result<PathBuf, String> {
    let (manifest_path, _, meta_path) = cache_paths();
    if !force_network && read_meta_fresh(&meta_path) && manifest_path.is_file() {
        return Ok(manifest_path);
    }
    match fetch_manifest_yaml() {
        Ok(content) => {
            if let Some(parent) = manifest_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&manifest_path, content).map_err(|e| e.to_string())?;
            write_meta(&meta_path)?;
            Ok(manifest_path)
        }
        Err(_) => {
            // Prefer existing cached yaml over wiping with fixture when offline.
            if manifest_path.is_file() {
                Ok(manifest_path)
            } else {
                find_fixture_path()
            }
        }
    }
}

#[derive(Deserialize)]
struct RawFileMeta {
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawSteam {
    id: Option<i64>,
}

#[derive(Deserialize)]
struct RawManifestEntry {
    files: Option<HashMap<String, RawFileMeta>>,
    steam: Option<RawSteam>,
    notes: Option<Vec<String>>,
    alias: Option<String>,
}

fn normalize_key(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn build_db_from_yaml(yaml_path: &Path, db_path: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(yaml_path).map_err(|e| format!("read yaml: {e}"))?;
    let parsed: HashMap<String, RawManifestEntry> =
        serde_yaml::from_str(&raw).map_err(|e| format!("parse yaml: {e}"))?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if db_path.exists() {
        let _ = std::fs::remove_file(db_path);
    }

    let conn = Connection::open(db_path).map_err(|e| format!("open db: {e}"))?;
    conn.execute_batch(
        "
        PRAGMA journal_mode = OFF;
        PRAGMA synchronous = OFF;
        CREATE TABLE games (
          title TEXT PRIMARY KEY,
          steam_id INTEGER,
          notes TEXT
        );
        CREATE INDEX idx_games_steam_id ON games(steam_id) WHERE steam_id IS NOT NULL;
        CREATE TABLE game_aliases (
          alias_title TEXT PRIMARY KEY,
          target_title TEXT NOT NULL
        );
        CREATE TABLE game_files (
          game_title TEXT NOT NULL,
          path_template TEXT NOT NULL,
          tags TEXT NOT NULL DEFAULT '[\"save\"]'
        );
        CREATE INDEX idx_game_files_game ON game_files(game_title);
        CREATE TABLE game_lookup (
          game_title TEXT NOT NULL,
          norm TEXT NOT NULL,
          kind TEXT NOT NULL
        );
        CREATE INDEX idx_game_lookup_norm_kind ON game_lookup(norm, kind);
        ",
    )
    .map_err(|e| format!("create schema: {e}"))?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("begin: {e}"))?;
    {
        let mut insert_game = tx
            .prepare("INSERT INTO games (title, steam_id, notes) VALUES (?, ?, ?)")
            .map_err(|e| e.to_string())?;
        let mut insert_alias = tx
            .prepare("INSERT INTO game_aliases (alias_title, target_title) VALUES (?, ?)")
            .map_err(|e| e.to_string())?;
        let mut insert_file = tx
            .prepare("INSERT INTO game_files (game_title, path_template, tags) VALUES (?, ?, ?)")
            .map_err(|e| e.to_string())?;
        let mut insert_lookup = tx
            .prepare("INSERT INTO game_lookup (game_title, norm, kind) VALUES (?, ?, ?)")
            .map_err(|e| e.to_string())?;

        for (title, entry) in &parsed {
            let steam_id = entry.steam.as_ref().and_then(|s| s.id);
            let notes = entry
                .notes
                .as_ref()
                .map(|n| serde_json::to_string(n).unwrap_or_else(|_| "[]".into()));
            let has_files = entry.files.as_ref().map(|f| !f.is_empty()).unwrap_or(false);

            if let Some(alias) = &entry.alias {
                insert_alias
                    .execute(params![title, alias])
                    .map_err(|e| e.to_string())?;
                insert_game
                    .execute(params![title, steam_id, notes])
                    .map_err(|e| e.to_string())?;
                insert_lookup
                    .execute(params![alias, normalize_key(title), "title"])
                    .map_err(|e| e.to_string())?;
                if !has_files {
                    continue;
                }
            } else {
                insert_game
                    .execute(params![title, steam_id, notes])
                    .map_err(|e| e.to_string())?;
                insert_lookup
                    .execute(params![title, normalize_key(title), "title"])
                    .map_err(|e| e.to_string())?;
            }

            if let Some(files) = &entry.files {
                for (file_path, meta) in files {
                    let tags = match &meta.tags {
                        Some(t) if !t.is_empty() => t.clone(),
                        _ => vec!["save".into()],
                    };
                    let tags_json =
                        serde_json::to_string(&tags).unwrap_or_else(|_| "[\"save\"]".into());
                    insert_file
                        .execute(params![title, file_path, tags_json])
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    tx.commit().map_err(|e| format!("commit: {e}"))?;
    Ok(())
}

fn rebuild_database(yaml_path: &Path) -> Result<(), String> {
    let (_, db_path, _) = cache_paths();
    let temp_path = PathBuf::from(format!("{}.tmp", db_path.display()));
    build_db_from_yaml(yaml_path, &temp_path)?;
    let _ = std::fs::remove_file(&db_path);
    std::fs::rename(&temp_path, &db_path).map_err(|e| format!("rename manifest.db: {e}"))?;
    Ok(())
}

fn open_connection(db_path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open manifest.db: {e}"))
}

fn ensure_database(force_network: bool) -> Result<PathBuf, String> {
    migrate_legacy_cache();
    let (_, db_path, meta_path) = cache_paths();

    if force_network {
        let yaml_path = ensure_yaml_on_disk(true)?;
        rebuild_database(&yaml_path)?;
        write_meta(&meta_path)?;
        return Ok(db_path);
    }

    // Electron ManifestStore.openOnce: reuse only when meta is fresh AND db exists.
    if read_meta_fresh(&meta_path) && db_path.is_file() {
        return Ok(db_path);
    }

    let yaml_path = ensure_yaml_on_disk(false)?;
    rebuild_database(&yaml_path)?;
    if !read_meta_fresh(&meta_path) {
        write_meta(&meta_path)?;
    }
    Ok(db_path)
}

fn attached_game_context() -> (String, String, String) {
    let pid = std::env::var("GLINT_GAME_PID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    let Some(pid) = pid else {
        return (String::new(), String::new(), String::new());
    };
    let exe_path = resolve_process_exe_path(pid).unwrap_or_default();
    if exe_path.is_empty() {
        return (String::new(), String::new(), String::new());
    }
    let game_dir = Path::new(&exe_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let exe_name = Path::new(&exe_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    (game_dir, exe_path, exe_name)
}

fn resolve_process_exe_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

fn read_steam_app_id(game_dir: &str) -> Option<i64> {
    let raw = std::fs::read_to_string(Path::new(game_dir).join("steam_appid.txt")).ok()?;
    raw.trim().parse().ok()
}

fn detect_steam_root(game_dir: &str) -> String {
    let normalized = game_dir.replace('\\', "/");
    let marker = "/steamapps/common/";
    if let Some(idx) = normalized.to_lowercase().find(marker) {
        return game_dir[..idx].to_string();
    }
    game_dir.to_string()
}

fn detect_store_user_id(steam_root: &str, app_id: Option<i64>) -> String {
    let Some(app_id) = app_id else {
        return String::new();
    };
    let userdata = Path::new(steam_root).join("userdata");
    let Ok(users) = std::fs::read_dir(&userdata) else {
        return String::new();
    };
    for entry in users.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let candidate = userdata.join(&name).join(app_id.to_string());
        if candidate.exists() {
            return name;
        }
    }
    String::new()
}

fn build_placeholder(
    game_dir: &str,
    steam_root: &str,
    store_user_id: &str,
    steam_id: Option<i64>,
) -> PlaceholderContext {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PlaceholderContext {
        home: home.clone(),
        win_local_app_data: std::env::var("LOCALAPPDATA")
            .unwrap_or_else(|_| Path::new(&home).join("AppData/Local").display().to_string()),
        win_app_data: std::env::var("APPDATA").unwrap_or_else(|_| {
            Path::new(&home)
                .join("AppData/Roaming")
                .display()
                .to_string()
        }),
        win_documents: std::env::var("USERPROFILE")
            .map(|p| Path::new(&p).join("Documents").display().to_string())
            .unwrap_or_else(|_| Path::new(&home).join("Documents").display().to_string()),
        store_game_dir: game_dir.to_string(),
        store_user_dir: game_dir.to_string(),
        store_user_id: store_user_id.to_string(),
        root: steam_root.to_string(),
        steam_id: steam_id.map(|id| id.to_string()),
    }
}

fn resolve_placeholders(path: &str, ctx: &PlaceholderContext) -> String {
    let mut out = path.replace('\\', "/");
    let pairs: [(&str, &str); 8] = [
        ("<home>", &ctx.home),
        ("<winLocalAppData>", &ctx.win_local_app_data),
        ("<winAppData>", &ctx.win_app_data),
        ("<winDocuments>", &ctx.win_documents),
        ("<storeGameDir>", &ctx.store_game_dir),
        ("<storeUserDir>", &ctx.store_user_dir),
        ("<storeUserId>", &ctx.store_user_id),
        ("<root>", &ctx.root),
    ];
    for (token, value) in pairs {
        let value = value.replace('\\', "/");
        out = out.replace(token, &value);
    }
    if let Some(steam_id) = &ctx.steam_id {
        out = out.replace("<steamId>", steam_id);
    }
    out.replace('/', "\\")
}

fn expand_path_templates(raw_path: &str, ctx: &PlaceholderContext) -> Vec<String> {
    if !raw_path.contains("<storeUserId>") {
        return vec![resolve_placeholders(raw_path, ctx)];
    }
    let mut paths = HashSet::new();
    if !ctx.store_user_id.is_empty() {
        paths.insert(resolve_placeholders(raw_path, ctx));
    }
    let parent_template = raw_path
        .split("<storeUserId>")
        .next()
        .unwrap_or("")
        .trim_end_matches(['/', '\\']);
    let parent = resolve_placeholders(parent_template, ctx);
    if let Ok(entries) = std::fs::read_dir(&parent) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let mut nested = ctx.clone();
                nested.store_user_id = entry.file_name().to_string_lossy().to_string();
                paths.insert(resolve_placeholders(raw_path, &nested));
            }
        }
    }
    if paths.is_empty() {
        paths.insert(resolve_placeholders(raw_path, ctx));
    }
    paths.into_iter().collect()
}

fn ensure_context(state: &mut StoreState) -> Result<(), String> {
    if state.game_dir.is_empty() {
        return Ok(());
    }
    if state.manifest_key.is_some() {
        return Ok(());
    }
    let steam_root = detect_steam_root(&state.game_dir);
    let steam_id = read_steam_app_id(&state.game_dir);
    let store_user_id = detect_store_user_id(&steam_root, steam_id);
    state.placeholder = Some(build_placeholder(
        &state.game_dir,
        &steam_root,
        &store_user_id,
        steam_id,
    ));
    state.manifest_key = Some(match_game(&state.conn, &state.game_dir, &state.exe_name)?);
    Ok(())
}

fn match_game(conn: &Connection, game_dir: &str, exe_name: &str) -> Result<Option<String>, String> {
    if let Some(steam_app_id) = read_steam_app_id(game_dir) {
        let mut stmt = conn
            .prepare("SELECT title FROM games WHERE steam_id = ?")
            .map_err(|e| e.to_string())?;
        if let Some(title) = stmt
            .query_row(params![steam_app_id], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
        {
            return Ok(Some(title));
        }
    }

    let folder_name = Path::new(game_dir)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let folder_norm = normalize_key(folder_name);
    {
        let mut stmt = conn
            .prepare("SELECT game_title FROM game_lookup WHERE norm = ? AND kind = 'title' LIMIT 1")
            .map_err(|e| e.to_string())?;
        if let Some(title) = stmt
            .query_row(params![folder_norm], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
        {
            return Ok(Some(title));
        }
    }

    let exe_stem = Path::new(exe_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(exe_name);
    let needles: Vec<String> = [normalize_key(exe_stem), folder_norm]
        .into_iter()
        .filter(|n| n.len() >= MIN_FUZZY_LEN)
        .collect();

    for needle in needles {
        let mut stmt = conn
            .prepare(
                "SELECT game_title FROM game_lookup
                 WHERE kind = 'title' AND length(norm) >= ? AND (
                   norm LIKE '%' || ? || '%' OR ? LIKE '%' || norm || '%'
                 )
                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;
        if let Some(title) = stmt
            .query_row(params![MIN_FUZZY_LEN as i64, needle, needle], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
        {
            return Ok(Some(title));
        }
    }
    Ok(None)
}

fn resolve_canonical_title(conn: &Connection, title: &str) -> Result<String, String> {
    let mut current = title.to_string();
    for _ in 0..4 {
        let mut stmt = conn
            .prepare("SELECT target_title FROM game_aliases WHERE alias_title = ?")
            .map_err(|e| e.to_string())?;
        let next = stmt
            .query_row(params![current], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?;
        match next {
            Some(t) => current = t,
            None => return Ok(current),
        }
    }
    Ok(current)
}

fn get_game(conn: &Connection, title: &str) -> Result<Option<Value>, String> {
    let canonical = resolve_canonical_title(conn, title)?;
    let mut stmt = conn
        .prepare("SELECT title, steam_id, notes FROM games WHERE title = ?")
        .map_err(|e| e.to_string())?;
    let row = stmt
        .query_row(params![canonical], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((title, steam_id, notes)) = row else {
        return Ok(None);
    };

    let mut files_stmt = conn
        .prepare("SELECT path_template, tags FROM game_files WHERE game_title = ? ORDER BY rowid")
        .map_err(|e| e.to_string())?;
    let file_rows = files_stmt
        .query_map(params![canonical], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;

    let mut files = serde_json::Map::new();
    for row in file_rows {
        let (path_template, tags_json) = row.map_err(|e| e.to_string())?;
        let tags: Value = serde_json::from_str(&tags_json).unwrap_or_else(|_| json!(["save"]));
        files.insert(path_template, json!({ "tags": tags }));
    }

    let mut entry = json!({
        "title": title,
        "files": files,
    });
    if let Some(id) = steam_id {
        entry
            .as_object_mut()
            .unwrap()
            .insert("steamId".into(), json!(id));
    }
    if let Some(notes_raw) = notes {
        if let Ok(notes_val) = serde_json::from_str::<Value>(&notes_raw) {
            entry
                .as_object_mut()
                .unwrap()
                .insert("notes".into(), notes_val);
        }
    }
    Ok(Some(entry))
}

fn get_manifest_entry(state: &mut StoreState) -> Result<Option<Value>, String> {
    ensure_context(state)?;
    let key = match &state.manifest_key {
        Some(Some(k)) => k.clone(),
        _ => return Ok(None),
    };
    get_game(&state.conn, &key)
}

fn normalize_tags(tags: &[Value]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for t in tags {
        match t.as_str() {
            Some("save") => out.push("save"),
            Some("config") => out.push("config"),
            Some("settings") => out.push("settings"),
            _ => {}
        }
    }
    if out.is_empty() {
        out.push("save");
    }
    out
}

fn get_save_locations(state: &mut StoreState) -> Result<Value, String> {
    let entry = match get_manifest_entry(state)? {
        Some(e) => e,
        None => return Ok(json!([])),
    };
    let Some(ctx) = state.placeholder.clone() else {
        return Ok(json!([]));
    };
    let Some(files) = entry.get("files").and_then(Value::as_object) else {
        return Ok(json!([]));
    };

    let mut locations = Vec::new();
    for (raw_path, meta) in files {
        let tags_val = meta
            .get("tags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![json!("save")]);
        let tags = normalize_tags(&tags_val);
        let resolved = expand_path_templates(raw_path, &ctx);
        let filtered: Vec<String> = if tags.contains(&"save") {
            resolved
                .iter()
                .filter(|c| {
                    Path::new(c)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|b| b.chars().all(|ch| ch.is_ascii_digit()))
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let paths_to_use = if filtered.is_empty() {
            resolved
        } else {
            filtered
        };
        for resolved_path in paths_to_use {
            let exists = Path::new(&resolved_path).exists();
            locations.push(json!({
                "path": resolved_path,
                "tags": tags,
                "exists": exists,
            }));
        }
    }
    Ok(Value::Array(locations))
}

fn find_game(state: &StoreState, query: &str) -> Result<Value, String> {
    let needle = format!("%{}%", query.to_lowercase());
    let mut stmt = state
        .conn
        .prepare(
            "SELECT title FROM games
             WHERE lower(title) LIKE ?
             ORDER BY title
             LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let titles: Vec<String> = stmt
        .query_map(params![needle, 50i64], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    let mut out = Vec::new();
    for title in titles {
        if let Some(entry) = get_game(&state.conn, &title)? {
            out.push(entry);
        }
    }
    Ok(Value::Array(out))
}

fn count_games(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT COUNT(*) FROM games", [], |r| r.get(0))
        .map_err(|e| e.to_string())
}

fn update_manifest() -> Result<Value, String> {
    {
        let mut guard = STORE
            .lock()
            .map_err(|_| "save manifest store lock poisoned".to_string())?;
        *guard = None;
    }
    let db_path = ensure_database(true)?;
    let (game_dir, exe_path, exe_name) = attached_game_context();
    let conn = open_connection(&db_path)?;
    let game_count = count_games(&conn)?;
    let mut guard = STORE
        .lock()
        .map_err(|_| "save manifest store lock poisoned".to_string())?;
    *guard = Some(StoreState {
        cache_dir: cache_dir(),
        db_path,
        conn,
        game_dir,
        exe_path,
        exe_name,
        manifest_key: None,
        placeholder: None,
    });
    if let Some(state) = guard.as_mut() {
        ensure_context(state)?;
    }
    Ok(json!({ "updated": true, "gameCount": game_count }))
}
