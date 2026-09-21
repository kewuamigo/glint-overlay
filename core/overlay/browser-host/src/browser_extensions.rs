//! Unpacked browser-extension sideload prefs + disk list (`__goHost`).
//!
//! Folders: `%APPDATA%/Glint/BrowserExtensions/<id>/manifest.json`
//! Prefs:   `%APPDATA%/Glint/browser-extensions.json` — `{ "<id>": false }` disables
//! (absent / true = enabled). CEF reads the same prefs on next helper start.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tracing::warn;

/// Disk folders under BrowserExtensions that have `manifest.json`, plus enabled flag.
pub fn list() -> Vec<Value> {
    list_at(&extensions_root(), &disabled_ids())
}

/// Update prefs for `id`. Returns `{ "restartRequired": bool }`.
/// `restartRequired` is true when the enablement bit actually changed (CEF restart
/// needed to apply `--load-extension`).
pub fn set_enabled(args_json: &str) -> Result<String, String> {
    let (id, enabled): (String, bool) = serde_json::from_str(args_json)
        .map_err(|err| format!("browser.extensions.setEnabled needs [id, enabled]: {err}"))?;
    if id.is_empty() {
        return Err("browser.extensions.setEnabled: empty id".into());
    }
    set_enabled_at(&prefs_path(), &id, enabled)
}

/// Parse `browser.extensions.openOptions` / `openPopup` args `[id]`.
pub fn parse_open_satellite_id(method: &str, args_json: &str) -> Result<String, String> {
    let (id,): (String,) = serde_json::from_str(args_json)
        .map_err(|err| format!("{method} needs [id]: {err}"))?;
    if id.is_empty() {
        return Err(format!("{method}: empty id"));
    }
    Ok(id)
}

/// `browser.extensions.closeSatellite` takes no args (`[]` or omitted object).
pub fn parse_close_satellite(args_json: &str) -> Result<(), String> {
    let trimmed = args_json.trim();
    if trimmed.is_empty() || trimmed == "[]" || trimmed == "null" {
        return Ok(());
    }
    let _: Vec<Value> = serde_json::from_str(args_json)
        .map_err(|err| format!("browser.extensions.closeSatellite needs []: {err}"))?;
    Ok(())
}

fn list_at(root: &Path, disabled: &HashSet<String>) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest = dir.join("manifest.json");
        if !manifest.is_file() {
            continue;
        }
        out.push(json!({
            "id": name,
            "enabled": !disabled.contains(&name),
        }));
    }
    out.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["id"].as_str().unwrap_or(""))
    });
    out
}

fn set_enabled_at(path: &Path, id: &str, enabled: bool) -> Result<String, String> {
    let mut map = read_prefs_object(path);
    let was_disabled = map.get(id) == Some(&Value::Bool(false));
    let will_be_disabled = !enabled;
    let changed = was_disabled != will_be_disabled;
    if will_be_disabled {
        map.insert(id.to_string(), Value::Bool(false));
    } else {
        // Absent = enabled; drop explicit true to keep the file sparse.
        map.remove(id);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("prefs dir: {err}"))?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|err| format!("prefs encode: {err}"))?;
    std::fs::write(path, text).map_err(|err| format!("prefs write: {err}"))?;
    Ok(json!({ "restartRequired": changed }).to_string())
}

fn disabled_ids() -> HashSet<String> {
    parse_disabled(&read_prefs_text(&prefs_path()))
}

fn read_prefs_object(path: &Path) -> serde_json::Map<String, Value> {
    match serde_json::from_str::<Value>(&read_prefs_text(path)) {
        Ok(Value::Object(map)) => map,
        Ok(_) => {
            warn!(path = %path.display(), "browser-extensions.json is not an object — resetting");
            serde_json::Map::new()
        }
        Err(_) => serde_json::Map::new(),
    }
}

fn read_prefs_text(path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            warn!(path = %path.display(), %err, "browser-extensions.json unreadable");
            String::new()
        }
    }
}

