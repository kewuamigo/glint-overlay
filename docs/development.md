# Fejlesztői útmutató — App fejlesztés

Ez a dokumentum leírja, hogyan épül fel a Glint monorepo, hogyan fejlesztesz **appokat** (built-in core + külső), valamint hogyan telepíted őket futásidőben.

**Egyetlen fejlesztői modell:** minden funkció = `manifest.json` + ESM bundle + `@glint/plugin-sdk`.

Részletesebb architektúra: [ARCHITECTURE.md](ARCHITECTURE.md)

---

## Előfeltételek

| Eszköz | Verzió | Megjegyzés |
|--------|--------|------------|
| Windows | 10/11 | A platform jelenleg csak Windows |
| Node.js | 22+ | pnpm workspace |
| pnpm | 10.12.1 | `packageManager` a root `package.json`-ban |
| Rust | 1.85+ | launcher, overlay DLL, injektor |
| MSVC Build Tools | ajánlott | overlay DLL build (`scripts/build-overlay.ps1`) |

```powershell
# Repo gyökérből
npx pnpm@10.12.1 install
.\build-all.ps1
```

**Futtatás (Admin):**

```powershell
.\target\release\glint-launcher.exe
```

Játékban **Shift+Tab** nyitja az interaktív overlay-t.

---

## App típusok

| Típus | Hol él | Telepítés | Példa |
|-------|--------|-----------|-------|
| **Built-in core app** | `internal-apps/<id>/` a repóban (`builtin: true`) | `npm run build` (dist bundle) | Metrics, Browser, Achievements |
| **Külső app** | `%APPDATA%/Glint/apps/<id>/` | manuális másolás vagy `Install-DevApp` a `scripts/install-dev-plugins.ps1`-ben | saját AppData plugin |
| **Host / engine** | `host/*`, `ui/*`, `packages/*`, `core/*` | `build-all.ps1` | Launcher Electron, CEF helper, UI shell, Rust overlay |

A külső appok **nem** kerülnek automatikusan az AppData-ba build után — külön telepíteni kell őket (lásd lent).

Built-in appok a host induláskor a repó `internal-apps/` mappájából töltődnek (`scanApps()`). Külső appok az AppData `apps/` mappájából.

---

## Monorepo felépítés

```
.
├── host/
│   ├── launcher/             # Launcher Electron wrapper (out-of-game)
│   ├── native/               # Staged natives (DLL, browser, cef)
│   └── cef/                  # glint-cef + plugin-shared React shims
├── ui/
│   ├── shell/                # Overlay shell (tab sáv, AppRuntime, Shadow DOM)
│   └── launcher/             # Launcher React UI
├── sdk/
│   ├── plugin/               # App típusok + PluginAPI (api.native.*, api.browser.*)
│   └── bridge/               # Bridge types + hostInvoke / pluginInvoke
├── packages/
│   └── ...                   # domain libek (achievements-core, static-server, …)
├── internal-apps/
│   ├── metrics/              # Built-in core app (Metrics tab + HUD)
│   ├── browser/              # Built-in privileged core app (Browser + content hole)
│   └── achievements/         # Built-in core app (Achievements + unlock toasts)
└── core/                     # Rust: overlay/ (incl. browser-host), metrics/, inject/, launcher/, tools/
```

Cloud save sync is **launcher-only** (Settings → Cloud Sync), not an overlay app. Path resolution stays in `packages/save-manifest`.
**pnpm workspace** (`pnpm-workspace.yaml`): `host/*`, `packages/*`, `sdk/*`, `ui/*`, `internal-apps/*`.

---

## Core app fejlesztés (monorepo)

### Mit módosíthatsz tipikusan?

| Cél | Fő fájlok / csomagok |
|-----|----------------------|
| Overlay shell (tab sáv, pinned HUD) | `AppShell.tsx`, `PinnedHudLayer.tsx`, `AppRuntimeContext.tsx` |
| App betöltés, Shadow DOM izoláció | `AppHost.tsx`, `PluginSurface.tsx`, `plugin-styles.ts` |
| App registry (built-in + AppData) | `core/overlay/browser-host/src/apps.rs` |
| SDK + IPC bridge (native, browser) | `sdk/plugin/`, `sdk/bridge/`, `browser-host` bridge |
| Rust hook / render | `core/overlay/overlay-core/`, `core/overlay/overlay-dll/` |

### Build parancsok

| Parancs | Mit buildel |
|---------|-------------|
| `.\build-all.ps1` | Rust release + overlay DLL + összes aktív Node csomag |
| `npm run build` | Csak Node/TS csomagok (gyors iteráció) |
| `npm run build:rust` | `cargo build --release` |
| `npx pnpm --filter @glint/shell build` | Overlay shell csomag |
| `.\scripts\build-browser.ps1` | CEF UI helper → `host/native/glint-browser.exe` |

### Fejlesztési ciklus (UI)

