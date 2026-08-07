//! Plugin key/value store — Electron `plugin-storage.ts` parity.
//!
//! On disk: `{apps_root}/{pluginId}/data/store.json` (JSON object, 2-space pretty).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

fn store_path(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("data").join("store.json")
}

fn ensure_data_dir(plugin_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(plugin_dir.join("data"))
        .map_err(|e| format!("storage mkdir failed: {e}"))
}

fn read_store(plugin_dir: &Path) -> Result<Map<String, Value>, String> {
    ensure_data_dir(plugin_dir)?;
    let path = store_path(plugin_dir);
    if !path.exists() {
        return Ok(Map::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("storage read failed: {e}"))?;
    let raw = raw.strip_prefix('\u{FEFF}').unwrap_or(raw.as_str());
    let parsed: Value =
        serde_json::from_str(raw).map_err(|e| format!("storage parse failed: {e}"))?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err("storage store.json must be a JSON object".into()),
    }
}

fn write_store(plugin_dir: &Path, store: &Map<String, Value>) -> Result<(), String> {
    ensure_data_dir(plugin_dir)?;
    let text = serde_json::to_string_pretty(&Value::Object(store.clone()))
        .map_err(|e| format!("storage encode failed: {e}"))?;
    std::fs::write(store_path(plugin_dir), text).map_err(|e| format!("storage write failed: {e}"))
}

/// Missing key → JSON `null` (Electron parity).
pub fn get(plugin_dir: &Path, key: &str) -> Result<Value, String> {
    let store = read_store(plugin_dir)?;
    Ok(store.get(key).cloned().unwrap_or(Value::Null))
}

pub fn set(plugin_dir: &Path, key: &str, value: Value) -> Result<(), String> {
    let mut store = read_store(plugin_dir)?;
    store.insert(key.to_string(), value);
    write_store(plugin_dir, &store)
}

pub fn remove(plugin_dir: &Path, key: &str) -> Result<(), String> {
    let mut store = read_store(plugin_dir)?;
    store.remove(key);
    write_store(plugin_dir, &store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_plugin_dir() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("go-storage-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_get_set_remove() {
        let dir = temp_plugin_dir();
        assert_eq!(get(&dir, "k").unwrap(), Value::Null);

        set(&dir, "k", Value::String("v".into())).unwrap();
        assert_eq!(get(&dir, "k").unwrap(), Value::String("v".into()));

        let raw = std::fs::read_to_string(store_path(&dir)).unwrap();
        assert!(raw.contains("\n  "));
        assert!(!raw.starts_with('\u{FEFF}'));

        remove(&dir, "k").unwrap();
        assert_eq!(get(&dir, "k").unwrap(), Value::Null);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strips_bom_on_read() {
        let dir = temp_plugin_dir();
        ensure_data_dir(&dir).unwrap();
        std::fs::write(
            store_path(&dir),
            "\u{FEFF}{\n  \"bom\": true\n}",
        )
        .unwrap();
        assert_eq!(get(&dir, "bom").unwrap(), Value::Bool(true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