/// `{ "<id>": false }` → disabled set. Unparseable → empty (all enabled).
fn parse_disabled(text: &str) -> HashSet<String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        if !text.trim().is_empty() {
            warn!("browser-extensions.json is not a JSON object — all enabled");
        }
        return HashSet::new();
    };
    map.into_iter()
        .filter(|(_, v)| v == &Value::Bool(false))
        .map(|(id, _)| id)
        .collect()
}

fn prefs_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("GLINT_BROWSER_EXTENSIONS_PREFS") {
        return PathBuf::from(dir);
    }
    appdata_glint()
        .unwrap_or_default()
        .join("browser-extensions.json")
}

fn extensions_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("GLINT_BROWSER_EXTENSIONS_DIR") {
        return PathBuf::from(dir);
    }
    appdata_glint()
        .unwrap_or_default()
        .join("BrowserExtensions")
}

fn appdata_glint() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|dir| PathBuf::from(dir).join(glint_overlay_common::product::APP_DATA_DIR_NAME))
}

/// Install from a Chrome Web Store detail URL or 32-char extension id.
/// Downloads CRX/zip, unpacks under BrowserExtensions/<id>/, fails closed.
/// Returns `{ "id", "restartRequired": true }` — CEF picks up on next helper start.
pub fn install_from_store(args_json: &str) -> Result<String, String> {
    let (input,): (String,) = serde_json::from_str(args_json).map_err(|err| {
        format!("browser.extensions.installFromStore needs [urlOrId]: {err}")
    })?;
    let id = parse_store_extension_id(input.trim())?;
    install_from_store_at(&id, &extensions_root())
}

fn install_from_store_at(id: &str, root: &Path) -> Result<String, String> {
    let bytes = download_crx(id)?;
    let zip = strip_crx_header(&bytes)?;
    let dest = root.join(id);
    unpack_zip_bytes(&zip, &dest)?;
    if !dest.join("manifest.json").is_file() {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(
            "install failed: unpacked package has no manifest.json (fail closed)".into(),
        );
    }
    Ok(json!({ "id": id, "restartRequired": true }).to_string())
}

/// Accept bare 32-char id (a-p) or CWS detail URL ending in that id.
fn parse_store_extension_id(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("browser.extensions.installFromStore: empty url/id".into());
    }
    if is_chrome_extension_id(trimmed) {
        return Ok(trimmed.to_ascii_lowercase());
    }
    // .../detail/<slug>/<id> or .../detail/<id>
    for part in trimmed.trim_end_matches('/').rsplit('/') {
        if is_chrome_extension_id(part) {
            return Ok(part.to_ascii_lowercase());
        }
    }
    Err(
        "browser.extensions.installFromStore: need a Chrome Web Store URL or 32-char extension id"
            .into(),
    )
}

fn is_chrome_extension_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'a'..=b'p' | b'A'..=b'P'))
}

fn download_crx(id: &str) -> Result<Vec<u8>, String> {
    // Unofficial update2 redirect — fragile; fail closed on any error.
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=131.0.0.0&acceptformat=crx2,crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
    );
    let response = ureq::get(&url)
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
        .call()
        .map_err(|err| format!("store download failed: {err}"))?;
    if !(200..300).contains(&response.status()) {
        return Err(format!(
            "store download failed: HTTP {}",
            response.status()
        ));
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|err| format!("store download read failed: {err}"))?;
    if bytes.len() < 4 {
        return Err("store download failed: empty body".into());
    }
    Ok(bytes)
}

/// CRX2/CRX3 → zip payload; plain PK zip passes through.
fn strip_crx_header(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() >= 2 && data[0] == b'P' && data[1] == b'K' {
        return Ok(data.to_vec());
    }
    if data.len() < 16 || &data[0..4] != b"Cr24" {
        return Err("install failed: response is not a CRX or zip".into());
    }
    let version = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let header_size = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let zip_start = match version {
        2 | 3 => 12usize.saturating_add(header_size),
        _ => return Err(format!("install failed: unsupported CRX version {version}")),
    };
    if zip_start >= data.len() {
        return Err("install failed: truncated CRX".into());
    }
    let zip = &data[zip_start..];
    if zip.len() < 2 || zip[0] != b'P' || zip[1] != b'K' {
        return Err("install failed: CRX payload is not a zip".into());
    }
    Ok(zip.to_vec())
}