1. Módosítod a `ui/shell` (vagy bridge / browser-host) kódot.
2. Build:

   ```powershell
   npx pnpm --filter @glint/shell build
   # ha Rust bridge / session változott:
   .\scripts\build-browser.ps1
   ```

3. **Indítsd újra** a launchert, majd attacholj újra (a UI statikus bundle a `ui/shell/dist`-ből jön).

UI hot-reload csak önálló Vite dev szerverrel (`npm run dev` a `ui/shell`-ban) — teljes in-game teszthez rebuild + restart kell.

### Mit **nem** kell AppData-ba másolni?

A core app (`ui/shell`, Rust DLL-ek, `glint-browser`) a repó build outputjaiból fut. Ezek **git-ben** / stage-elve vannak, nem user-szintű telepítésként.

---

## App fejlesztés

### Telepített app könyvtár (külső)

```
%APPDATA%/Glint/apps/<app-id>/
  manifest.json       # kötelező
  index.js            # ESM bundle (entry)
  assets/             # opcionális
  data/               # host hozza létre — storage, backup, stb.
```

Built-in appok a repóban:

```
internal-apps/<app-id>/
  manifest.json       # builtin: true
  dist/index.js         # Vite lib build output
```

A host induláskor beolvassa a built-in `internal-apps/*/manifest.json` fájlokat és az AppData `apps/*/manifest.json` fájlokat, validálja az engedélyeket, regisztrálja a `glint-plugin://` protokollt, és dinamikusan importálja a bundle-t.

Egyéni app mappa (teszt):

```powershell
$env:GLINT_APPS_DIR = "D:\my-apps"
# Legacy fallback (átmeneti):
# $env:GLINT_PLUGINS_DIR = "D:\my-apps"
```

### manifest.json

```json
{
  "id": "my-app",
  "name": "My App",
  "version": "1.0.0",
  "entry": "./index.js",
  "builtin": false,
  "privileged": false,
  "permissions": ["metrics", "storage", "fs:read"],
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

| Mező | Kötelező | Leírás |
|------|----------|--------|
| `id` | igen | Egyedi azonosító; **meg kell egyeznie** a mappa nevével (`^[a-z0-9-]+$`) |
| `name` | igen | Megjelenő név a tab sávban |
| `version` | igen | Semver |
| `entry` | igen | Relatív útvonal az ESM bundle-höz (`./index.js` külsőnél; built-innél `./dist/index.js`) |
| `builtin` | nem | `true` = core app a repó `internal-apps/` mappájából; nem törölhető |
| `privileged` | nem | `true` = light DOM (pl. Browser webview); alapból Shadow DOM |
| `permissions` | nem | Lásd lent |
| `panels` | igen | Legalább egy panel |

**Engedélyek:** `metrics`, `storage`, `fs:read`, `fs:write`, `fs:pick`, `game:process`, `game:memory:read`, `game:memory:write`, `game:invoke` (utóbbi három Phase 3).

### App komponens

```tsx
import type { PluginComponent } from '@glint/plugin-sdk';

export const Plugin: PluginComponent = ({ api, mode }) => {
  if (mode === 'hud') {
    return <div>HUD badge</div>;
  }
  return <div>Full panel</div>;
};

