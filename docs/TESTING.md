# Testing

## Quick reference

| Suite | Command | When |
|-------|---------|------|
| **Unit (default)** | `cargo test --workspace` | Every commit — no inject, no admin (`launcher` bin has `test = false` due to admin manifest) |
| **Integration (SpinningCube)** | `$env:GLINT_INTEGRATION=1; cargo test -p glint-integration-tests` | After overlay DLL + metrics DLL built |
| **Single crate** | `cargo test -p glint-overlay-core` | Focused work |

Prerequisites for integration tests:

1. `SpinningCube.exe` at the repository root (D3D11 sample)
2. `cargo build --workspace` and `.\scripts\build-overlay.ps1` (overlay `.dll` set + `.node`)
3. Run from repo root (or any cwd where `injector::project_root()` resolves)

---

## Unit tests (in-crate)

| Crate | What is covered |
|-------|-----------------|
| `gpu-texture` | DXGI `Present` vtable probe (`present.rs`) |
| `metrics-common` | EMA, FPS window math, SHM name format |
| `metrics-etw` | Frame-gen heuristics (`fps/frame_gen.rs`) |
| `overlay-common` | IPC pipe name, `Request` bincode roundtrip, `PercentLength` |
| `overlay-client` | Overlay DLL path constants (`paths.rs`) |
| `overlay-core` | `OverlayLayout::calc`, `renderer_map::with_map_entry` |
| `injector` | `project_root` / `is_project_root` when run inside repo |

```powershell
cargo test --workspace
```

---

## Dynamic integration tests

Crate: `core/tools/integration-tests`

| Test | Verifies |
|------|----------|
| `spinning_cube_inject_ipc_roundtrip` | Spawn sample → inject overlay DLL → `WindowEvent::Added` → `SetPosition` + `ListenInput` IPC |
| `spinning_cube_window_resize_event` | `SetWindowPos` → `WindowEvent::Resized` |
| `spinning_cube_layout_ipc_stack` | `SetAnchor` + `SetMargin` + `SetPosition` + `UpdateSharedHandle(None)` |
| `spinning_cube_process_alive_during_ipc` | Target process stays alive while IPC is open |
| `spinning_cube_metrics_shm_after_present` | Overlay + metrics inject → `read_metrics_for_pid` sees `MetricsBlock` |

Tests **skip** unless `GLINT_INTEGRATION=1` (or `true`) so CI and local `cargo test` stay fast.

```powershell
# Build artifacts first
cargo build --workspace --release
.\scripts\build-overlay.ps1

# Dynamic tests (may need Administrator for some ETW/metrics paths)
$env:GLINT_INTEGRATION = "1"
cargo test -p glint-integration-tests -- --nocapture
```

Single test:

```powershell
$env:GLINT_INTEGRATION = "1"
cargo test -p glint-integration-tests spinning_cube_inject_ipc_roundtrip -- --nocapture
```

---

## Manual smoke (launcher)

```powershell
.\target\release\glint-launcher.exe
# Pick SpinningCube or target game — Shift+Tab overlay
```

---

## Adding tests

- **Pure logic** → `#[cfg(test)]` in the same module as the code.
- **Cross-crate / inject** → `core/tools/integration-tests/tests/` with `skip_unless_integration()`.
- **Do not** inject in unit tests by default — keeps `cargo test --workspace` safe on dev machines.

See also [REFACTORING_TODO.md](./REFACTORING_TODO.md) and [development.md](./development.md).
