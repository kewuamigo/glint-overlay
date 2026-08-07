//! Stop orphaned `GlintMetrics*` ETW sessions and `glint-metrics-etw.exe` processes.

use std::process::Command;
use tracing::info;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Hide console window for `taskkill` / `logman` (GUI launcher must not flash CMD).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const TRACE_PREFIX: &str = "GlintMetrics";
const ETW_PROCESS_IMAGE: &str = "glint-metrics-etw.exe";

#[derive(Debug, Default, Clone)]
pub struct CleanupReport {
    pub etw_processes_killed: bool,
    pub traces_stopped: usize,
    pub errors: Vec<String>,
}

/// Stop orphaned `GlintMetrics*` real-time ETW sessions (does not kill processes).
pub fn stop_glint_trace_sessions() -> usize {
    stop_named_traces(&mut Vec::new())
}

/// Kill helper processes and stop all real-time ETW traces owned by Glint.
pub fn cleanup_glint_etw() -> CleanupReport {
    let mut report = CleanupReport::default();
    report.etw_processes_killed = kill_etw_processes(&mut report.errors);
    report.traces_stopped = stop_named_traces(&mut report.errors);

    if report.traces_stopped > 0 || report.etw_processes_killed {
        info!(
            traces_stopped = report.traces_stopped,
            etw_killed = report.etw_processes_killed,
            "Glint ETW cleanup finished"
        );
    }

    report
}

fn hide_console(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn kill_etw_processes(errors: &mut Vec<String>) -> bool {
    let mut cmd = Command::new("taskkill");
    hide_console(&mut cmd);
    match cmd.args(["/IM", ETW_PROCESS_IMAGE, "/F", "/T"]).output() {
        Ok(output) => {
            if output.status.success() {
                return true;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            // ERROR: The process "..." not found. — nothing to kill
            if stderr.contains("not found") {
                return false;
            }
            errors.push(format!("taskkill: {stderr}"));
            false
        }
        Err(err) => {
            errors.push(format!("taskkill failed: {err}"));
            false
        }
    }
}

fn stop_named_traces(errors: &mut Vec<String>) -> usize {
    let mut query = Command::new("logman");
    hide_console(&mut query);
    let output = match query.args(["query", "-ets"]).output() {
        Ok(output) => output,
        Err(err) => {
            errors.push(format!("logman query failed: {err}"));
            return 0;
        }
    };

    if !output.status.success() {
        errors.push(format!(
            "logman query exit {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
        return 0;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut stopped = 0usize;

    for line in text.lines() {
        let name = line.split_whitespace().next().unwrap_or("");
        if !name.starts_with(TRACE_PREFIX) {
            continue;
        }

        let mut stop = Command::new("logman");
        hide_console(&mut stop);
        match stop.args(["stop", name, "-ets"]).output() {
            Ok(output) if output.status.success() => stopped += 1,
            Ok(output) => errors.push(format!(
                "logman stop {name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(err) => errors.push(format!("logman stop {name}: {err}")),
        }
    }

    stopped
}
