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

## Unpacked browser extensions (best-effort)

Scope authority: [`docs/research/2026-08-16-cef-browser-extensions.md`](../../docs/research/2026-08-16-cef-browser-extensions.md) (P0/P1 only). Product OSR is dual Alloy windowless (`CreateDualOsr`: shell React + content page). Shell-only iframe (`GLINT_CEF_SHELL_ONLY=1`) is opt-in — most sites refuse framing (XFO / CSP). Extensions still load via `--load-extension` on the helper; Chrome-style satellites remain for options/popup.

**Supported:** drop unpacked **MV3** folders under `%APPDATA%\Glint\BrowserExtensions\<id>\` (each must contain `manifest.json`), **or** use overlay Ext panel / `__goHost` `browser.extensions.installFromStore` with a Chrome Web Store URL or 32-char id (downloads CRX via Google’s update2 redirect, unpacks, fail closed). On next CEF helper start, `glint-cef` appends Chromium `--load-extension` with those absolute paths. **Restart the helper** (or the overlay session that spawns it) after adding/removing folders — there is no runtime `LoadExtension` API on CEF 150.

**Enablement prefs:** `%APPDATA%\Glint\browser-extensions.json` — same shape as apps enablement (`{ "<id>": false }` disables; absent/`true` = enabled). Host methods `browser.extensions.list` / `browser.extensions.setEnabled` update this file; CEF omits disabled ids from `--load-extension` on the **next** start (`restartRequired` from setEnabled / installFromStore).

**Not supported / honesty:** In-page Chrome Web Store “Add to Chrome” / “Switch to Chrome?” on Alloy windowless OSR is **not** a Glint install path — use product unpack. Action toolbar icons and tab-bound popups are best-effort satellites only. Prefer content-script-only (or other non–Chrome-UI) extensions; each package must be validated on this path.

**ToS / fragility:** `installFromStore` uses an **unofficial** clients2.google.com CRX redirect. Google may change or block it; downloads can fail without notice. Glint does not claim Chrome Web Store partnership or branded Chrome. Fail closed on fetch/unpack errors — overlay UI must not blank.

**QA sample:** `host/cef/testdata/extensions/glint-spike-cs/` — copy into `%APPDATA%\Glint\BrowserExtensions\glint-spike-cs\`, restart helper, navigate content to `https://example.com/`, then check `window.__glintExtSpike === true` or `document.documentElement.getAttribute('data-glint-ext-spike') === '1'`.

## OSR topology (dual default)

- **Default:** `CreateDualOsr` — shell React (`file://` AppShell) + content page OSR; Present stamps shell layer 0 + content ≥1 into the React hole (`setContentRect`).
- **Opt-in shell-only:** `GLINT_CEF_SHELL_ONLY=1` — one CreateBrowser + iframe page (most sites refuse framing).
- **Legacy spike:** `GLINT_CEF_SINGLE_CONTENT_OSR=1` creates content-only OSR (blanks Interactive shell — not product).
- **Mouse:** ShellOnly must not use the legacy `chrome_top_px`→Content Y-strip (hit rides high); dual uses the content hole.