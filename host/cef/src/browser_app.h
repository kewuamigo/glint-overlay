#pragma once

#include "include/cef_app.h"
#include "include/cef_browser.h"
#include "include/cef_render_process_handler.h"

#include "osr_client.h"
#include "shared_tex.h"

#include "ipc/gen/cef_ipc.pb.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <string>

class IpcClient;

class BrowserApp : public CefApp,
                   public CefBrowserProcessHandler,
                   public CefRenderProcessHandler {
 public:
  BrowserApp();

  CefRefPtr<CefBrowserProcessHandler> GetBrowserProcessHandler() override {
    return this;
  }
  CefRefPtr<CefRenderProcessHandler> GetRenderProcessHandler() override {
    return this;
  }
  void OnBeforeCommandLineProcessing(
      const CefString& process_type,
      CefRefPtr<CefCommandLine> command_line) override;
  void OnRegisterCustomSchemes(CefRawPtr<CefSchemeRegistrar> registrar) override;
  void OnContextInitialized() override;

  void OnContextCreated(CefRefPtr<CefBrowser> browser,
                        CefRefPtr<CefFrame> frame,
                        CefRefPtr<CefV8Context> context) override;

  void HandleEnvelope(gameoverlay::cef::Envelope env);
  void SetIpc(IpcClient* ipc) { ipc_ = ipc; }
  void SetShareParams(unsigned long parent_pid, unsigned long luid_low, long luid_high) {
    parent_pid_ = parent_pid;
    luid_low_ = luid_low;
    luid_high_ = luid_high;
    publisher_.Configure(static_cast<DWORD>(parent_pid), static_cast<LONG>(luid_low),
                         static_cast<LONG>(luid_high));
  }
  void ShutdownBrowser();

  // Shared internals (proto dispatch).
  void CreateSessionOsr(int w, int h, const std::string& url);
  void ApplySetSurfaceSize(int w, int h);
  void ApplySetInnerBounds(int x, int y, int w, int h, uint32_t layout_generation);
  void ApplySetContentRect(bool clear, int x, int y, int w, int h);
  void ApplySetFocus(bool focus, gameoverlay::cef::FocusTarget target);
  void ApplySetHidden(bool hidden);
  void Navigate(const std::string& url);
  void ContentNavigate(const std::string& url);
  void GoBack();
  void GoForward();
  void Reload();
  /** Chrome-style satellite for extension options/popup — never converts content_. */
  void OpenExtensionSatellite(const std::string& extension_id,
                              gameoverlay::cef::ExtensionSatelliteKind kind);
  void CloseExtensionSatellite();
  void ShutdownSession(bool quit_message_loop);
  /** `__goHost` bridge (host → page): settle one invoke / deliver one push. */
  void DeliverBridgeResult(uint32_t request_id, bool ok, const std::string& result_json,
                           const std::string& error);
  void DeliverBridgePush(const std::string& message_json);
  CefRefPtr<OsrClient> ClientForFocusTarget(gameoverlay::cef::FocusTarget target);
  void InjectMouse(CefRefPtr<OsrClient> t, const char* type, int x, int y,
                   int click_count, uint32_t modifiers,
                   cef_mouse_button_type_t button);
  void InjectKey(CefRefPtr<OsrClient> t, const char* type, uint32_t modifiers,
                 int windows_key_code, int native_key_code, bool is_system_key,
                 const std::string& character);
  void InjectWheel(CefRefPtr<OsrClient> t, int x, int y, int dx, int dy,
                   uint32_t modifiers);
  bool IpcNegotiated() const;

 private:
  void CreateBrowser(int w, int h, const std::string& url, bool hwnd_mode, HWND parent);
  /** Product default: one shell CreateBrowser (AppShell + iframe page). */
  void CreateShellOnlyOsr(int w, int h, const std::string& chrome_url);
  /** Rollback dual OSR (`GLINT_CEF_DUAL_OSR=1`). */
  void CreateDualOsr(int w, int h, const std::string& chrome_url);
  /** Legacy spike: one content windowless OSR only (no chrome_). */
  void CreateContentOnlyOsr(int w, int h);
  /** Env `GLINT_CEF_SPIKE_CHROME_SATELLITE=1`: one Chrome-style popup beside dual OSR. */
  void MaybeSpawnChromeStyleSatelliteSpike();
  /** Create/replace Chrome-style windowed satellite (nullptr ipc, OsrRole::Chrome). */
  void OpenChromeStyleSatellite(const std::string& url);
  void CloseChromeStyleSatellite();
  void LayoutContentHole();
  void ApplyInnerWindow();
  void PushWindowBoundsToUi();
  void SetSurfaceSize(int w, int h);
  void SetInnerWindow(int x, int y, int w, int h);
  void OnOsrPaint(OsrRole role, HANDLE shared_handle, uint32_t w, uint32_t h,
                  const SharedDirtyRect* dirty, size_t dirty_n);
  void SendPaint(uint32_t layer, uint32_t w, uint32_t h, uint64_t nt_handle);
  void HandleChromeMessage(const std::string& json);
  void PushNavToChrome(const std::string& nav_json);
  /** Shell-only: set `#glint-browser-content` iframe src + synthesize navState. */
  void NavigateShellIframe(const std::string& url);
  void ShellIframeHistory(const char* op);
  void FlushPendingContentNavigate();
  void FlushPendingFocus();
  /** Layout CEF child inside Electron parent (client coords). No TOPMOST chase. */
  void SetBounds(int x, int y, int w, int h, HWND parent);
  void SetVisible(bool visible);
  void DestroyOwnedHost();
  void SendPaintError(const char* msg);
  void SendHostUiAction(gameoverlay::cef::HostUiActionKind kind);

  IpcClient* ipc_ = nullptr;
  SharedTexPublisher publisher_;
  /** Shell host document OSR (`file://` AppShell). Null on content-only spike. */
  CefRefPtr<OsrClient> chrome_;
  /** Dual-OSR page peer only (`GLINT_CEF_DUAL_OSR`). Null on product shell-only. */
  CefRefPtr<OsrClient> content_;
  /** HWND / legacy single-client path. */
  CefRefPtr<OsrClient> client_;
  /** Chrome-style satellite (not OSR; not content_; nullptr ipc). */
  CefRefPtr<OsrClient> satellite_;
  bool created_ = false;
  bool hwnd_mode_ = false;
  /** Chrome OSR / publish texture size (game/fullscreen). */
  int chrome_w_ = 0;
  int chrome_h_ = 0;
  /** Inner browser window within the fullscreen chrome surface. */
  int window_x_ = 0;
  int window_y_ = 0;
  int window_w_ = 0;
  int window_h_ = 0;
  int content_x_ = 0;
  int content_y_ = 0;
  int content_w_ = 0;
  int content_h_ = 0;
  /** True when host set an arbitrary shell React hole (not kChromeTopPx strip). */
  bool content_rect_override_ = false;
  /** When true, skip content-layer paint so chrome newtab UI stays visible. */
  bool content_blank_ = true;
  /** content_navigate before content browser OnAfterCreated — never silent-drop. */
  std::string pending_content_url_;
  /** set_focus before the target browser OnAfterCreated — never silent-drop. */
  bool pending_focus_ = false;
  gameoverlay::cef::FocusTarget pending_focus_target_ =
      gameoverlay::cef::FOCUS_TARGET_UNSPECIFIED;
  /** file:// chrome UI URL. */
  std::string chrome_url_;
  /** Electron (or other) parent — CEF is SetAsChild of this HWND. */
  HWND parent_hwnd_ = nullptr;
  /** Message-only parent for windowless OSR (never on-screen / no desktop mirror). */
  HWND owned_host_ = nullptr;
  unsigned long parent_pid_ = 0;
  unsigned long luid_low_ = 0;
  long luid_high_ = 0;
  bool context_initialized_ = false;
  bool ready_sent_ = false;
  /** Echoed on Paint; last applied SetInnerBounds generation (0 = legacy). */
  uint32_t layout_generation_ = 0;
  /** Last JS-applied window bounds; skip redundant PushWindowBoundsToUi. */
  int last_js_x_ = 0;
  int last_js_y_ = 0;
  int last_js_w_ = 0;
  int last_js_h_ = 0;

  void MaybeSendReady();

  IMPLEMENT_REFCOUNTING(BrowserApp);
};
