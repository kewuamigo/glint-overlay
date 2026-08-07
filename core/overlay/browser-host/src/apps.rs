//! App discovery + enablement for the CEF session owner.
//!
//! Port of the Electron `host/overlay/src/app-registry.ts` scan (same roots,
//! same precedence, same `PluginManifestSummary` shape) plus the enablement
//! policy: `%APPDATA%/Glint/apps-enabled.json` is re-read on every list,
//! so a launcher disable binds into a live session without a new channel.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tracing::warn;

/// Permissions + privileged flag for an enabled app (plugin invoke gate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppAccess {
    pub permissions: Vec<String>,
    pub privileged: bool,
}

/// Manifest summaries for every enabled app, builtin first then by name —
/// the order the shell renders (`apps.list`).
pub fn list_enabled() -> Vec<Value> {
    let mut by_id: BTreeMap<String, Value> = BTreeMap::new();

    for (id, app) in scan_root(&builtin_apps_root()) {
        if app["builtin"] == Value::Bool(true) {
            by_id.insert(id, app);
        }
    }
    for (id, app) in scan_root(&apps_root()) {
        if by_id.get(&id).map(|a| a["builtin"] == Value::Bool(true)) == Some(true) {
            continue;
        }
        by_id.insert(id, app);
    }
    let legacy = legacy_apps_root();
    if legacy != apps_root() {
        for (id, app) in scan_root(&legacy) {
            by_id.entry(id).or_insert(app);
        }
    }

    offer(by_id, &disabled_ids())
}

/// Resolve an enabled app by `pluginId`. `None` if unknown or disabled.
pub fn lookup_enabled(plugin_id: &str) -> Option<AppAccess> {
    list_enabled()
        .into_iter()
        .find(|app| app.get("id").and_then(Value::as_str) == Some(plugin_id))
        .map(|app| access_from(&app))
}

fn access_from(app: &Value) -> AppAccess {
    let permissions = app
        .get("permissions")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let privileged = app.get("privileged") == Some(&Value::Bool(true));
    AppAccess {
        permissions,
        privileged,
    }
}

/// Drops the apps the player disabled and orders what is left for the shell.
fn offer(discovered: BTreeMap<String, Value>, disabled: &HashSet<String>) -> Vec<Value> {
    let mut apps: Vec<Value> = discovered
        .into_iter()
        .filter(|(id, _)| !disabled.contains(id))
        .map(|(_, app)| app)
        .collect();
    apps.sort_by(|a, b| {
        let rank = |app: &Value| u8::from(app["builtin"] != Value::Bool(true));
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    apps
}

/// Ids the player turned off. Absent file / absent id means enabled, matching
/// the Electron host where every discovered app is offered.
fn disabled_ids() -> HashSet<String> {
    let path = match appdata_glint() {
        Some(dir) => dir.join("apps-enabled.json"),
        None => return HashSet::new(),
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_disabled(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashSet::new(),
        Err(err) => {
            warn!(path = %path.display(), %err, "apps-enabled.json unreadable — all apps enabled");
            HashSet::new()
        }
    }
}

/// `{ "<app id>": false }`. Unparseable content leaves every app enabled.
fn parse_disabled(text: &str) -> HashSet<String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        warn!("apps-enabled.json is not a JSON object — all apps enabled");
        return HashSet::new();
    };
    map.into_iter()
        .filter(|(_, enabled)| enabled == &Value::Bool(false))
        .map(|(id, _)| id)
        .collect()
}

/// `(id, PluginManifestSummary)` for every valid app directly under `root`.
fn scan_root(root: &Path) -> Vec<(String, Value)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().to_string();
        if let Some(app) = read_summary(&entry.path(), &id) {
            out.push((id, app));
        }
    }
    out
}

/// Validates the fields the shell depends on and adds the derived
/// `permissions` / `bundleUrl` / `iconUrl` the Electron registry added.
fn read_summary(dir: &Path, id: &str) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let mut manifest: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => {
            warn!(%id, %err, "skip app — invalid manifest.json");
            return None;
        }
    };
    let obj = manifest.as_object_mut()?;
    let named = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    let id_ok = obj.get("id").and_then(Value::as_str) == Some(id)
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    let panels_ok = obj
        .get("panels")
        .and_then(Value::as_array)
        .is_some_and(|panels| !panels.is_empty());
    if !id_ok || !named("name") || !named("version") || !panels_ok {
        warn!(%id, "skip app — manifest does not match the app schema");
        return None;
    }
    let entry = relative(obj.get("entry").and_then(Value::as_str)?);
    if entry.is_empty() || !dir.join(&entry).is_file() {
        warn!(%id, %entry, "skip app — entry file missing");
        return None;
    }
    let icon = obj.get("icon").and_then(Value::as_str).map(relative);
    obj.entry("permissions").or_insert_with(|| json!([]));
    obj.insert(
        "bundleUrl".into(),
        json!(format!("glint-plugin://{id}/{entry}")),
    );
    if let Some(icon) = icon {
        obj.insert(
            "iconUrl".into(),
            json!(format!("glint-plugin://{id}/{icon}")),
        );
    }
    Some(manifest)
}