fn unpack_zip_bytes(zip: &[u8], dest: &Path) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|err| format!("replace old unpack: {err}"))?;
    }
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|err| format!("extensions root: {err}"))?;
    let staging = parent.join(format!(
        ".glint-unpack-{}",
        dest.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("ext")
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|err| format!("staging dir: {err}"))?;
    let zip_path = staging.join("pkg.zip");
    std::fs::write(&zip_path, zip).map_err(|err| format!("write zip: {err}"))?;
    let extract_dir = staging.join("out");
    std::fs::create_dir_all(&extract_dir).map_err(|err| format!("extract dir: {err}"))?;
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(&zip_path)
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .map_err(|err| format!("tar extract failed: {err}"))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("install failed: could not unpack package (tar)".into());
    }
    // Prefer nested folder that already has manifest.json; else use extract root.
    let source = find_manifest_dir(&extract_dir).unwrap_or(extract_dir.clone());
    if let Err(err) = reject_zip_slip(&source, &extract_dir) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    std::fs::rename(&source, dest).map_err(|err| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("install move failed: {err}")
    })?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

fn find_manifest_dir(root: &Path) -> Option<PathBuf> {
    if root.join("manifest.json").is_file() {
        return Some(root.to_path_buf());
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join("manifest.json").is_file() {
            return Some(p);
        }
    }
    None
}

