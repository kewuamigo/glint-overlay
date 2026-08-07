# @glint/cloud-sync-server

Minimal self-host HTTP backend for Glint cloud sync revisions.
Stores revision directories (`manifest.json`, `saves/`, `achievements.json`) under a configurable disk root. Auth is a static bearer token.

## Run

```bash
cd packages/cloud-sync-server
pnpm install   # from repo root if needed
pnpm build

# required
set CLOUD_SYNC_TOKEN=dev-secret          # PowerShell: $env:CLOUD_SYNC_TOKEN="dev-secret"
# optional (defaults shown)
set CLOUD_SYNC_ROOT=./cloud-sync-data
set CLOUD_SYNC_PORT=8787
set CLOUD_SYNC_HOST=127.0.0.1

pnpm start
```

| Env | Required | Default | Meaning |
|-----|----------|---------|---------|
| `CLOUD_SYNC_TOKEN` | yes | — | Bearer token (`Authorization: Bearer …`) |
| `CLOUD_SYNC_ROOT` | no | `./cloud-sync-data` | Disk root for `{gameId}/{revisionId}/` trees |
| `CLOUD_SYNC_PORT` | no | `8787` | Listen port |
| `CLOUD_SYNC_HOST` | no | `127.0.0.1` | Bind address |

## API

All routes except `GET /health` require `Authorization: Bearer <CLOUD_SYNC_TOKEN>`.
Unauthorized requests return `401` and do not read or write revision blobs.

| Method | Path | Body | Notes |
|--------|------|------|-------|
| `GET` | `/health` | — | Liveness |
| `GET` | `/v1/games/:gameId/revisions` | — | JSON `{ revisions: SyncRevisionInfo[] }` |
| `PUT` | `/v1/games/:gameId/revisions/:revisionId` | `application/x-tar` | Upload packed revision dir; `201` + `SyncRevisionInfo` |
| `GET` | `/v1/games/:gameId/revisions/:revisionId` | — | Download tar of revision dir |
| `DELETE` | `/v1/games/:gameId/revisions/:revisionId` | — | Remove revision; `204` (or `404` if missing) |

`gameId` / `revisionId` must match `[a-zA-Z0-9._-]{1,128}`. The tar must contain a valid `manifest.json` whose `gameId` / `revisionId` match the URL.

## Docker

From repo root (or this directory):

```bash
cd packages/cloud-sync-server
cp .env.example .env   # set CLOUD_SYNC_TOKEN
docker compose up --build -d
```

| Compose env | Meaning |
|-------------|---------|
| `CLOUD_SYNC_TOKEN` | **Required** bearer token (set in `.env`) |
| `CLOUD_SYNC_PORT` | Host port mapped to container `8787` (default `8787`) |

Data persists in the `cloud-sync-data` Docker volume (`/data` in the container). Bind address inside the container is `0.0.0.0`.

Launcher Settings → Cloud Sync → self-host: `http://127.0.0.1:8787` (or your host) + the same token.

```bash
docker compose logs -f
docker compose down
```

## Smoke test

Starts a temporary server, checks auth rejection, upload → list → download, long-path tar round-trip, and DELETE:

```bash
pnpm smoke
```
