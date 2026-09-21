#include "osr_client.h"

#include "ipc_server.h"
#include "json_util.h"

#include "include/wrapper/cef_helpers.h"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

#include <cstdio>

OsrClient::OsrClient(IpcClient* ipc, OsrRole role) : ipc_(ipc), role_(role) {}

void OsrClient::GetViewRect(CefRefPtr<CefBrowser> /*browser*/, CefRect& rect) {
  std::lock_guard<std::mutex> lock(mu_);
  rect.Set(0, 0, width_ > 0 ? width_ : 1, height_ > 0 ? height_ : 1);
}

bool OsrClient::GetScreenInfo(CefRefPtr<CefBrowser> /*browser*/, CefScreenInfo& screen_info) {
  CefRect rect(0, 0, width_ > 0 ? width_ : 1, height_ > 0 ? height_ : 1);
  // OSR atlas coords are CSS pixels (SetSize / shell layout). A real DPI scale
  // makes CEF paint a larger shared texture than SetSize reports, so host crops
  // from the wrong region (top-left chrome baked into window promos, bottoms cut).
  screen_info.device_scale_factor = 1.0f;
  screen_info.depth = 32;
  screen_info.depth_per_component = 8;
  screen_info.is_monochrome = false;
  screen_info.rect = rect;
  screen_info.available_rect = rect;
  return true;
}

void OsrClient::OnPaint(CefRefPtr<CefBrowser> /*browser*/,
                        PaintElementType /*type*/,
                        const RectList& /*dirtyRects*/,
                        const void* /*buffer*/,
                        int /*width*/,
                        int /*height*/) {
  // Unused when shared_texture_enabled — display path is OnAcceleratedPaint.
}

void OsrClient::OnAcceleratedPaint(CefRefPtr<CefBrowser> /*browser*/,
                                   PaintElementType type,
                                   const RectList& dirtyRects,
                                   const CefAcceleratedPaintInfo& info) {
  CEF_REQUIRE_UI_THREAD();
  if (windowed_ || type != PET_VIEW || hidden_ || !paint_fn_) return;

  // Always use SetSize view rect (Electron stamps its widget size, not padded
  // coded_size — coded_size inflated the DXGI quad toward fullscreen).
  uint32_t w = 0;
  uint32_t h = 0;
  {
    std::lock_guard<std::mutex> lock(mu_);
    w = width_ > 0 ? static_cast<uint32_t>(width_) : 0;
    h = height_ > 0 ? static_cast<uint32_t>(height_) : 0;
  }
  if (w == 0 || h == 0 || !info.shared_texture_handle) return;

  std::vector<SharedDirtyRect> dirty;
  dirty.reserve(dirtyRects.size());
  for (const auto& r : dirtyRects) {
    dirty.push_back({r.x, r.y, r.width, r.height});
  }
  paint_fn_(role_, info.shared_texture_handle, w, h, dirty.data(), dirty.size());
}

void OsrClient::OnAfterCreated(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  browser_ = browser;
  HWND hwnd = browser->GetHost()->GetWindowHandle();
  // Windowless + HWND_MESSAGE: no on-screen host. Do not EnumWindows / park
  // desktop — that was fighting the wrong problem (NO_STAMP proved stamp-only).
  if (hwnd && IsWindow(hwnd) && windowed_) {
    ShowWindow(hwnd, SW_HIDE);
  }
  if (after_created_fn_) after_created_fn_();
}

bool OsrClient::OnBeforePopup(
    CefRefPtr<CefBrowser> /*browser*/,
    CefRefPtr<CefFrame> /*frame*/,
    int /*popup_id*/,
    const CefString& /*target_url*/,
    const CefString& /*target_frame_name*/,
    WindowOpenDisposition /*target_disposition*/,
    bool /*user_gesture*/,
    const CefPopupFeatures& /*popupFeatures*/,
    CefWindowInfo& /*windowInfo*/,
    CefRefPtr<CefClient>& /*client*/,
    CefBrowserSettings& /*settings*/,
    CefRefPtr<CefDictionaryValue>& /*extra_info*/,
    bool* /*no_javascript_access*/) {
  return true;
}

