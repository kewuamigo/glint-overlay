use anyhow::{Context, Result};
use serde::Serialize;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

use crate::config::GameEntry;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub name: String,
}

pub fn list_all_processes() -> Result<Vec<ProcessEntry>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .context("CreateToolhelp32Snapshot failed")?;

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    let mut processes = Vec::new();

    unsafe {
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = exe_name(&entry);
                if !name.is_empty() && entry.th32ProcessID != 0 {
                    processes.push(ProcessEntry {
                        pid: entry.th32ProcessID,
                        name,
                    });
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
    }

    processes.sort_by_key(|p| p.name.to_ascii_lowercase());
    Ok(processes)
}

pub fn find_pid_by_name(name: &str) -> Result<u32> {
    let name_lower = name.to_lowercase();
    for process in list_all_processes()? {
        if process.name.eq_ignore_ascii_case(&name_lower) {
            return Ok(process.pid);
        }
    }
    anyhow::bail!("process not found: {name}")
}

/// Returns true when `pid` is still a live process.
pub fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = windows::Win32::Foundation::CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

/// Resolve a CLI target string as PID or executable name.
pub fn resolve_pid_target(target: &str) -> Result<u32> {
    if let Ok(pid) = target.parse::<u32>() {
        Ok(pid)
    } else {
        find_pid_by_name(target)
    }
}

pub fn list_running_targets(games: &[GameEntry]) -> Result<Vec<(String, u32)>> {
    let mut results = Vec::new();
    for game in games {
        if let Ok(pid) = find_pid_by_name(&game.executable) {
            results.push((game.name.clone(), pid));
        }
    }
    Ok(results)
}

fn exe_name(entry: &PROCESSENTRY32W) -> String {
    String::from_utf16_lossy(
        &entry
            .szExeFile
            .iter()
            .take_while(|&&c| c != 0)
            .copied()
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_process_alive_for_current_pid() {
        let pid = std::process::id();
        assert!(is_process_alive(pid));
        assert!(!is_process_alive(0));
    }
}