export default Plugin;
```

| `mode` | Mikor | Hol jelenik meg |
|--------|-------|-----------------|
| `full` | Overlay nyitva (Shift+Tab) | AppShell app terület |
| `hud` | Overlay zárva **és** panel pinelve | `PinnedHudLayer` a `hudSlot` pozícióban |

### Új app létrehozása (lépések)

1. **Másold** a `internal-apps/metrics/` (vagy `achievements/`) mappát → `internal-apps/<your-id>/`.
2. Frissítsd a `manifest.json`-t (`id`, `permissions`, `panels`).
3. Add hozzá / frissítsd a `package.json` `name` mezőjét (`@glint/<your-id>`).
4. Implementáld a React UI-t; használd az `api` objektumot (`@glint/plugin-sdk` típusok).
5. Build:

   ```powershell
   npx pnpm --filter @glint/<your-id> build
   ```

6. **Telepítés** AppData-ba (lásd „Telepítés” szekció) — vagy `builtin: true` esetén elég a repo dist.
7. **Indítsd újra** az overlay hostot.

### Vite konfiguráció (fontos)

A app **megosztott Reactet** használ — ne bundle-eld a Reactet, különben `useState` / hook hibák lesznek.

```ts
// internal-apps/<id>/vite.config.ts
export default defineConfig({
  define: {
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
  build: {
    lib: {
      entry: 'src/index.tsx',
      formats: ['es'],
      fileName: () => 'index.js',
    },
    rollupOptions: {
      external: ['react', 'react/jsx-runtime'],
      output: {
        paths: {
          react: 'glint-plugin://_shared/react.js',
          'react/jsx-runtime': 'glint-plugin://_shared/react-jsx-runtime.js',
        },
      },
    },
  },
});
```

A host a `glint-plugin://_shared/*` útvonalon szolgálja ki a React shimeket (`host/cef/plugin-shared/`).

### CSS / UI library (Tailwind, shadcn, stb.)

A app CSS **izolált Shadow DOM-ban** fut (kivéve `privileged: true` appok, pl. Browser) — nem szivárog a host UI-ba.

**Ajánlott minta** (`internal-apps/metrics/src/index.tsx`):

```tsx
import pluginCss from './styles.css?inline';

const APP_ID = 'my-app'; // egyezzen a manifest id-vel

if (typeof window !== 'undefined' && window.__goQueuePluginCss) {
  window.__goQueuePluginCss(PLUGIN_ID, pluginCss);
}
```

- Importáld a stílusokat `?inline`-nal, ne `document.head`-be injektálj közvetlenül.
- A `APP_ID` legyen beégetve — párhuzamos app betöltésnél is helyes marad.
- UI wrapper: `.my-app-root` scope Tailwind változóknak.

### App API (rövid referencia)

```ts
api.pluginId / api.panelId
api.panels.setPinned(bool) / api.panels.isPinned()
api.overlay.open() / close() / isOpen()
api.native.overlay.*          // overlay-core: position, anchor, blockInput, …
api.native.metrics.getSnapshot()
api.native.window.*           // Hidden / HudPinned / Interactive
api.browser.*                 // focus, blur, setContentRect (privileged browser app)
api.game.targetPid
api.game.getProcessInfo()          // game:process
api.game.onMetrics(cb)             // metrics
api.game.saves.getSaveLocations()  // Ludusavi (host-side)
api.fs.readText / writeText / listDir / isDirectory / …
api.storage.get / set / remove     // storage → apps/<id>/data/
```

Teljes típusok: `sdk/plugin/src/index.ts`.

---

## Telepítés

### Core app (fejlesztői build)

```powershell
.\build-all.ps1
# Admin:
.\target\release\glint-launcher.exe
```

Nincs külön „telepítő” — a launcher a repo `target/release` és `host/native/` stage outputját használja.

### Külső app — manuális telepítés

```powershell
$id = "my-app"
$dest = "$env:APPDATA\Glint\apps\$id"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item "internal-apps\my-app\manifest.json" $dest\
Copy-Item "internal-apps\my-app\dist\index.js" $dest\
```

Indítsd újra az overlay-t. Új app = új mappa + manifest + bundle.

### Külső app — dev install script

A `scripts/install-dev-plugins.ps1` tartalmaz egy `Install-DevApp` helper-t AppData telepítéshez. Built-in appok (`metrics`, `browser`, `achievements`) a repo `internal-apps/`-ból töltődnek — nem kell AppData-ba másolni.

Cloud save backup/restore: **launcher Settings → Cloud Sync** (nem overlay tab).

Saját külső apphoz hívd meg az `Install-DevApp`-ot a scriptben, vagy használd a manuális másolást fent.

---

## Gyakori hibák

| Tünet | Ok | Megoldás |
|-------|-----|----------|
| `process is not defined` | Node globals a plugin bundle-ben | `define: { 'process.env.NODE_ENV': … }` a Vite-ban |
| `Cannot read properties of null (reading 'useState')` | Két React példány | Externalizáld a Reactet (`_shared` shimek) |
| Plugin tab nem jelenik meg | Rossz `id` / hiányzó fájl | Mappa neve = `manifest.id`; legyen `index.js` (külső) vagy `dist/index.js` (built-in) |
| `JSON.parse` hiba storage-nál | UTF-8 BOM a `store.json`-ban | UTF-8 **BOM nélkül** írj (lásd install script) |
| Host UI megváltozik plugin CSS-től | CSS a `document.head`-be került | `?inline` + `__goQueuePluginCss` (Shadow DOM) |
| Változások nem látszanak | Nem restartoltad / nem attacholtál újra | UI + browser-host rebuild, launcher restart, friss inject |

---

## Ajánlott workflow összefoglalva

### Core feature (host / UI)

1. Branch → módosítás `ui/*`, `sdk/*`, vagy `core/overlay/browser-host`
2. `npx pnpm --filter @glint/shell build` (+ `build-browser.ps1` ha kell)
3. Launcher restart + újra-attach
4. Commit / push a monorepóba

### App feature

1. `internal-apps/<id>/` fejlesztés
2. `npx pnpm --filter @glint/<id> build`
3. Built-in: restart + újra-attach; külső: másolás AppData `apps/`-ba **vagy** `Install-DevApp` (`scripts/install-dev-plugins.ps1`)
4. Launcher restart
5. Disztribúció: zip a `%APPDATA%/Glint/apps/<id>/` struktúrával

---

## Kapcsolódó dokumentumok

- [plugin-development.md](plugin-development.md) — app-specifikus részletek (manifest, native API, plugin walkthrough)
- [ARCHITECTURE.md](ARCHITECTURE.md) — rendszerarchitektúra
