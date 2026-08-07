# Glint

Glint is an open-source Windows game overlay — hotkey to toggle, translucent UI over the game, and room for community apps. Think Steam Overlay energy without locking you into someone else's store.

It ships a launcher that attaches to games, an in-game React UI hosted in CEF (`glint-browser`), a small app SDK, built-in apps (metrics, browser, achievements), and optional **cloud save sync** you can self-host.

## Features

- Multi-API overlay hooks (DX9–12, Vulkan, OpenGL)
- Window modes: hidden, HUD-pinned, interactive
- In-game browser (tabs, navigation, bookmarks)
- App SDK for third-party React bundles
- Dual FPS paths: Present hook + ETW FrameGen-aware metrics
- Launcher cloud sync for saves + achievements (self-host, Drive, or FTP)

## Prerequisites

- Windows 10/11
- Node.js 22+
- Rust 1.85+
- MSVC Build Tools (recommended for the overlay DLL)

CEF binaries are fetched during build (see `scripts/fetch-cef.ps1`) — not vendored in git.

## Quick start

```powershell
npx pnpm@10.12.1 install
.\build-all.ps1

# Run as Administrator:
.\target\release\glint-launcher.exe
```

In game, **Shift+Tab** toggles the interactive overlay.

More detail: [docs/development.md](docs/development.md) · [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) · [docs/TESTING.md](docs/TESTING.md)

## Cloud sync (self-host)

Want game saves and achievement snapshots on a machine you control?

1. Follow **[packages/cloud-sync-server/README.md](packages/cloud-sync-server/README.md)** — local run or Docker Compose.
2. Set a bearer token (`CLOUD_SYNC_TOKEN`).
3. In the launcher: **Settings → Cloud Sync** → self-host URL (e.g. `http://127.0.0.1:8787`) + the same token.

```powershell
cd packages/cloud-sync-server
cp .env.example .env   # set CLOUD_SYNC_TOKEN
docker compose up --build -d
```

## Project structure

```
.
├── core/            # Rust: overlay, metrics, inject, launcher CLI
├── host/            # Electron launcher + CEF staging
├── ui/              # React overlay shell + launcher UI
├── sdk/             # Plugin SDK + bridge types
├── internal-apps/   # Built-in apps (metrics, browser, achievements, …)
├── packages/        # Shared libs + cloud-sync-server
├── docs/            # Architecture, development, compatibility
└── scripts/         # Build / package helpers
```

User-installed apps live at `%APPDATA%/Glint/apps/<id>/` — not in this repo.

## Contributing

PRs welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for build, test, and review expectations.

## Roadmap

Near-term and later themes: [ROADMAP.md](ROADMAP.md).

## Releases

Tagged GitHub Releases will appear on this repository. Early tags may be **source-only** (build with the quick start above); installer/portable assets follow when packaging CI is solid on this repo.

## License

MIT — see [LICENSE](LICENSE).
