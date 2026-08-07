# Refactoring TODO — Glint Rust

Prioritized backlog for readability and deduplication. Status as of 2026-07-10.

**Note (2026-08-02):** In-game Electron overlay (`host/overlay`,
`host/surface` / `@glint/overlay-electron`) has been **deleted**. Historical
items below that mention `overlay-electron` refer to the pre-CEF cutover host.

Legend: **S** = small (1–2 h), **M** = medium (half day), **L** = large (design decision / multi-day)

---

## Done recently

- [x] `metrics-common` — unified `counter` + `shm` (engine + metrics-native)
- [x] Unified SHM naming: `Local\GlintMetrics-{pid}`
- [x] `IntDashMap` — single definition in `overlay-core`, vulkan-layer reuses it
- [x] Engine dead code cleanup (unused imports, IPC fields, duplicate log)
- [x] Unused Cargo deps removed (overlay-common, engine, overlay-node, launcher)
- [x] Injector DLL path resolution → `injector/src/paths.rs`
- [x] Shared temp-file logging → `metrics-common/src/log.rs` (`debug_log`)
- [x] **P0 #3** — `metrics-common/timing.rs` (EMA + FPS window); `metrics-etw` uses it
- [x] **P0 #1** — `gpu-texture/shared.rs` (`open_shared_texture2d`, `find_dxgi_adapter`); engine + overlay-core dx11
- [x] **P3 #13** — launcher uses `injector::default_overlay_dll_dir()`
- [x] **P3 #12** — `injector/paths.rs::project_root()` shared; launcher UI refresh
- [x] **P0 #4** — `gpu-texture/present.rs` (`resolve_dxgi_present_address`)
- [x] **P0 #2 (partial)** — `overlay-core/hook/dx/render_gate.rs` for dx11 + dx12
- [x] **P0 #2** — `overlay-core/hook/dx/renderer_map.rs` (`with_map_entry` for dx9/dx11/dx12)
- [x] `overlay-core/surface.rs` uses `gpu-texture::open_shared_texture2d`
- [x] `metrics-etw` elevation check via Windows token API
- [x] `engine/input.rs` unsafe-block cleanup
- [x] **P2 #10** — `overlay-dll/server.rs` (`bind_named_pipe`, `listen_loop`, `serve_connection`)
- [x] **P2 #9** — IPC framing documented in `docs/ARCHITECTURE.md`
- [x] **P2 #11** — injector vs `overlay-client` split documented in `docs/ARCHITECTURE.md`
- [x] **P4 #15** — `backend/window/input_event.rs` shared builders (WndProc + LL hooks)
- [x] **P4 #17** — layout uses `PercentLength` from common (verified)
- [x] **P6 #22** — `overlay-node/util::spawn_promise`
- [x] **P6 #23** — overlay-node deps verified (`mimalloc`, `bytemuck`, `num`)
- [x] **P5 #21** — `etw-cleanup` kept separate; documented in README + ARCHITECTURE
- [x] **P1 #7** — engine `input.rs` uses `KBDLLHOOKSTRUCT` / `MSLLHOOKSTRUCT`
- [x] **P1 #8** — engine `present-hook` feature gates hook install
- [x] **P5 #19** — `metrics-etw/fps/` split (`tracker`, `snapshot`, `frame_gen`)
- [x] **P4 #18** — OpenGL/WGL bindings → `src/gl/mod.rs`, `src/wgl/mod.rs`
- [x] **P1 #5** — legacy engine stack removed; single `overlay-electron` host
- [x] **P1 #6** — engine `DrawLayerRequest` + hybrid path module docs (in archive)
- [x] **P8 #27** — SpinningCube: inject, metrics SHM, resize, layout IPC stack, process-alive
- [x] **P7 #25** — `overlay-hook/README.md` documents pregenerated stubs
- [x] **P7 #24** — workspace `windows` = version only; per-crate minimal feature lists
- [x] **P7 #26** — `InjectOptions` + `InjectCliArgs`; `ENV_OVERLAY_DLL_DIR`; `require_overlay_dll_dir` in injector

---

## P0 — High impact, relatively safe

### 1. ~~DXGI shared texture opening~~ **DONE** → `gpu-texture/src/shared.rs`

### 2. ~~`overlay-core` DX renderer hook boilerplate~~ **DONE** → `render_gate.rs` + `renderer_map.rs`

### 3. ~~`metrics-etw` duplicate frametime math~~ **DONE** → `metrics-common/timing.rs`

### 4. ~~Present-hook probe duplication~~ **DONE** → `gpu-texture/src/present.rs`

