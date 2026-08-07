# glint-cef-protocol

Source of truth for the host ↔ CEF Protobuf IPC (`MSG_PROTO` / `Envelope`).

Schema: [`proto/cef_ipc.proto`](proto/cef_ipc.proto)

## Rust

```bash
cargo test -p glint-cef-protocol
```

Build uses `prost-build` + vendored `protoc` (`protoc-bin-vendored`) — no system protoc required.

## Schema notes

- **Version negotiation:** `Hello` / `HelloAck` bodies are empty; the spoken version is `Envelope.protocol_version`.
- **Paint:** `nt_handle` is `uint64` (shared texture NT handle).
- **Error:** `Error.correlation_id` may echo the request; `Envelope.correlation_id` is authoritative for pairing.
- **Dropped old JSON ops (intentional):** `set_size`, `set_visible`, and HWND / `hwnd` fields — use `SetInnerBounds` / `SetSurfaceSize` / `SetHidden`; OSR-only until a future `SessionMode`.

## Regen C++

Generated sources live at `host/cef/src/ipc/gen/cef_ipc.pb.{h,cc}` (package `gameoverlay.cef`).

**Preferred:** `scripts/build-cef.ps1` FetchContent-builds protobuf **v21.12** and runs its `protoc` via CMake `add_custom_command` before compiling `glint-cef`.

**Manual** (must use a protoc that matches the linked libprotobuf — currently **21.12**):

```powershell
# From repo root, with protoc 21.12 on PATH (or set $Protoc):
$Proto = (Resolve-Path "core/overlay/cef-protocol/proto").Path
$Out   = (Resolve-Path "host/cef/src/ipc/gen" -ErrorAction SilentlyContinue)
if (-not $Out) { $Out = (New-Item -ItemType Directory -Force -Path "host/cef/src/ipc/gen").FullName }
# Leading colon on --cpp_out avoids Windows drive-letter being parsed as a plugin option.
protoc -I $Proto "--cpp_out=:$Out" "$Proto/cef_ipc.proto"
```

Do not regenerate with a mismatched major (e.g. protoc 31.x against libprotobuf 21.x).
