#pragma once

#include "include/cef_scheme.h"

#include <string>
#include <vector>

/** `glint-plugin://<app id>/<rel>` — the app bundle URLs `apps.list`
 *  emits, served straight from the app dirs (CEF port of the Electron
 *  `src/plugin-protocol.ts` handler). `_shared` is the React shim route. */
namespace plugin_scheme {

extern const char kScheme[];

/** Browser-process only, before CefInitialize: dirs `<app id>` is looked up in
 *  (builtin first, then user installs) and the `_shared` shim dir. */
void SetRoots(std::vector<std::wstring> app_roots, std::wstring shared_dir);

/** CefApp::OnRegisterCustomSchemes — must run in every process. */
void Register(CefRawPtr<CefSchemeRegistrar> registrar);

/** CefApp::OnContextInitialized — browser process only. */
void InstallFactory();

}  // namespace plugin_scheme
