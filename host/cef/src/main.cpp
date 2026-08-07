#include "browser_app.h"
#include "ipc_server.h"
#include "plugin_scheme.h"

#include "include/cef_app.h"
#include "include/cef_command_line.h"
#include "include/wrapper/cef_helpers.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <shellapi.h>

#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

namespace {

struct HelperArgs {
  uint16_t port = 0;
  unsigned long parent_pid = 0;
  unsigned long luid_low = 0;
  long luid_high = 0;
  /** `glint-plugin://` roots — repeatable, host order = lookup order. */
  std::vector<std::wstring> plugin_roots;
  std::wstring plugin_shared;
};

HelperArgs ParseArgs(int argc, wchar_t** argv) {
  HelperArgs out;
  for (int i = 1; i + 1 < argc; ++i) {
    const std::wstring key(argv[i]);
    if (key == L"--port") {
      out.port = static_cast<uint16_t>(_wtoi(argv[i + 1]));
    } else if (key == L"--parent-pid") {
      out.parent_pid = wcstoul(argv[i + 1], nullptr, 10);
    } else if (key == L"--luid-low") {
      out.luid_low = wcstoul(argv[i + 1], nullptr, 10);
    } else if (key == L"--luid-high") {
      out.luid_high = _wtol(argv[i + 1]);
    } else if (key == L"--plugin-root") {
      out.plugin_roots.emplace_back(argv[i + 1]);
    } else if (key == L"--plugin-shared") {
      out.plugin_shared = argv[i + 1];
    }
  }
  return out;
}

}  // namespace

int APIENTRY wWinMain(HINSTANCE hInstance,
                      HINSTANCE /*hPrev*/,
                      LPTSTR /*lpCmdLine*/,
                      int /*nCmdShow*/) {
  CefMainArgs main_args(hInstance);

  void* sandbox_info = nullptr;

  // Same CefApp in render processes so window.goBrowser V8 binding is installed.
  CefRefPtr<BrowserApp> app(new BrowserApp());
  int exit_code = CefExecuteProcess(main_args, app.get(), sandbox_info);
  if (exit_code >= 0) {
    return exit_code;
  }

  int argc = 0;
  LPWSTR* argv = CommandLineToArgvW(GetCommandLineW(), &argc);
  const HelperArgs args = ParseArgs(argc, argv);
  if (argv) LocalFree(argv);
  if (args.port == 0 || args.parent_pid == 0) {
    return 2;
  }

  app->SetShareParams(args.parent_pid, args.luid_low, args.luid_high);
  plugin_scheme::SetRoots(args.plugin_roots, args.plugin_shared);

  // Browser process only (subprocesses returned above). Its single UI thread
  // dispatches injected input *and* runs the OSR composite, so at the default
  // NORMAL class it loses quanta to games running ABOVE_NORMAL/HIGH and hover
  // feedback arrives late. ABOVE_NORMAL is deliberate: HIGH measurably costs
  // the game frames, which is a worse trade than slightly late hover.
  SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);

  CefSettings settings;
  settings.no_sandbox = true;
  settings.windowless_rendering_enabled = true;
  settings.multi_threaded_message_loop = false;

  // Prefer one stable profile so cache/cookies/localStorage survive restarts —
  // a fresh per-PID dir cold-starts every page load. Chromium cannot share a
  // profile across processes, so claim it with a named mutex and fall back to a
  // per-PID dir when another overlay browser already owns it.
  HANDLE profile_lock =
      CreateMutexW(nullptr, TRUE, L"Local\\glint-cef-profile");
  const bool own_shared_profile =
      profile_lock != nullptr && GetLastError() != ERROR_ALREADY_EXISTS;
  const std::wstring profile_leaf =
      own_shared_profile ? L"shared" : std::to_wstring(args.parent_pid);

  wchar_t local_app_data[MAX_PATH] = {};
  std::wstring root_cache;
  if (GetEnvironmentVariableW(L"LOCALAPPDATA", local_app_data, MAX_PATH) > 0) {
    root_cache =
        std::wstring(local_app_data) + L"\\Glint\\cef-user-data\\" + profile_leaf;
    CreateDirectoryW((std::wstring(local_app_data) + L"\\Glint").c_str(),
                     nullptr);
    CreateDirectoryW(
        (std::wstring(local_app_data) + L"\\Glint\\cef-user-data").c_str(),
        nullptr);
  } else {
    root_cache = L".\\cef-user-data\\" + profile_leaf;
  }
  CreateDirectoryW(root_cache.c_str(), nullptr);
  CefString(&settings.root_cache_path).FromWString(root_cache);
  CefString(&settings.cache_path).FromWString(root_cache + L"\\Default");

  // IPC must be live before CefInitialize: OnContextInitialized runs during init
  // and drops ready if ipc_ is still null. Ready is sent only from there.
  IpcClient ipc;
  app->SetIpc(&ipc);
  if (!ipc.Start(args.port, [app](const gameoverlay::cef::Envelope& env) {
        app->HandleEnvelope(env);
      })) {
    return 3;
  }

  if (!CefInitialize(main_args, settings, app.get(), sandbox_info)) {
    ipc.Stop();
    return 1;
  }

  CefRunMessageLoop();

  ipc.Stop();
  app->ShutdownBrowser();
  CefShutdown();
  if (profile_lock) {
    ReleaseMutex(profile_lock);
    CloseHandle(profile_lock);
  }
  return 0;
}
