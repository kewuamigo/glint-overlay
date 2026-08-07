## Active path (production)

```
glint-launcher.exe          # Electron — out-of-game only
  └── inject overlay DLL + spawn glint-browser.exe
        ├── core/overlay/overlay-dll — inject + GPU compositor + pipe
        ├── core/overlay/browser-host — CEF UI helper (shell + apps)
        ├── host/cef → glint-cef.exe — windowless OSR
        ├── ui/shell — React shell (served into CEF)
        └── internal-apps/browser — Browser AppWindow + content OSR hole
```

**Bridge (egy global, ne bonyolítsd):** [architektura.md](./architektura.md)  
In-game host cutover: [specs/002-cef-overlay-host](../specs/002-cef-overlay-host/spec.md).

**Browser:** Shell + apps run in `glint-browser` / CEF. Nav via `__goHost`
invoke (`browser.navigate` / reload / back / forward).

## IPC and shared memory

The repo has **two named-pipe protocols** from different eras. Only one is active in production.

### Active — `overlay-common` (bincode + length prefix)

Used by **`overlay-dll`** (server) and **`overlay-client`** / **`browser-host`** (client).

| Layer | Format |
|-------|--------|
| Transport | Windows named pipe, byte stream |
| Framing | `u32` little-endian body length, then body (`Frame` in `overlay-common/src/ipc.rs`) |
| Payload | **bincode** (standard config) |
| Client → server | `ClientRequest { id: u32, req: Request }` |
| Server → client | `ServerToClientPacket::Response { id, data }` or `::Event(OverlayEvent)` |

**Pipe name:** `\\.\pipe\glint-overlay-{pid}-{module_handle}`  
CEF helper connects to the **`-cef`** suffix pipe (`create_ipc_addr_cef`).

`module_handle` is the injected DLL base address in the target process. Multiple overlay DLL instances in one process get distinct pipes.

**Flow:**

```
Launcher
  injector::inject_overlay_cef_pipe(pid, …)
    → RtlCreateUserThread + LoadLibraryW (overlay-dll)
  spawn glint-browser (GLINT_PIPE=…-cef)
    → NamedPipeClient connects to CEF pipe
  overlay-dll::listen_loop
    → serve_connection: recv ClientRequest, reply ServerResponse, push OverlayEvent
```

Request types live in `overlay-common/src/request.rs`. Events are defined in `overlay-event`.

Implementation references:

- Server bind/listen: `overlay-dll/src/server.rs`
- Client read/write: `overlay-client/src/client.rs`
- Shared types: `overlay-common/src/ipc.rs`
- Session owner: `core/overlay/browser-host/`

### Removed — legacy paths

- Dual-Electron stack (`overlay-hud` + `overlay-browser`, opcode pipe) — removed.
- In-game Electron overlay (`host/overlay`, `host/surface`) — removed after CEF cutover.

**Do not merge** a second pipe protocol with `overlay-common`.

### CEF browser

`glint-cef.exe` (`host/cef`) renders windowless (OSR). The UI helper
(`glint-browser`) stamps shared textures onto overlay layer ≥1.
Obtain binaries: `pwsh scripts/fetch-cef.ps1` then `pwsh scripts/build-cef.ps1`.

### Metrics — shared memory (not a pipe)

FPS / frametime for the metrics DLL uses a separate channel:

- **Name:** `Local\GlintMetrics-{pid}` (`metrics-common/src/shm.rs`)
- **Layout:** `MetricsBlock` — version, fps, frametime EMA, etc.
- **Readers:** `metrics-etw`, launcher, host bridge via `read_metrics_for_pid`

No relation to overlay IPC framing.

## Injector vs overlay-client

Both inject DLLs; responsibilities are split on purpose.

| Crate | Role | Used by |
|-------|------|---------|
| **`overlay-client`** | Low-level inject (`NtOpenProcess` + `RtlCreateUserThread`), arch detection, **IPC client** (`IpcClientConn`) | `browser-host`, tests |
| **`injector`** (lib) | Path discovery, default DLL locations, `inject_overlay_cef_pipe` / `inject_metrics_dll`, process list | `launcher` |

**Path discovery** (`project_root`, `find_overlay_dll_dir`, `find_built_dll`) lives in **`injector::paths`**.

**Rule of thumb:** library integration → `overlay-client`; desktop launcher attach → `injector` lib (no separate injector.exe).

### ETW cleanup (`etw-cleanup` crate)

Separate small crate (not merged into `metrics-etw`) because both **`launcher`** and **`metrics-etw`** call it — see `core/metrics/etw-cleanup/README.md`.

## Components

| Component | Path | Role |
|-----------|------|------|
| Launcher | `core/launcher/launcher` + `host/launcher` | Process picker; inject + spawn CEF UI helper |
| Overlay DLL | `core/overlay/overlay-dll` | GPU hooks (DX9–12, Vulkan, GL), IPC server |
| Overlay client | `core/overlay/overlay-client` | Inject + IPC from Rust hosts |
| CEF UI helper | `core/overlay/browser-host` | Shell/apps session, `__goHost` bridge, stamps |
| CEF runtime | `host/cef` | `glint-cef.exe` + `plugin-shared` shims |
| UI | `ui/shell` | React shell bundle |
| Session modes | `browser-host` `SessionMode` | Hidden / HudPinned / Interactive |
| Browser app | `internal-apps/browser` | Dock Browser + content hole |
| ETW metrics | `core/metrics/metrics-etw` | Display FPS (+ merges Present-hook SHM) |

## Build

```powershell
.\build-all.ps1
# or
cargo build --release
.\scripts\build-overlay.ps1   # MSVC required for overlay DLL
.\scripts\build-browser.ps1
.\scripts\build-cef.ps1
npm run build
```