---

## P1 — Engine crate (deferred stack, still in tree)

### 5. ~~Engine binary IPC protocol~~ **DONE** — removed with deferred stack; single-host Electron path

### 6. ~~Engine `compositor.rs` complexity~~ **DONE** — `DrawLayerRequest`; hybrid path documented in module docs

### 7. ~~Engine `input.rs` low-level hooks~~ **DONE** — `KBDLLHOOKSTRUCT` / `MSLLHOOKSTRUCT` + `# Safety` on procs

### 8. ~~Engine `hook.rs` stub~~ **DONE** — `present-hook` feature gates install/uninstall in `lib.rs`

---

## P2 — overlay-client / overlay-dll / overlay-common

### 9. ~~IPC framing duplication~~ **DONE** — documented in `docs/ARCHITECTURE.md` (active vs deferred)

### 10. ~~`overlay-dll` server setup~~ **DONE** → `overlay-dll/src/server.rs`

### 11. ~~`overlay-client` injector vs `injector` crate~~ **DONE** — split documented in `docs/ARCHITECTURE.md`

---

## P3 — Launcher / injector / ops

### 12. ~~Project root discovery duplication~~ **DONE** → `injector::project_root()`

### 13. ~~Launcher overlay DLL dir hardcoded~~ **DONE** — uses `injector::default_overlay_dll_dir()`

### 14. ~~`injector/process.rs` sort~~ **DONE** — already uses `sort_by_key(|p| p.name.to_ascii_lowercase())`

---

## P4 — overlay-core internals

### 15. ~~`backend/window/` submodules~~ **DONE** → `input_event.rs`

### 16. `renderer/dx/shaders.rs` vs `overlay-vulkan-layer/renderer/shaders.rs` **(L)**
- **Problem:** Separate shader pipelines per backend (expected), but SPIR-V/HLSL blobs may share layout constants.
- **Target:** Only consolidate metadata (vertex layout, blend state), not rendering code.

### 17. ~~`layout.rs` + `overlay-common/size.rs`~~ **DONE** — `overlay-core` uses `PercentLength` from common; no duplicate math

### 18. ~~OpenGL / WGL generated bindings~~ **DONE** → `src/gl/mod.rs`, `src/wgl/mod.rs`

---

## P5 — metrics / ETW

### 19. ~~`metrics-etw` snapshot assembly~~ **DONE** → `fps/tracker.rs`, `fps/snapshot.rs`, `fps/frame_gen.rs`

### 20. ~~`metrics-etw/main.rs` admin check~~ **DONE** — `OpenProcessToken` + `TokenElevation`

### 21. ~~`etw-cleanup` scope~~ **DONE** — separate crate; `crates/etw-cleanup/README.md` + ARCHITECTURE

---

## P6 — overlay-node / neon

### 22. ~~`overlay-node` promise boilerplate~~ **DONE** → `util::spawn_promise`

### 23. ~~`overlay-node` unused deps~~ **DONE** — all three required (documented in `Cargo.toml`)

---

## P8 — Testing

### 27. ~~Integration test harness~~ **DONE** — resize + layout IPC + metrics SHM on SpinningCube

### 24. ~~Root `Cargo.toml` windows features~~ **DONE** — workspace pins version only; each crate lists features

### 25. ~~`overlay-hook` pregenerated stubs~~ **DONE** — documented in crate README

### 26. ~~Binary crate overlap~~ **DONE** — `InjectOptions`, shared env/constants, launcher → Electron only

---

## Suggested order of work

1. **CEF Phase 1** — after TS refactor gate (see ARCHITECTURE.md)
2. **P4 #16** — shader layout metadata (optional, Rust overlay-core)

---

## P9 — React / Electron (new)

### 28. ~~DLL path unification~~ **DONE** — `host/native/src/dll-paths.ts`

### 29. ~~Split `overlay-electron/main.ts`~~ **DONE** — Phase 2
- `overlay-session.ts` (`ElectronOverlay`), `ipc-handlers.ts`, `plugin-protocol.ts`, slim `main.ts` (~55 lines)

### 30. ~~UI package dedup~~ **DONE** — Phase 3–4
- Dead metrics components removed from `packages/ui`; IPC via `overlay-bridge/channels`
- Deferred dual-client stack **removed** (2026-07); single `overlay-electron` host

---

## Out of scope (intentional deferrals)

- CEF host implementation — planned (see ARCHITECTURE.md)
- Plugin SDK memory API stubs — Phase 3
- TypeScript / Electron packages — separate cleanup pass
