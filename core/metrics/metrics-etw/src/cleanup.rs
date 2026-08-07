use glint_etw_cleanup::stop_glint_trace_sessions;

/// Stop stale trace sessions from prior runs. Must NOT kill ETW helper processes —
/// calling full cleanup here would taskkill the current process on startup.
pub fn stop_stale_traces() {
    let stopped = stop_glint_trace_sessions();
    if stopped > 0 {
        tracing::info!(stopped, "stopped stale Glint ETW trace session(s)");
    }
}
