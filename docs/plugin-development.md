# App Development Guide

> **Új fejlesztőknek:** olvasd el először a [Fejlesztői útmutatót](development.md) (app build, telepítés, Vite/React minta).

Glint apps are React bundles loaded at runtime. Each app declares capabilities in `manifest.json` and receives a typed `PluginAPI` via `@glint/plugin-sdk`.

- **Built-in core apps** ship in the repo under `internal-apps/<id>/` (`builtin: true`) — Metrics, Browser, Achievements.
- **External apps** install to `%APPDATA%/Glint/apps/<id>/`.

For architecture, IPC channels, and Phase 3 memory APIs, see the design spec: [Plugin Platform & Engine Host](superpowers/specs/2026-07-06-plugin-platform-design.md).

---

## App package layout

External apps install under:

```
%APPDATA%/Glint/apps/<app-id>/
  manifest.json       # v2 schema (required)
  index.js            # ESM bundle, default export Plugin component
  assets/             # optional icons, images
  data/               # app-writable storage (auto-created by host)
```

Built-in core apps live in the repo:

```
internal-apps/<app-id>/
  manifest.json       # builtin: true
  dist/index.js       # Vite lib build output
```

The Electron host scans built-in `internal-apps/*/manifest.json` and user `apps/*/manifest.json` on startup, validates permissions, registers a read-only `glint-plugin://` protocol handler, and pushes manifest summaries to the renderer.

**Development workflow (external app):**

1. Create an app package under `internal-apps/<your-id>/` (see `internal-apps/metrics/` or `achievements/` as reference).
2. Implement a React component with type `PluginComponent` from `@glint/plugin-sdk`.
3. Build the bundle: `npx pnpm --filter @glint/<package-name> build`
4. Copy `manifest.json` and `dist/index.js` (as `index.js`) into `%APPDATA%/Glint/apps/<app-id>/`.

Or call `Install-DevApp` from `scripts/install-dev-plugins.ps1` for AppData installs. Built-in apps load from the repo; cloud save sync is launcher Settings → Cloud Sync (not an overlay app).

Override the scan root with `GLINT_APPS_DIR` (legacy fallback: `GLINT_PLUGINS_DIR`).

---

## manifest.json (v2)

```json
{
  "id": "my-app",
  "name": "My App",
  "version": "1.0.0",
  "entry": "./index.js",
  "builtin": false,
  "privileged": false,
  "permissions": [
    "metrics",
    "storage",
    "fs:read",
    "fs:write",
    "game:process"
  ],
  "panels": [
    {
      "id": "main",
      "title": "My Panel",
      "pinnable": true,
      "defaultPinned": false,
      "hudSlot": "top-right"
    }
  ]
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `id` | yes | Unique plugin ID; must match install directory name |
| `name` | yes | Display name in AppShell tab strip |
| `version` | yes | Semver string |
| `entry` | yes | Relative path to ESM bundle (`./index.js` external; `./dist/index.js` built-in) |
| `builtin` | no | `true` = core app from repo `internal-apps/`; cannot be overridden by user install |
| `privileged` | no | `true` = render in light DOM (required for Browser webview); default Shadow DOM |
| `permissions` | no | Capability grants (see table below); omit for no extra access |
| `panels` | yes | One or more UI panels exposed by this app |

### Panel fields

| Field | Default | Description |
|-------|---------|-------------|
| `id` | — | Panel identifier (unique within app) |
| `title` | — | Tab / HUD label |
| `pinnable` | `true` | Whether user can pin to HUD |
| `defaultPinned` | `false` | Initial pin state on first load |
| `hudSlot` | `top-right` | HUD position when pinned (`top-left`, `top-right`, `bottom-left`, `bottom-right`, `top-center`) |

### Permissions

| Permission | Capability |
|------------|------------|
| `metrics` | FPS / frametime stream via `api.game.onMetrics()` |
| `storage` | Key-value JSON in `%APPDATA%/Glint/apps/<id>/data/` |
| `fs:read` | Read plugin dir, game install dir, and resolved save roots |
| `fs:write` | Write plugin data dir and declared game save dirs |
| `fs:pick` | User folder picker (one-time grant, stored in plugin data) |
| `game:process` | PID, exe path, window title, module list |
| `game:memory:read` | Read game process memory via overlay DLL *(Phase 3)* |
| `game:memory:write` | Write game process memory *(Phase 3; requires manifest declaration + launcher warning)* |
| `game:invoke` | Call function in game thread at resolved address *(Phase 3)* |

The main process checks `manifest.permissions` before every IPC handler. Undeclared permissions are rejected.

---

## HUD vs full render modes

Apps export a single React component. The host passes a `mode` prop (`PluginRenderMode`) that tells the component which UI to render:

| Mode | When active | Where rendered |
|------|-------------|----------------|
| `hud` | Overlay closed **and** panel is pinned | `PinnedHudLayer` at the panel's `hudSlot` |
| `full` | Overlay open (Shift+Tab) | AppShell app tab area |

```typescript
import type { PluginComponent } from '@glint/plugin-sdk';

