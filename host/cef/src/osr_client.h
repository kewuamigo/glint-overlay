#pragma once

#include "include/cef_client.h"
#include "include/cef_life_span_handler.h"
#include "include/cef_load_handler.h"
#include "include/cef_render_handler.h"
#include "include/cef_display_handler.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <functional>
#include <mutex>
#include <string>

class IpcClient;

enum class OsrRole { Chrome, Content };

/** Accelerated paint → host compositor (role, handle, w, h). */
using OsrPaintFn = std::function<void(OsrRole role, HANDLE shared_handle, uint32_t w,
                                      uint32_t h)>;
/** Chrome UI process messages (goBrowser JSON). */
using OsrChromeMsgFn = std::function<void(const std::string& json)>;
/** Content navState JSON (also mirrored into chrome UI by BrowserApp). */
using OsrNavStateFn = std::function<void(const std::string& json)>;
/** Fired once browser_ is set (flush pending content_navigate). */
using OsrAfterCreatedFn = std::function<void()>;
/** Chrome main-frame finished loading (re-apply #app bounds). */
using OsrLoadEndFn = std::function<void()>;

class OsrClient : public CefClient,
                  public CefRenderHandler,
                  public CefLifeSpanHandler,
                  public CefLoadHandler,
                  public CefDisplayHandler {
 public:
  OsrClient(IpcClient* ipc, OsrRole role);

  CefRefPtr<CefRenderHandler> GetRenderHandler() override {
    return windowed_ ? nullptr : this;
  }
  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return this; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return this; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return this; }

  void SetWindowed(bool windowed) { windowed_ = windowed; }
  void SetPaintHandler(OsrPaintFn fn) { paint_fn_ = std::move(fn); }
  void SetChromeMessageHandler(OsrChromeMsgFn fn) { chrome_msg_fn_ = std::move(fn); }
  void SetNavStateHandler(OsrNavStateFn fn) { nav_fn_ = std::move(fn); }
  void SetAfterCreatedHandler(OsrAfterCreatedFn fn) { after_created_fn_ = std::move(fn); }
  void SetLoadEndHandler(OsrLoadEndFn fn) { load_end_fn_ = std::move(fn); }
  OsrRole role() const { return role_; }

  void GetViewRect(CefRefPtr<CefBrowser> browser, CefRect& rect) override;
  void OnPaint(CefRefPtr<CefBrowser> browser,
               PaintElementType type,
               const RectList& dirtyRects,
               const void* buffer,
               int width,
               int height) override;
  void OnAcceleratedPaint(CefRefPtr<CefBrowser> browser,
                          PaintElementType type,
                          const RectList& dirtyRects,
                          const CefAcceleratedPaintInfo& info) override;
  bool GetScreenInfo(CefRefPtr<CefBrowser> browser, CefScreenInfo& screen_info) override;

  void OnAfterCreated(CefRefPtr<CefBrowser> browser) override;
  void OnBeforeClose(CefRefPtr<CefBrowser> browser) override;
  bool OnBeforePopup(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefFrame> frame,
                     int popup_id,
                     const CefString& target_url,
                     const CefString& target_frame_name,
                     WindowOpenDisposition target_disposition,
                     bool user_gesture,
                     const CefPopupFeatures& popupFeatures,
                     CefWindowInfo& windowInfo,
                     CefRefPtr<CefClient>& client,
                     CefBrowserSettings& settings,
                     CefRefPtr<CefDictionaryValue>& extra_info,
                     bool* no_javascript_access) override;

  void OnLoadingStateChange(CefRefPtr<CefBrowser> browser,
                            bool isLoading,
                            bool canGoBack,
                            bool canGoForward) override;

  void OnTitleChange(CefRefPtr<CefBrowser> browser, const CefString& title) override;
  void OnAddressChange(CefRefPtr<CefBrowser> browser,
                       CefRefPtr<CefFrame> frame,
                       const CefString& url) override;

  bool OnProcessMessageReceived(CefRefPtr<CefBrowser> browser,
                                CefRefPtr<CefFrame> frame,
                                CefProcessId source_process,
                                CefRefPtr<CefProcessMessage> message) override;

  CefRefPtr<CefBrowser> browser() const { return browser_; }
  void SetSize(int width, int height);
  void SetHidden(bool hidden);
  void EmitNavState();

 private:
  IpcClient* ipc_;
  OsrRole role_;
  OsrPaintFn paint_fn_;
  OsrChromeMsgFn chrome_msg_fn_;
  OsrNavStateFn nav_fn_;
  OsrAfterCreatedFn after_created_fn_;
  OsrLoadEndFn load_end_fn_;
  CefRefPtr<CefBrowser> browser_;
  int width_ = 1280;
  int height_ = 720;
  bool windowed_ = false;
  bool hidden_ = false;
  std::string title_;
  std::string url_;
  bool loading_ = false;
  bool can_go_back_ = false;
  bool can_go_forward_ = false;
  mutable std::mutex mu_;

  IMPLEMENT_REFCOUNTING(OsrClient);
};
