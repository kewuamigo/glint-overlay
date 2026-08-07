# Overlay architektúra — tartsd egyszerűen

**Ez a fájl a kanonikus útmutató.** Ha bármi ütközik vele, előbb egyszerűsíts, ne adj hozzá új réteget.

## Egy mondatban

```
Launcher (Electron) ──attach──► overlay DLL + glint-browser (CEF)
                                      └──► [egy bridge: __goHost] ──► React UI
```

## Szabály #1: Ne bonyolítsd túl

- **Egy bridge global:** `__goHost` — invoke + push. Nincs párhuzamos UI IPC.
- **Új funkció?** Új `method` string a meglévő invoke-on. Nem új channel / global.
- **Plugin IPC** = ugyanaz az invoke `pluginId`-vel (`pluginInvoke`).
- **Ne duplikáld** ugyanazt az adatot két úton (bridge + DOM olvasás a hostból).
- **Browser navigáció** a session owneron megy CEF felé: `browser.navigate` /
  `reload` / `goBack` / `goForward`.
- **Browser dock** (`browser.openSession`) = ugyanaz a `glint-browser`
  helper + ugyanaz az overlay layer — nincs második exe.
- **Input** a DLL event sink → browser-host (és onnan CEF).

A launcher megtarthatja a saját Electron IPC-jét (out-of-game UI); az **in-game**
út nem Electron overlay host.

## Adatfolyam

```
Game DLL ◄──named pipe (-cef)──► glint-browser
                                      │
                      ┌───────────────┼───────────────┐
                      ▼               ▼               ▼
                 input/mode      __goHost.invoke   push / postMessage
                      │               │               │
                      ▼               ▼               ▼
                 CEF OSR          bridge.rs        React (ui/shell)
                      ▲
                      └── glint-cef.exe
```

## `__goHost.invoke` — egy dispatcher, két útvonal

```js
__goHost.invoke(method, args, pluginId?)
```

| pluginId | Útvonal | Példa method |
|----------|---------|--------------|
| nincs | Host / shell | `apps.list`, `panel.setPinned`, `browser.navigate`, `ui.toggleInteractive` |
| van | Plugin sandbox | `native.metrics.getSnapshot`, … — CEF: `plugin_ipc::dispatch` |

Renderer helperök (`@glint/overlay-bridge`):

- `hostInvoke(method, args)` — UI
- `pluginInvoke(pluginId, method, args)` — plugin (ugyanaz a bridge, pluginId-vel)

## Push — host → UI

Host → CEF message router → `postMessage` → React.

Típusok (szerződés szerint): `metrics`, `connection`, `chrome`, `apps`,
`browser.navState`, `achievement`, …

## Mit **ne** csinálj

- ❌ Új UI IPC channel / második bridge global
- ❌ Második game↔host pipe protokoll az `overlay-common` mellé
- ❌ `executeJavaScript` / DOM olvasás a hostból — UI pusholjon state-et
- ❌ Electron in-game overlay host visszaépítése
- ❌ Dupla input handler

## Fájlok

| Mi | Hol |
|----|-----|
| Channel / bridge típusok | `sdk/bridge/` |
| In-game invoke dispatch | `core/overlay/browser-host/src/bridge.rs` (+ `main.rs` session methods) |
| App registry / enablement | `core/overlay/browser-host/src/apps.rs` |
| UI shell | `ui/shell/src/` |
| CEF helper + plugin scheme | `host/cef/` |
| React shims (`_shared`) | `host/cef/plugin-shared/` |
| Session ownership contract | `specs/002-cef-overlay-host/contracts/session-ownership.md` |
| Host bridge contract | `specs/002-cef-overlay-host/contracts/host-bridge.md` |