fn relative(value: &str) -> String {
    value.trim_start_matches("./").to_string()
}

/// Dirs the CEF `glint-plugin` scheme resolves `<app id>/<rel>` under,
/// in the precedence `list_enabled` uses — one scheme for builtin and
/// user-installed apps alike.
pub fn asset_roots() -> Vec<PathBuf> {
    let mut roots = vec![builtin_apps_root(), apps_root()];
    let legacy = legacy_apps_root();
    if !roots.contains(&legacy) {
        roots.push(legacy);
    }
    roots
}

/// React shims the app bundles import as `glint-plugin://_shared/*`.
pub fn shared_deps_root() -> PathBuf {
    repo_dir(
        "GLINT_PLUGIN_SHARED_DIR",
        "host/cef/plugin-shared",
    )
}

fn appdata_glint() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| {
        PathBuf::from(dir).join(glint_overlay_common::product::APP_DATA_DIR_NAME)
    })
}

/// User-installed apps root (`%APPDATA%/Glint/apps`).
/// Plugin `data/store.json` and similar live under `{apps_root()}/<id>/`.
pub fn apps_root() -> PathBuf {
    for var in ["GLINT_APPS_DIR", "GLINT_PLUGINS_DIR"] {
        if let Some(dir) = std::env::var_os(var) {
            return PathBuf::from(dir);
        }
    }
    appdata_glint().unwrap_or_default().join("apps")
}

/// Legacy user install dir before the apps/ migration.
fn legacy_apps_root() -> PathBuf {
    appdata_glint().unwrap_or_default().join("plugins")
}

/// Built-in apps shipped with the repo (`internal-apps/*` with `builtin: true`).
fn builtin_apps_root() -> PathBuf {
    repo_dir("GLINT_BUILTIN_APPS_DIR", "internal-apps")
}

/// Repo-relative dir, resolved against the crate or the staged exe, with an
/// env override for packaged layouts.
fn repo_dir(env_var: &str, rel: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os(env_var) {
        return PathBuf::from(dir);
    }
    let mut candidates = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(rel),
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Packaged: `{install}/native/glint-browser.exe` → `{install}/…`
            candidates.push(dir.join("..").join(rel));
            // Dev staged: `host/native/…` → repo root is two levels up.
            candidates.push(dir.join("../..").join(rel));
        }
    }
    candidates
        .iter()
        .find(|dir| dir.is_dir())
        .cloned()
        .unwrap_or_else(|| candidates.swap_remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_only_for_explicit_false() {
        let disabled = parse_disabled(r#"{"metrics":false,"save-manager":true,"notes":false}"#);
        assert!(disabled.contains("metrics"));
        assert!(disabled.contains("notes"));
        assert!(!disabled.contains("save-manager"));
    }

    #[test]
    fn unparseable_enablement_leaves_apps_enabled() {
        assert!(parse_disabled("").is_empty());
        assert!(parse_disabled("[\"metrics\"]").is_empty());
        assert!(parse_disabled("{").is_empty());
    }

    #[test]
    fn disabled_apps_are_not_offered() {
        let discovered = BTreeMap::from([
            (
                "metrics".to_string(),
                json!({"id":"metrics","name":"Metrics","builtin":true}),
            ),
            ("notes".to_string(), json!({"id":"notes","name":"Notes"})),
            ("atlas".to_string(), json!({"id":"atlas","name":"Atlas"})),
        ]);
        let offered = offer(discovered, &HashSet::from(["notes".to_string()]));
        let ids: Vec<&str> = offered.iter().map(|a| a["id"].as_str().unwrap()).collect();
        // Builtin first, then by name; the disabled app is gone.
        assert_eq!(ids, ["metrics", "atlas"]);
    }

    #[test]
    fn missing_app_dir_scans_empty() {
        assert!(scan_root(Path::new("C:/glint-no-such-apps-root")).is_empty());
    }

    #[test]
    fn access_from_reads_permissions_and_privileged() {
        let app = json!({
            "id": "browser",
            "privileged": true,
            "permissions": ["metrics", "storage"],
        });
        assert_eq!(
            access_from(&app),
            AppAccess {
                permissions: vec!["metrics".into(), "storage".into()],
                privileged: true,
            }
        );
        let plain = json!({"id": "notes"});
        assert_eq!(
            access_from(&plain),
            AppAccess {
                permissions: vec![],
                privileged: false,
            }
        );
    }
}