export const Plugin: PluginComponent = ({ api, mode }) => {
  if (mode === 'hud') {
    return <CompactBadge api={api} />;
  }
  return <FullPanel api={api} />;
};

export default Plugin;
```

**Pin behavior:** When the overlay is closed, pinned panels keep the overlay surface in `HudPinned` mode (transparent HUD composited in-game). Unpinning all panels returns the overlay to `Hidden` (no GPU paint). Use `api.panels.setPinned(true/false)` and `api.panels.isPinned()` to control pin state from app code. Built-in Metrics uses panel key `metrics:main`.

**Overlay chrome:** `api.overlay.open()`, `api.overlay.close()`, and `api.overlay.isOpen()` control the full AppShell.

**Privileged apps:** Browser (`privileged: true`) renders in the light DOM so the host-owned webview can attach. Control it via `api.browser.focus()`, `api.browser.blur()`, and `api.browser.setContentRect()`.

---

## App SDK quick reference

Build with Vite in **library mode**. **Externalize React** — the host provides shared shims at `glint-plugin://_shared/react.js` (see `internal-apps/metrics/vite.config.ts`). Bundling a second React copy causes hook errors (`useState` on null).

For CSS (Tailwind, shadcn, etc.), inject via `import css from './styles.css?inline'` and `window.__goQueuePluginCss(appId, css)` so styles stay inside the app Shadow DOM (or privileged host root). See [development.md](development.md#css--ui-library-tailwind-shadcn-stb).

```typescript
// vite.config.ts — external React (required)
rollupOptions: {
  external: ['react', 'react/jsx-runtime'],
  output: {
    paths: {
      react: 'glint-plugin://_shared/react.js',
      'react/jsx-runtime': 'glint-plugin://_shared/react-jsx-runtime.js',
    },
  },
},
define: { 'process.env.NODE_ENV': JSON.stringify('production') },
```

Install `@glint/plugin-sdk` as a dev dependency for types only.

```typescript
import type { PluginComponent, PluginProps } from '@glint/plugin-sdk';

// PluginProps = { api: PluginAPI; mode: 'hud' | 'full' }
```

Key API surfaces:

- `api.native.overlay.*` — overlay-core position, anchor, margin, input blocking
- `api.native.metrics.getSnapshot()` — ETW metrics snapshot
- `api.native.window.*` — overlay mode (Hidden / HudPinned / Interactive)
- `api.browser.*` — webview focus/blur/content rect (privileged browser app)
- `api.game.saves` — Ludusavi-backed save path resolution (`getSaveLocations`, `findGame`, etc.)
- `api.game.getProcessInfo()` — running game metadata (requires `game:process`)
- `api.fs.*` — gated file read/write/list (requires `fs:read` / `fs:write`)
- `api.storage.*` — persistent key-value store in app `data/` dir

---

## Walkthrough: Metrics reference app

The in-repo Metrics app (`internal-apps/metrics/`) is a small built-in example of HUD + full modes, `api.native.metrics`, and Shadow DOM CSS — without needing AppData install.

### 1. Build

```powershell
npx pnpm --filter @glint/metrics-app build
```

Output: `internal-apps/metrics/dist/index.js`

### 2. Launch and open the panel

1. Start Glint and attach to a game.
2. Press **Shift+Tab** to open the overlay.
3. Select the **Metrics** tab in the AppShell (dock).

### 3. Pin to HUD

1. With the overlay open, pin the Metrics panel (pin toggle in AppShell).
2. Close the overlay (Shift+Tab again).
3. A compact HUD view appears in the configured `hudSlot`.

### Cloud save sync (launcher)

Manual local Save Manager backups are gone. Configure cloud backup/restore in the **launcher**: **Settings → Cloud Sync** (self-host / Drive / FTP). Session-exit sync and restore use `packages/save-manifest` path resolution; in-game `game.saves.*` may still exist for other consumers.

---

## Creating a new app

1. Copy `internal-apps/metrics/` (or `achievements/`) as a starting point (or scaffold with Vite + React).
2. Update `manifest.json` — unique `id`, required permissions, panel definitions.
3. Implement your component; branch on `mode` for HUD vs full layouts.
4. Add the package to the pnpm workspace (`internal-apps/*` is already included).
5. Build and, for external apps, copy to `%APPDATA%/Glint/apps/<id>/` (or set `builtin: true` for core apps).
6. Restart or reload the overlay host to pick up new apps.

Invalid app directories (bad manifest, missing entry file) are skipped with a log message; they do not crash the host. App render errors are caught by an error boundary in `AppHost`.

---

## Related docs

- [Fejlesztői útmutató (app fejlesztés + telepítés)](development.md)
- [Plugin Platform & Engine Host design spec](superpowers/specs/2026-07-06-plugin-platform-design.md) — full architecture, IPC, SaveAPI, Phase 3 memory