void OsrClient::OnBeforeClose(CefRefPtr<CefBrowser> /*browser*/) {
  CEF_REQUIRE_UI_THREAD();
  browser_ = nullptr;
}

void OsrClient::OnLoadingStateChange(CefRefPtr<CefBrowser> browser,
                                     bool isLoading,
                                     bool canGoBack,
                                     bool canGoForward) {
  CEF_REQUIRE_UI_THREAD();
  if (role_ == OsrRole::Chrome) {
    // Remounted chrome UI (#app) — tell BrowserApp to re-apply SetInnerBounds DOM.
    if (!isLoading && load_end_fn_) load_end_fn_();
    return;
  }
  loading_ = isLoading;
  can_go_back_ = canGoBack;
  can_go_forward_ = canGoForward;
  if (browser && browser->GetMainFrame()) {
    url_ = browser->GetMainFrame()->GetURL().ToString();
  }
  EmitNavState();
}

void OsrClient::OnTitleChange(CefRefPtr<CefBrowser> /*browser*/, const CefString& title) {
  CEF_REQUIRE_UI_THREAD();
  if (role_ != OsrRole::Content) return;
  title_ = title.ToString();
  EmitNavState();
}

void OsrClient::OnAddressChange(CefRefPtr<CefBrowser> /*browser*/,
                                CefRefPtr<CefFrame> frame,
                                const CefString& url) {
  CEF_REQUIRE_UI_THREAD();
  if (role_ != OsrRole::Content) return;
  if (frame && frame->IsMain()) {
    url_ = url.ToString();
    EmitNavState();
  }
}

void OsrClient::SetSize(int width, int height) {
  auto resize = [&] {
    std::lock_guard<std::mutex> lock(mu_);
    const int w = width > 0 ? width : 1;
    const int h = height > 0 ? height : 1;
    if (width_ == w && height_ == h) return false;
    width_ = w;
    height_ = h;
    return true;
  }();
  if (!browser_) return;
  if (resize) {
    browser_->GetHost()->WasResized();
  } else {
    browser_->GetHost()->Invalidate(PET_VIEW);
  }
}

void OsrClient::SetHidden(bool hidden) {
  hidden_ = hidden;
  if (browser_) {
    browser_->GetHost()->WasHidden(hidden);
    if (!hidden) {
      browser_->GetHost()->WasResized();
      browser_->GetHost()->Invalidate(PET_VIEW);
    }
  }
}

void OsrClient::EmitNavState() {
  if (role_ != OsrRole::Content) return;
  // In-process chrome UI still consumes a JSON mirror via nav_fn_.
  std::string json = std::string("{\"op\":\"navState\",\"url\":\"") + JsonEscape(url_) +
                     "\",\"title\":\"" + JsonEscape(title_) + "\",\"loading\":" +
                     (loading_ ? "true" : "false") + ",\"canGoBack\":" +
                     (can_go_back_ ? "true" : "false") + ",\"canGoForward\":" +
                     (can_go_forward_ ? "true" : "false") + "}";
  if (ipc_) {
    gameoverlay::cef::Envelope env;
    env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
    auto* nav = env.mutable_nav_state();
    nav->set_url(url_);
    nav->set_title(title_);
    nav->set_loading(loading_);
    nav->set_can_go_back(can_go_back_);
    nav->set_can_go_forward(can_go_forward_);
    ipc_->SendProto(env);
  }
  if (nav_fn_) nav_fn_(json);
}

bool OsrClient::OnProcessMessageReceived(CefRefPtr<CefBrowser> /*browser*/,
                                         CefRefPtr<CefFrame> /*frame*/,
                                         CefProcessId /*source_process*/,
                                         CefRefPtr<CefProcessMessage> message) {
  CEF_REQUIRE_UI_THREAD();
  if (!message || message->GetName() != "goBrowser") return false;
  CefRefPtr<CefListValue> args = message->GetArgumentList();
  if (!args || args->GetSize() < 1 || args->GetType(0) != VTYPE_STRING) return false;
  const std::string json = args->GetString(0).ToString();
  if (json.empty()) return false;
  if (chrome_msg_fn_) {
    chrome_msg_fn_(json);
    return true;
  }
  return false;
}
