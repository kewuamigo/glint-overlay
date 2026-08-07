# Windows packaging

## Local

1. Install [Inno Setup 6](https://jrsoftware.org/isinfo.php) (provides `ISCC.exe`).
2. From repo root:

```powershell
.\scripts\package-release.ps1 -Version 0.1.0
```

Use `-SkipBuild` if you already ran `.\build-all.ps1`. Use `-PortableZip` for a zip beside the Setup exe.

Outputs:
- `dist/release/payload/` — staged install tree
- `dist/release/Glint-Setup-<ver>.exe`
- optional `dist/release/Glint-portable-<ver>.zip`

## CI

Push a tag `v0.1.0` (or run **Release** workflow manually). Artifacts upload to the GitHub Release.