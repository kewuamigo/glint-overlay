# glint-cef

Windowless CEF helper used by `glint-browser` for in-game shell/apps and
browser content (OSR).

## Obtain CEF binaries

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/fetch-cef.ps1
```

Downloads the Spotify CEF **windows64 standard** build into `third_party/cef/` (gitignored).

## Build

Requires Visual Studio 2022 (C++), CMake 3.21+.

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/build-cef.ps1
```

Stages `glint-cef.exe` + CEF DLLs to `host/native/cef/`.

Optional: `GLINT_CEF_EXE` env var overrides the helper path.

## Runtime

`glint-browser` (`core/overlay/browser-host`) spawns this helper and speaks
the existing CEF IPC (length-prefixed frames). App bundles resolve via the
`glint-plugin://` scheme; React shims live in `host/cef/plugin-shared/`.

Do not commit `third_party/cef` or `host/native/cef` binaries.
