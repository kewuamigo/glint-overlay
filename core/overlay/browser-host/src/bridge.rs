//! Pure `__goHost.invoke` dispatch (no CEF / session side effects).
//!
//! Session-owned methods (`ui.toggleInteractive`, `browser.*`) stay in `main`.
//! Contract: `specs/002-cef-overlay-host/contracts/host-bridge.md`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::apps;
use crate::browser_extensions;
use serde_json::Value;

/// In-session panel pin map (`{manifestId}:{panelId}` → pinned).
/// Shared with `main` so T019/T020 can drive SessionMode from pin emptiness.
pub type PinMap = Arc<Mutex<HashSet<String>>>;

pub fn new_pin_map() -> PinMap {
    Arc::new(Mutex::new(HashSet::new()))
}

pub fn any_pinned(pins: &PinMap) -> bool {
    pins.lock().map(|g| !g.is_empty()).unwrap_or(false)
}

/// Apply `panel.setPinned` args `[key, pinned]` to the shared map. Returns ack.
pub fn set_pinned(pins: &PinMap, args_json: &str) -> Result<String, String> {
    let (key, pinned): (String, bool) = serde_json::from_str(args_json)
        .map_err(|err| format!("panel.setPinned needs [key, pinned]: {err}"))?;
    if key.is_empty() {
        return Err("panel.setPinned: empty key".into());
    }
    let mut map = pins.lock().map_err(|_| "pin map poisoned".to_string())?;
    if pinned {
        map.insert(key);
    } else {
        map.remove(&key);
    }
    Ok(String::new())
}

/// Resolve a host method that needs only process-local state (disk scan, ack).
pub fn dispatch_sync(
    method: &str,
    args_json: &str,
    plugin_id: &str,
    pins: &PinMap,
) -> Result<String, String> {
    if !plugin_id.is_empty() {
        return crate::plugin_ipc::dispatch(plugin_id, method, args_json);
    }
    match method {
        // Re-scanned per call so a launcher enable/disable binds live.
        "apps.list" => serde_json::to_string(&apps::list_enabled()).map_err(|err| err.to_string()),
        // Disk + prefs only — CEF applies on next helper start (`--load-extension`).
        "browser.extensions.list" => {
            serde_json::to_string(&browser_extensions::list()).map_err(|err| err.to_string())
        }
        "browser.extensions.setEnabled" => browser_extensions::set_enabled(args_json),
        "browser.extensions.installFromStore" => {
            browser_extensions::install_from_store(args_json)
        }
        "panel.setPinned" => set_pinned(pins, args_json),
        // Shell-local focus — ack so the caller settles.
        "ui.setFocusedKey" => Ok(String::new()),
        "ui.playAchievementSound" => {
            let rare = serde_json::from_str::<Vec<Value>>(args_json)
                .ok()
                .and_then(|a| a.first().cloned())
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            crate::plugin_achievements::play_toast_sound(rare);
            Ok(String::new())
        }
        other => Err(format!("unsupported host method: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_cef_protocol::{
        BridgeInvoke, BridgeResult, Envelope, PROTOCOL_VERSION, decode_envelope, encode_envelope,
        envelope::Body,
    };

    /// Contract: BridgeInvoke(apps.list) → dispatch → BridgeResult(ok, JSON array).
    #[test]
    fn apps_list_invoke_result_roundtrip() {
        let pins = new_pin_map();
        let invoke = Envelope {
            protocol_version: PROTOCOL_VERSION as i32,
            correlation_id: 0,
            body: Some(Body::BridgeInvoke(BridgeInvoke {
                request_id: 7,
                method: "apps.list".into(),
                args_json: "[]".into(),
                plugin_id: String::new(),
            })),
        };
        let inv_bytes = encode_envelope(&invoke).expect("encode invoke");
        let inv_decoded = decode_envelope(&inv_bytes).expect("decode invoke");
        let Some(Body::BridgeInvoke(inv)) = inv_decoded.body else {
            panic!("expected BridgeInvoke");
        };
        assert_eq!(inv.method, "apps.list");
        assert_eq!(inv.request_id, 7);

        let result_json = dispatch_sync(&inv.method, &inv.args_json, &inv.plugin_id, &pins)
            .expect("apps.list should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&result_json).expect("apps.list returns JSON");
        assert!(parsed.is_array(), "apps.list result must be a JSON array");

        let result = Envelope {
            protocol_version: PROTOCOL_VERSION as i32,
            correlation_id: 0,
            body: Some(Body::BridgeResult(BridgeResult {
                request_id: inv.request_id,
                ok: true,
                result_json,
                error: String::new(),
            })),
        };
        let res_bytes = encode_envelope(&result).expect("encode result");
        let res_decoded = decode_envelope(&res_bytes).expect("decode result");
        let Some(Body::BridgeResult(res)) = res_decoded.body else {
            panic!("expected BridgeResult");
        };
        assert!(res.ok);
        assert_eq!(res.request_id, 7);
        assert!(res.error.is_empty());
        let again: serde_json::Value =
            serde_json::from_str(&res.result_json).expect("result_json still JSON");
        assert!(again.is_array());
    }

    #[test]
    fn unknown_method_rejects() {
        let pins = new_pin_map();
        let err = dispatch_sync("no.such", "[]", "", &pins).expect_err("unknown");
        assert!(err.contains("unsupported host method"));
    }

    #[test]
    fn extensions_set_enabled_args_shape() {
        let pins = new_pin_map();
        let err = dispatch_sync("browser.extensions.setEnabled", "[]", "", &pins)
            .expect_err("bad args");
        assert!(err.contains("setEnabled"));
    }

    #[test]
    fn extensions_satellite_methods_not_sync() {
        // open/close need CEF session path in main — not disk-only dispatch_sync.
        let pins = new_pin_map();
        for method in [
            "browser.extensions.openOptions",
            "browser.extensions.openPopup",
            "browser.extensions.closeSatellite",
        ] {
            let err = dispatch_sync(method, r#"["x"]"#, "", &pins).expect_err(method);
            assert!(
                err.contains("unsupported host method"),
                "{method}: {err}"
            );
        }
    }

    #[test]
    fn set_pinned_updates_map() {
        let pins = new_pin_map();
        assert!(!any_pinned(&pins));
        set_pinned(&pins, r#"["metrics:main",true]"#).expect("pin");
        assert!(any_pinned(&pins));
        set_pinned(&pins, r#"["metrics:main",false]"#).expect("unpin");
        assert!(!any_pinned(&pins));
    }
}
