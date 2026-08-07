use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use glint_etw_cleanup::cleanup_glint_etw;
use glint_injector::{list_all_processes, scan_games, steam_display_name, ProcessEntry};
use serde::Serialize;

use crate::elevate::{is_elevated, run_elevated};
use crate::host::spawn_overlay_host;

#[derive(Serialize)]
struct ProcessesResponse {
    processes: Vec<ProcessEntry>,
}

#[derive(Serialize)]
struct GamesResponse {
    games: Vec<glint_injector::ScannedGame>,
}

#[derive(Serialize)]
struct ProfileResponse {
    username: String,
    display_name: String,
    status: String,
}

#[derive(Serialize)]
struct InjectResponse {
    ok: bool,
    pid: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_file: Option<String>,
}

pub fn run_cli(args: &[String]) -> Result<()> {
    match args {
        [cmd] if cmd == "processes" => {
            let processes = list_all_processes()?;
            write_json(&ProcessesResponse { processes })
        }
        [cmd] if cmd == "scan-games" => {
            let config = config_games_path();
            let games = scan_games(config.as_deref())?;
            write_json(&GamesResponse { games })
        }
        [cmd] if cmd == "profile" => {
            write_json(&ProfileResponse {
                username: windows_username(),
                display_name: steam_display_name()
                    .unwrap_or_else(windows_username),
                status: "online".into(),
            })
        }
        [cmd, pid, rest @ ..] if cmd == "inject" => {
            let mut pid: u32 = pid.parse().context("inject requires numeric pid")?;
            let exe_name = rest.first().cloned().or_else(|| {
                list_all_processes().ok()?.into_iter().find(|p| p.pid == pid).map(|p| p.name)
            });

            if !is_elevated() {
                let mut args = vec!["--cli".into(), "inject".into(), pid.to_string()];
                if let Some(ref exe) = exe_name {
                    args.push(exe.clone());
                }
                run_elevated(&args)?;
                return write_json(&InjectResponse {
                    ok: true,
                    pid,
                    log_file: None,
                });
            }

            // Bootstrap exes (e.g. Alan Wake 2) may exit and relaunch — refresh PID by name.
            if !glint_injector::is_process_alive(pid) {
                if let Some(ref exe) = exe_name {
                    pid = glint_injector::find_pid_by_name(exe)
                        .with_context(|| format!("pid {pid} gone; process not found: {exe}"))?;
                } else {
                    anyhow::bail!("pid {pid} is not alive");
                }
            }

            let _ = cleanup_glint_etw();
            let (_child, log_file) = spawn_overlay_host(pid, exe_name.as_deref())?;
            write_json(&InjectResponse {
                ok: true,
                pid,
                log_file: Some(log_file.display().to_string()),
            })
        }
        _ => {
            anyhow::bail!(
                "unknown cli command. Usage: --cli processes|scan-games|profile|inject <pid> [exe]"
            )
        }
    }
}

fn config_games_path() -> Option<PathBuf> {
    glint_injector::project_root().map(|root| root.join("config/games.json"))
}

fn windows_username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "Player".into())
}

fn write_json<T: Serialize>(value: &T) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    Ok(())
}