fn reject_zip_slip(dir: &Path, root: &Path) -> Result<(), String> {
    let root_canon = root
        .canonicalize()
        .map_err(|err| format!("install failed: {err}"))?;
    fn walk(path: &Path, root_canon: &Path) -> Result<(), String> {
        let canon = path
            .canonicalize()
            .map_err(|err| format!("install failed (path check): {err}"))?;
        if !canon.starts_with(root_canon) {
            return Err("install failed: package path escapes destination (fail closed)".into());
        }
        if path.is_dir() {
            for entry in std::fs::read_dir(path)
                .map_err(|err| format!("install failed: {err}"))?
                .flatten()
            {
                walk(&entry.path(), root_canon)?;
            }
        }
        Ok(())
    }
    walk(dir, &root_canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_pair() -> (PathBuf, PathBuf) {
        let n = TEST_SEQ.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("glint-ext-test-{n}"));
        let _ = std::fs::remove_dir_all(&root);
        let ext = root.join("BrowserExtensions");
        let prefs = root.join("browser-extensions.json");
        std::fs::create_dir_all(&ext).expect("ext root");
        (ext, prefs)
    }

    fn write_manifest(dir: &Path) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(dir.join("manifest.json"), r#"{"manifest_version":3,"name":"t"}"#)
            .expect("manifest");
    }

    #[test]
    fn parse_disabled_only_false() {
        let d = parse_disabled(r#"{"a":false,"b":true,"c":false}"#);
        assert!(d.contains("a"));
        assert!(d.contains("c"));
        assert!(!d.contains("b"));
    }

    #[test]
    fn list_and_set_enabled_roundtrip() {
        let (ext, prefs) = temp_pair();
        write_manifest(&ext.join("alpha"));
        write_manifest(&ext.join("beta"));
        // No manifest → ignored.
        std::fs::create_dir_all(ext.join("junk")).expect("junk");

        let listed = list_at(&ext, &HashSet::new());
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["id"], "alpha");
        assert_eq!(listed[0]["enabled"], true);
        assert_eq!(listed[1]["id"], "beta");

        let r1 = set_enabled_at(&prefs, "alpha", false).expect("disable");
        let v1: Value = serde_json::from_str(&r1).unwrap();
        assert_eq!(v1["restartRequired"], true);

        let r2 = set_enabled_at(&prefs, "alpha", false).expect("noop");
        let v2: Value = serde_json::from_str(&r2).unwrap();
        assert_eq!(v2["restartRequired"], false);

        let disabled = parse_disabled(&std::fs::read_to_string(&prefs).unwrap());
        let listed = list_at(&ext, &disabled);
        assert_eq!(listed[0]["id"], "alpha");
        assert_eq!(listed[0]["enabled"], false);
        assert_eq!(listed[1]["enabled"], true);

        let r3 = set_enabled_at(&prefs, "alpha", true).expect("enable");
        let v3: Value = serde_json::from_str(&r3).unwrap();
        assert_eq!(v3["restartRequired"], true);
        let disabled = parse_disabled(&std::fs::read_to_string(&prefs).unwrap());
        assert!(!disabled.contains("alpha"));

        let _ = std::fs::remove_dir_all(ext.parent().unwrap());
    }

    #[test]
    fn open_satellite_args_shape() {
        assert_eq!(
            parse_open_satellite_id("browser.extensions.openOptions", r#"["ext-a"]"#).unwrap(),
            "ext-a"
        );
        let err = parse_open_satellite_id("browser.extensions.openPopup", "[]").unwrap_err();
        assert!(err.contains("openPopup"));
        let err = parse_open_satellite_id("browser.extensions.openOptions", r#"[""]"#).unwrap_err();
        assert!(err.contains("empty id"));
    }

    #[test]
    fn close_satellite_args_shape() {
        parse_close_satellite("[]").unwrap();
        parse_close_satellite("").unwrap();
        let err = parse_close_satellite("{}").unwrap_err();
        assert!(err.contains("closeSatellite"));
    }

    #[test]
    fn parse_store_id_from_url_and_bare() {
        let id = "abcdefghijklmnopabcdefghijklmnop";
        assert_eq!(parse_store_extension_id(id).unwrap(), id);
        assert_eq!(
            parse_store_extension_id(&format!(
                "https://chromewebstore.google.com/detail/foo/{id}"
            ))
            .unwrap(),
            id
        );
        assert!(parse_store_extension_id("not-an-id").is_err());
        assert!(parse_store_extension_id("").is_err());
        assert!(parse_store_extension_id("abcdefghijklmnopqrstuvwxyzabcdef").is_err());
    }

    #[test]
    fn strip_crx3_and_plain_zip() {
        let zip = b"PK\x03\x04hello-zip";
        assert_eq!(strip_crx_header(zip).unwrap(), zip);

        let mut crx = Vec::new();
        crx.extend_from_slice(b"Cr24");
        crx.extend_from_slice(&3u32.to_le_bytes());
        crx.extend_from_slice(&4u32.to_le_bytes()); // header size
        crx.extend_from_slice(&[0, 0, 0, 0]);
        crx.extend_from_slice(zip);
        assert_eq!(strip_crx_header(&crx).unwrap(), zip);

        assert!(strip_crx_header(b"nope").is_err());
    }

    #[test]
    fn unpack_zip_bytes_writes_manifest() {
        let (ext, _) = temp_pair();
        let dest = ext.join("abcdefghijklmnopabcdefghijklmnop");
        // Minimal zip via tar: write a folder and tar it... easier: create zip with tar -a
        let staging = ext.join("_src");
        write_manifest(&staging);
        let zip_path = ext.join("t.zip");
        let status = std::process::Command::new("tar")
            .args([
                "-a",
                "-cf",
                zip_path.to_str().unwrap(),
                "-C",
                staging.to_str().unwrap(),
                "manifest.json",
            ])
            .status()
            .expect("tar create");
        if !status.success() {
            // Environments without tar zip create — skip soft.
            let _ = std::fs::remove_dir_all(ext.parent().unwrap());
            return;
        }
        let bytes = std::fs::read(&zip_path).expect("read zip");
        unpack_zip_bytes(&bytes, &dest).expect("unpack");
        assert!(dest.join("manifest.json").is_file());
        let _ = std::fs::remove_dir_all(ext.parent().unwrap());
    }
}
