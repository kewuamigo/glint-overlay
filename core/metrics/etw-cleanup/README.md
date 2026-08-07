# glint-etw-cleanup

Small standalone crate for stopping orphaned Glint ETW sessions.

## Why a separate crate?

`cleanup_glint_etw` is called from:

- **`launcher`** — user-facing cleanup before starting metrics
- **`metrics-etw`** — session teardown on shutdown

Keeping it out of `metrics-etw` avoids pulling ETW binary dependencies into the launcher, and avoids a circular dependency if `metrics-etw` ever needs cleanup during its own startup.

## API

- `stop_glint_trace_sessions()` — stop `GlintMetrics*` traces only
- `cleanup_glint_etw()` — kill `glint-metrics-etw.exe` + stop traces

Uses `logman` and `taskkill` (Windows only).
