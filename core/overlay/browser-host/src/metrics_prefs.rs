//! Persist metrics HUD detail level + tile flags next to other Glint prefs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetricsPrefs {
    pub detail_level: String,
    pub tiles: Tiles,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Tiles {
    pub fps: bool,
    pub cpu: bool,
    pub gpu: bool,
    pub ram: bool,
    pub vram: bool,
    pub graph: bool,
}

impl MetricsPrefs {
    fn classic() -> Self {
        Self {
            detail_level: "classic".into(),
            tiles: Tiles {
                fps: true,
                cpu: false,
                gpu: false,
                ram: false,
                vram: false,
                graph: false,
            },
        }
    }

    fn for_level(level: &str) -> Self {
        match level {
            "util" => Self {
                detail_level: "util".into(),
                tiles: Tiles {
                    fps: true,
                    cpu: true,
                    gpu: true,
                    ram: false,
                    vram: false,
                    graph: false,
                },
            },
            "full" => Self {
                detail_level: "full".into(),
                tiles: Tiles {
                    fps: true,
                    cpu: true,
                    gpu: true,
                    ram: true,
                    vram: true,
                    graph: false,
                },
            },
            _ => Self::classic(),
        }
    }
}

static CACHE: Mutex<Option<MetricsPrefs>> = Mutex::new(None);

pub fn get() -> MetricsPrefs {
    if let Ok(g) = CACHE.lock() {
        if let Some(p) = g.as_ref() {
            return p.clone();
        }
    }
    let p = load(&prefs_path());
    if let Ok(mut g) = CACHE.lock() {
        *g = Some(p.clone());
    }
    p
}

pub fn set_from_args(args_json: &str) -> Result<MetricsPrefs, String> {
    let args: Vec<Value> =
        serde_json::from_str(args_json).map_err(|e| format!("invalid setPrefs args: {e}"))?;
    let raw = args.first().cloned().unwrap_or(Value::Null);
    let mut next = get();
    if let Some(level) = raw.get("detailLevel").and_then(|v| v.as_str()) {
        if !matches!(level, "classic" | "util" | "full") {
            return Err("detailLevel must be classic|util|full".into());
        }
        if level != next.detail_level {
            next = MetricsPrefs::for_level(level);
        }
    }
    if let Some(tiles) = raw.get("tiles").and_then(|v| v.as_object()) {
        apply_tile(&mut next.tiles.fps, tiles.get("fps"));
        apply_tile(&mut next.tiles.cpu, tiles.get("cpu"));
        apply_tile(&mut next.tiles.gpu, tiles.get("gpu"));
        apply_tile(&mut next.tiles.ram, tiles.get("ram"));
        apply_tile(&mut next.tiles.vram, tiles.get("vram"));
        apply_tile(&mut next.tiles.graph, tiles.get("graph"));
    }
    save(&prefs_path(), &next)?;
    if let Ok(mut g) = CACHE.lock() {
        *g = Some(next.clone());
    }
    Ok(next)
}

pub fn to_json(p: &MetricsPrefs) -> Value {
    json!({
        "detailLevel": p.detail_level,
        "tiles": {
            "fps": p.tiles.fps,
            "cpu": p.tiles.cpu,
            "gpu": p.tiles.gpu,
            "ram": p.tiles.ram,
            "vram": p.tiles.vram,
            "graph": p.tiles.graph,
        }
    })
}

fn apply_tile(slot: &mut bool, v: Option<&Value>) {
    if let Some(b) = v.and_then(|x| x.as_bool()) {
        *slot = b;
    }
}

fn load(path: &Path) -> MetricsPrefs {
    let Ok(text) = std::fs::read_to_string(path) else {
        return MetricsPrefs::classic();
    };
    serde_json::from_str(&text).unwrap_or_else(|_| MetricsPrefs::classic())
}

fn save(path: &Path, prefs: &MetricsPrefs) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("prefs dir: {e}"))?;
    }
    let text = serde_json::to_string_pretty(prefs).map_err(|e| format!("prefs encode: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("prefs write: {e}"))
}

fn prefs_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("GLINT_METRICS_PREFS") {
        return PathBuf::from(dir);
    }
    std::env::var_os("APPDATA")
        .map(|dir| {
            PathBuf::from(dir)
                .join(glint_overlay_common::product::APP_DATA_DIR_NAME)
                .join("metrics-prefs.json")
        })
        .unwrap_or_else(|| PathBuf::from("metrics-prefs.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_prefs(f: impl FnOnce()) {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("go-metrics-prefs-{n}.json"));
        let prev = std::env::var_os("GLINT_METRICS_PREFS");
        unsafe { std::env::set_var("GLINT_METRICS_PREFS", &path) };
        if let Ok(mut g) = CACHE.lock() {
            *g = None;
        }
        f();
        match prev {
            Some(v) => unsafe { std::env::set_var("GLINT_METRICS_PREFS", v) },
            None => unsafe { std::env::remove_var("GLINT_METRICS_PREFS") },
        }
        if let Ok(mut g) = CACHE.lock() {
            *g = None;
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn default_is_classic_fps() {
        with_temp_prefs(|| {
            let p = get();
            assert_eq!(p.detail_level, "classic");
            assert!(p.tiles.fps);
            assert!(!p.tiles.gpu);
        });
    }

    #[test]
    fn set_level_applies_util_tiles() {
        with_temp_prefs(|| {
            let p = set_from_args(r#"[{"detailLevel":"util"}]"#).unwrap();
            assert_eq!(p.detail_level, "util");
            assert!(p.tiles.cpu && p.tiles.gpu);
            assert!(!p.tiles.ram);
            assert_eq!(get().detail_level, "util");
        });
    }
}
