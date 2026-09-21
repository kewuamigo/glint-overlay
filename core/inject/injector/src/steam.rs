use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::{AppConfig, GameEntry};
use crate::process::{ProcessEntry, find_pid_by_name, list_all_processes};

#[derive(Debug, Clone, Serialize)]
pub struct ScannedGame {
    pub id: String,
    pub name: String,
    pub source: String,
    pub exe: String,
    pub install_path: String,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub playtime_hours: Option<f64>,
}

#[derive(Debug, Clone)]
struct SteamApp {
    app_id: String,
    name: String,
    install_dir: String,
}

pub fn scan_games(config_path: Option<&Path>) -> Result<Vec<ScannedGame>> {
    let mut games = scan_steam_games()?;
    let config = match config_path {
        Some(path) => AppConfig::load(path)?,
        None => AppConfig::default(),
    };
    merge_config_games(&mut games, &config.games);
    attach_running_state(&mut games);
    games.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    Ok(games)
}

fn merge_config_games(games: &mut Vec<ScannedGame>, entries: &[GameEntry]) {
    for entry in entries {
        if games
            .iter()
            .any(|g| g.exe.eq_ignore_ascii_case(&entry.executable))
        {
            continue;
        }
        games.push(ScannedGame {
            id: format!("config:{}", entry.executable),
            name: entry.name.clone(),
            source: "config".into(),
            exe: entry.executable.clone(),
            install_path: String::new(),
            running: false,
            pid: None,
            playtime_hours: None,
        });
    }
}

fn attach_running_state(games: &mut [ScannedGame]) {
    let processes = list_all_processes().unwrap_or_default();
    for game in games {
        game.running = false;
        game.pid = None;
        let exe_lower = game.exe.to_ascii_lowercase();
        if let Some(proc) = processes
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&game.exe))
        {
            game.running = true;
            game.pid = Some(proc.pid);
            continue;
        }
        if game.source == "config" {
            if let Ok(pid) = find_pid_by_name(&game.exe) {
                game.running = true;
                game.pid = Some(pid);
            }
        } else if !game.install_path.is_empty() {
            if let Some(pid) = find_pid_in_install_dir(&processes, Path::new(&game.install_path)) {
                game.running = true;
                game.pid = Some(pid);
            }
        }
        let _ = exe_lower;
    }
}

fn find_pid_in_install_dir(processes: &[ProcessEntry], install_path: &Path) -> Option<u32> {
    let install = install_path.to_string_lossy().to_ascii_lowercase();
    processes
        .iter()
        .find(|p| {
            p.name.ends_with(".exe")
                && install.contains(&p.name.trim_end_matches(".exe").to_ascii_lowercase())
        })
        .map(|p| p.pid)
        .or_else(|| {
            processes.iter().find_map(|p| {
                if p.name.eq_ignore_ascii_case("game.exe") {
                    Some(p.pid)
                } else {
                    None
                }
            })
        })
}

fn scan_steam_games() -> Result<Vec<ScannedGame>> {
    let mut out = Vec::new();
    for library in steam_library_paths()? {
        let steamapps = library.join("steamapps");
        if !steamapps.is_dir() {
            continue;
        }
        for app in read_steam_apps(&steamapps)? {
            let common = library.join("common").join(&app.install_dir);
            let exe = resolve_game_exe(&common, &app.install_dir);
            out.push(ScannedGame {
                id: format!("steam:{}", app.app_id),
                name: app.name,
                source: "steam".into(),
                exe,
                install_path: common.display().to_string(),
                running: false,
                pid: None,
                playtime_hours: None,
            });
        }
    }
    Ok(out)
}

fn steam_library_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let default_steam = default_steam_dir();
    if default_steam.is_dir() {
        paths.push(default_steam.clone());
    }
    let vdf = default_steam.join("steamapps").join("libraryfolders.vdf");
    if vdf.is_file() {
        let text =
            std::fs::read_to_string(&vdf).with_context(|| format!("read {}", vdf.display()))?;
        for path in parse_vdf_paths(&text) {
            if path.is_dir() {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn default_steam_dir() -> PathBuf {
    std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .map(|p| p.join("Steam"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam"))
}

fn parse_vdf_paths(text: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"path\"") {
            if let Some(value) = vdf_quoted_value(rest) {
                paths.push(PathBuf::from(value.replace("\\\\", "\\")));
            }
        }
    }
    paths
}

fn vdf_quoted_value(rest: &str) -> Option<String> {
    let after_key = rest.trim();
    let start = after_key.find('"')?;
    let inner = &after_key[start + 1..];
    let end = inner.find('"')?;
    Some(inner[..end].to_string())
}

fn read_steam_apps(steamapps: &Path) -> Result<Vec<SteamApp>> {
    let mut apps = Vec::new();
    for entry in std::fs::read_dir(steamapps)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path())?;
        let fields = parse_acf_fields(&text);
        let Some(app_id) = fields.get("appid").cloned() else {
            continue;
        };
        let Some(install_dir) = fields.get("installdir").cloned() else {
            continue;
        };
        let display_name = fields
            .get("name")
            .cloned()
            .unwrap_or_else(|| install_dir.clone());
        apps.push(SteamApp {
            app_id,
            name: display_name,
            install_dir,
        });
    }
    Ok(apps)
}

fn parse_acf_fields(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        for key in ["appid", "name", "installdir"] {
            if let Some(rest) = line.strip_prefix(&format!("\"{key}\"")) {
                if let Some(value) = vdf_quoted_value(rest) {
                    fields.insert(key.to_string(), value);
                }
            }
        }
    }
    fields
}

fn resolve_game_exe(common_dir: &Path, install_dir: &str) -> String {
    if !common_dir.is_dir() {
        return format!("{install_dir}.exe");
    }
    let preferred = [
        "game.exe",
        &format!("{install_dir}.exe"),
        &format!("{}.exe", install_dir.replace(' ', "")),
    ];
    for name in preferred {
        if common_dir.join(name).is_file() {
            return name.to_string();
        }
    }
    let mut best: Option<(u64, String)> = None;
    if let Ok(entries) = std::fs::read_dir(common_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("exe") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case("uninstall.exe") || name.contains("setup") {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().is_none_or(|(s, _)| size > *s) {
                best = Some((size, name));
            }
        }
    }
    best.map(|(_, name)| name)
        .unwrap_or_else(|| format!("{install_dir}.exe"))
}

pub fn steam_display_name() -> Option<String> {
    let steam_dir = default_steam_dir();
    let vdf = steam_dir.join("config").join("loginusers.vdf");
    if !vdf.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(vdf).ok()?;
    parse_vdf_paths(&text);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"PersonaName\"") {
            return vdf_quoted_value(rest);
        }
        if let Some(rest) = line.strip_prefix("\"AccountName\"") {
            if let Some(name) = vdf_quoted_value(rest) {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_libraryfolders_paths() {
        let text = r#"
"libraryfolders"
{
    "0"
    {
        "path"		"C:\\SteamLibrary"
    }
}
"#;
        let paths = parse_vdf_paths(text);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("SteamLibrary"));
    }
}
