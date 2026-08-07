#include "browser_app.h"

#include "go_browser_handler.h"
#include "ipc_dispatch.h"
#include "ipc_server.h"
#include "json_util.h"
#include "plugin_scheme.h"

#include "include/base/cef_callback.h"
#include "include/wrapper/cef_closure_task.h"
#include "include/wrapper/cef_helpers.h"

#include <algorithm>
#include <climits>
#include <cstdio>
#include <cstring>
#include <string>
#include <utility>

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <shlobj.h>

#include <string>

namespace {

/** Must equal sum of --chrome-h + --tab-h + --tool-h + --bookmarks-h in app.css. */
constexpr int kChromeTopPx = 34 + 32 + 36 + 28;

constexpr wchar_t kOsrMsgClass[] = L"GlintCefMsgHost";

/** Message-only parent — NOT GetDesktopWindow (that mirrored the DXGI stamp
 *  back into OSR → nested Browser). Same idea as Electron: paint buffer only. */
HWND CreateMessageOnlyHwnd() {
  static bool registered = false;
  if (!registered) {
    WNDCLASSEXW wc{};
    wc.cbSize = sizeof(wc);
    wc.lpfnWndProc = DefWindowProcW;
    wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = kOsrMsgClass;
    if (!RegisterClassExW(&wc) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
      std::fprintf(stderr, "cef: RegisterClassExW MsgHost failed err=%lu\n",
                   static_cast<unsigned long>(GetLastError()));
      return nullptr;
    }
    registered = true;
  }
  return CreateWindowExW(0, kOsrMsgClass, L"", 0, 0, 0, 0, 0, HWND_MESSAGE, nullptr,
                         GetModuleHandleW(nullptr), nullptr);
}

/** `__goHost` (contracts/host-bridge.md): the one app-facing bridge global.
 *  Promise bookkeeping lives in JS so the render process needs no extra native
 *  handler — the send rides the existing goBrowser channel, and the browser
 *  process settles/pushes through __goHostSettle / __goHostPush. */
constexpr char kGoHostBootstrap[] = R"JS((function(){
if(window.__goHost)return;
var next=0,pending={};
window.__goHost={embedded:true,invoke:function(method,args,pluginId){
  var id=++next;
  return new Promise(function(resolve,reject){
    if(!window.goBrowser){reject(new Error('host bridge unavailable'));return;}
    pending[id]={resolve:resolve,reject:reject};
    window.goBrowser.postMessage({op:'host_invoke',id:id,method:String(method),
      pluginId:pluginId||'',args:JSON.stringify(args||[])});
  });
}};
window.__goHostSettle=function(id,ok,resultJson,error){
  var entry=pending[id];if(!entry)return;delete pending[id];
  if(!ok){entry.reject(new Error(error||'host invoke failed'));return;}
  try{entry.resolve(resultJson?JSON.parse(resultJson):undefined);}
  catch(err){entry.reject(err);}
};
window.__goHostPush=function(json){
  try{window.postMessage(JSON.parse(json),'*');}catch(err){}
};
})();)JS";

HWND CefChildHwnd(CefRefPtr<OsrClient> client) {
  if (!client || !client->browser()) return nullptr;
  return client->browser()->GetHost()->GetWindowHandle();
}

void LayoutChild(HWND child, int x, int y, int w, int h) {
  if (!child || !IsWindow(child)) return;
  const int ww = w > 0 ? w : 1;
  const int hh = h > 0 ? h : 1;
  SetWindowPos(child, nullptr, x, y, ww, hh, SWP_NOZORDER | SWP_NOACTIVATE);
}

void KickOsr(CefRefPtr<OsrClient> client) {
  if (!client || !client->browser()) return;
  client->browser()->GetHost()->Invalidate(PET_VIEW);
  client->browser()->GetHost()->WasResized();
}

}  // namespace

BrowserApp::BrowserApp() = default;

void BrowserApp::OnBeforeCommandLineProcessing(
    const CefString& /*process_type*/,
    CefRefPtr<CefCommandLine> command_line) {
  // Accelerated OSR (OnAcceleratedPaint + D3D shared texture).
  command_line->AppendSwitch("enable-begin-frame-scheduling");
  command_line->AppendSwitchWithValue("use-angle", "d3d11");
  // GPU-native rendering — without this CEF falls back to software rasterization
  // per-texture, which adds latency per paint and makes scrolling clunky.
  command_line->AppendSwitch("enable-gpu-rasterization");
  command_line->AppendSwitch("ignore-gpu-blocklist");
  // Keep raster threads low: the game needs those cores. Uncapping the GPU
  // process (disable-gpu-vsync / disable-frame-rate-limit) submits unbounded
  // work that queues ahead of the game and makes hover feel *later*, not
  // sooner — the overlay is paced by the game's Present, not by CEF.
  command_line->AppendSwitchWithValue("num-raster-threads", "2");
  // Overlay host runs elevated (ETW/injector). CEF 120+ auto-de-elevates and
  // CefInitialize fails (exit 1) unless we opt out.
  command_line->AppendSwitch("do-not-de-elevate");
  // Chrome UI is file:// next to the helper — allow local script/css loads.
  command_line->AppendSwitch("allow-file-access-from-files");
  // Unpacked extensions from %APPDATA%\Glint\BrowserExtensions (same folder
  // Electron used). This CEF build has no RequestContext::LoadExtension.
  {
    wchar_t appdata[MAX_PATH] = {};
    if (SUCCEEDED(SHGetFolderPathW(nullptr, CSIDL_APPDATA, nullptr, 0, appdata))) {
      std::wstring root = std::wstring(appdata) + L"\\Glint\\BrowserExtensions";
      WIN32_FIND_DATAW fd{};
      HANDLE find = FindFirstFileW((root + L"\\*").c_str(), &fd);
      if (find != INVALID_HANDLE_VALUE) {
        std::wstring list;
        do {
          if (!(fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) continue;
          if (fd.cFileName[0] == L'.') continue;
          std::wstring dir = root + L"\\" + fd.cFileName;
          if (GetFileAttributesW((dir + L"\\manifest.json").c_str()) == INVALID_FILE_ATTRIBUTES) {
            continue;
          }
          if (!list.empty()) list.push_back(L',');
          list += dir;
        } while (FindNextFileW(find, &fd));
        FindClose(find);
        if (!list.empty()) {
          command_line->AppendSwitchWithValue("load-extension", CefString(list));
          std::fprintf(stderr, "cef: load-extension from BrowserExtensions\n");
        }
      }
    }
  }
}

void BrowserApp::OnRegisterCustomSchemes(CefRawPtr<CefSchemeRegistrar> registrar) {
  plugin_scheme::Register(registrar);
}

void BrowserApp::OnContextInitialized() {
  CEF_REQUIRE_UI_THREAD();
  plugin_scheme::InstallFactory();
  context_initialized_ = true;
  MaybeSendReady();
}

void BrowserApp::MaybeSendReady() {
  CEF_REQUIRE_UI_THREAD();
  if (ready_sent_ || !context_initialized_ || !IpcNegotiated() || !ipc_) return;
  ready_sent_ = true;
  gameoverlay::cef::Envelope env;
  env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
  env.mutable_ready()->set_chrome_top_px(static_cast<uint32_t>(kChromeTopPx));
  ipc_->SendProto(env);
}

void BrowserApp::OnContextCreated(CefRefPtr<CefBrowser> /*browser*/,
                                  CefRefPtr<CefFrame> frame,
                                  CefRefPtr<CefV8Context> context) {
  // Render process: chrome_ is unset. Bind only on file:// chrome UI main frames
  // (content browser is about:blank / https — no goBrowser).
  if (!frame || !frame->IsMain()) return;
  const std::string url = frame->GetURL().ToString();
  if (!url.empty() && url.rfind("file:", 0) != 0) return;

  CefRefPtr<CefV8Value> global = context->GetGlobal();
  CefRefPtr<CefV8Value> go = CefV8Value::CreateObject(nullptr, nullptr);
  CefRefPtr<CefV8Value> post =
      CefV8Value::CreateFunction("postMessage", new GoBrowserHandler(frame));
  go->SetValue("postMessage", post, V8_PROPERTY_ATTRIBUTE_NONE);
  global->SetValue("goBrowser", go, V8_PROPERTY_ATTRIBUTE_NONE);

  frame->ExecuteJavaScript(kGoHostBootstrap, "gameoverlay://go-host", 0);
}

void BrowserApp::HandleEnvelope(gameoverlay::cef::Envelope env) {
  if (!CefCurrentlyOn(TID_UI)) {
    CefPostTask(TID_UI, base::BindOnce(
                            [](CefRefPtr<BrowserApp> self,
                               gameoverlay::cef::Envelope body) {
                              self->HandleEnvelope(std::move(body));
                            },
                            CefRefPtr<BrowserApp>(this), std::move(env)));
    return;
  }
  if (env.has_hello_ack()) {
    MaybeSendReady();
  }
  DispatchEnvelope(*this, env);
}

bool BrowserApp::IpcNegotiated() const {
  return ipc_ && ipc_->proto_negotiated();
}

void BrowserApp::SendPaintError(const char* msg) {
  if (!ipc_) return;
  gameoverlay::cef::Envelope env;
  env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
  env.mutable_paint_error()->set_message(msg ? msg : "unknown");
  ipc_->SendProto(env);
}

void BrowserApp::SendHostUiAction(gameoverlay::cef::HostUiActionKind kind) {
  if (!ipc_) return;
  gameoverlay::cef::Envelope env;
  env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
  env.mutable_host_ui_action()->set_kind(kind);
  ipc_->SendProto(env);
}

void BrowserApp::CreateSessionOsr(int w, int h, const std::string& url) {
  CreateBrowser(w, h, url, false, nullptr);
}

void BrowserApp::ApplySetSurfaceSize(int w, int h) {
  if (!hwnd_mode_) SetSurfaceSize(w, h);
}

void BrowserApp::ApplySetInnerBounds(int x, int y, int w, int h,
                                     uint32_t layout_generation) {
  // generation 0 = legacy/ignore — still apply bounds; Paint echoes whatever we store.
  layout_generation_ = layout_generation;
  if (!hwnd_mode_) SetInnerWindow(x, y, w, h);
}

void BrowserApp::ApplySetFocus(bool focus, gameoverlay::cef::FocusTarget target) {
  CefRefPtr<OsrClient> t;
  if (hwnd_mode_) {
    t = client_;
  } else if (target == gameoverlay::cef::FOCUS_TARGET_CONTENT) {
    t = content_;
  } else {
    t = chrome_;
  }
  if (!t || !t->browser()) {
    // The host focuses chrome right after CreateSession, long before
    // OnAfterCreated — dropping it leaves the shell with no focused frame and
    // no later SetFocus (focus only re-sends when the hover target changes).
    if (!hwnd_mode_) {
      pending_focus_ = focus;
      pending_focus_target_ = target;
    }
    return;
  }
  auto host = t->browser()->GetHost();
  host->SetFocus(focus);
  if (hwnd_mode_) {
    HWND hwnd_target = host->GetWindowHandle();
    if (!hwnd_target) hwnd_target = parent_hwnd_;
    if (focus && hwnd_target && parent_hwnd_ && IsWindowVisible(parent_hwnd_)) {
      SetFocus(hwnd_target);
    }
  }
}

void BrowserApp::ApplySetHidden(bool hidden) {
  if (hwnd_mode_) return;
  if (chrome_) chrome_->SetHidden(hidden);
  if (content_) content_->SetHidden(hidden);
}

void BrowserApp::GoBack() {
  if (content_ && content_->browser()) content_->browser()->GoBack();
  else if (client_ && client_->browser()) client_->browser()->GoBack();
}

void BrowserApp::GoForward() {
  if (content_ && content_->browser()) content_->browser()->GoForward();
  else if (client_ && client_->browser()) client_->browser()->GoForward();
}

void BrowserApp::Reload() {
  if (content_ && content_->browser()) content_->browser()->Reload();
  else if (client_ && client_->browser()) client_->browser()->Reload();
}

void BrowserApp::ShutdownSession(bool quit_message_loop) {
  ShutdownBrowser();
  if (quit_message_loop) CefQuitMessageLoop();
}

CefRefPtr<OsrClient> BrowserApp::ClientForFocusTarget(
    gameoverlay::cef::FocusTarget target) {
  if (target == gameoverlay::cef::FOCUS_TARGET_CONTENT) return content_;
  return chrome_;
}

void BrowserApp::DestroyOwnedHost() {
  if (owned_host_ && IsWindow(owned_host_)) {
    DestroyWindow(owned_host_);
  }
  owned_host_ = nullptr;
}

void BrowserApp::SetBounds(int x, int y, int w, int h, HWND parent) {
  if (!hwnd_mode_) return;
  if (parent && IsWindow(parent)) {
    parent_hwnd_ = parent;
  }
  if (!parent_hwnd_ || !IsWindow(parent_hwnd_)) return;

  const int ww = w > 0 ? w : 1;
  const int hh = h > 0 ? h : 1;

  HWND child = CefChildHwnd(client_);
  if (child) {
    if (GetParent(child) != parent_hwnd_) {
      SetParent(child, parent_hwnd_);
    }
    LayoutChild(child, x, y, ww, hh);
  }
  if (client_) client_->SetSize(ww, hh);
}

void BrowserApp::SetVisible(bool visible) {
  if (!hwnd_mode_) return;
  HWND child = CefChildHwnd(client_);
  if (!child || !IsWindow(child)) return;
  if (!visible) {
    ShowWindow(child, SW_HIDE);
    return;
  }
  if (!parent_hwnd_ || !IsWindow(parent_hwnd_) || !IsWindowVisible(parent_hwnd_)) {
    ShowWindow(child, SW_HIDE);
    return;
  }
  if (GetParent(child) != parent_hwnd_) {
    SetParent(child, parent_hwnd_);
  }
  ShowWindow(child, SW_SHOWNOACTIVATE);
}

void BrowserApp::LayoutContentHole() {
  if (content_rect_override_) {
    if (content_) content_->SetSize(content_w_ > 0 ? content_w_ : 1,
                                    content_h_ > 0 ? content_h_ : 1);
    return;
  }
  const int top = std::min(kChromeTopPx, std::max(0, window_h_ - 1));
  content_x_ = window_x_;
  content_y_ = window_y_ + top;
  content_w_ = window_w_ > 0 ? window_w_ : 1;
  content_h_ = std::max(1, window_h_ - top);
  if (content_) content_->SetSize(content_w_, content_h_);
}

void BrowserApp::ApplySetContentRect(bool clear, int x, int y, int w, int h) {
  if (clear) {
    content_rect_override_ = false;
    LayoutContentHole();
    // Clearing the hole — treat as blank until the next real navigate paints.
    content_blank_ = true;
    PublishComposite();
    return;
  }
  content_rect_override_ = true;
  content_blank_ = false;
  content_x_ = x;
  content_y_ = y;
  content_w_ = w > 0 ? w : 1;
  content_h_ = h > 0 ? h : 1;
  if (content_) content_->SetSize(content_w_, content_h_);
  if (content_ && content_->browser()) {
    content_->browser()->GetHost()->WasResized();
    content_->browser()->GetHost()->Invalidate(PET_VIEW);
  }
  PublishComposite();
}

void BrowserApp::PushWindowBoundsToUi() {
  if (!chrome_ || !chrome_->browser()) return;
  if (last_js_x_ == window_x_ && last_js_y_ == window_y_ &&
      last_js_w_ == window_w_ && last_js_h_ == window_h_)
    return;
  last_js_x_ = window_x_;
  last_js_y_ = window_y_;
  last_js_w_ = window_w_;
  last_js_h_ = window_h_;
  // Direct DOM style on #app (no JS helper).
  char script[384];
  std::snprintf(
      script, sizeof(script),
      "(function(x,y,w,h){var a=document.getElementById('app');if(!a)return;"
      "a.style.left=x+'px';a.style.top=y+'px';"
      "a.style.width=Math.max(1,w)+'px';a.style.height=Math.max(1,h)+'px';})(%d,%d,%d,%d);",
      window_x_, window_y_, window_w_, window_h_);
  chrome_->browser()->GetMainFrame()->ExecuteJavaScript(script, "", 0);
}

void BrowserApp::ApplyInnerWindow() {
  LayoutContentHole();
  PushWindowBoundsToUi();
  if (chrome_ && chrome_->browser()) {
    chrome_->browser()->GetHost()->Invalidate(PET_VIEW);
  }
}

void BrowserApp::SetSurfaceSize(int w, int h) {
  chrome_w_ = w > 0 ? w : 1;
  chrome_h_ = h > 0 ? h : 1;
  if (chrome_) chrome_->SetSize(chrome_w_, chrome_h_);
  // Inner window unchanged; chrome must repaint fullscreen transparent root.
  if (chrome_ && chrome_->browser()) {
    chrome_->browser()->GetHost()->Invalidate(PET_VIEW);
  }
}

void BrowserApp::SetInnerWindow(int x, int y, int w, int h) {
  window_x_ = x;
  window_y_ = y;
  window_w_ = w > 0 ? w : 1;
  window_h_ = h > 0 ? h : 1;
  ApplyInnerWindow();
}

void BrowserApp::CreateDualOsr(int w, int h, const std::string& chrome_url) {
  chrome_url_ = chrome_url;
  chrome_w_ = w > 0 ? w : 1;
  chrome_h_ = h > 0 ? h : 1;
  window_x_ = 0;
  window_y_ = 0;
  window_w_ = chrome_w_;
  window_h_ = chrome_h_;

  auto paint = [this](OsrRole role, HANDLE handle, uint32_t pw, uint32_t ph) {
    OnOsrPaint(role, handle, pw, ph);
  };
  auto chrome_msg = [this](const std::string& body) { HandleChromeMessage(body); };
  auto nav = [this](const std::string& body) { PushNavToChrome(body); };

  chrome_ = new OsrClient(ipc_, OsrRole::Chrome);
  chrome_->SetWindowed(false);
  chrome_->SetSize(chrome_w_, chrome_h_);
  chrome_->SetPaintHandler(paint);
  chrome_->SetChromeMessageHandler(chrome_msg);
  chrome_->SetAfterCreatedHandler([this]() {
    PushWindowBoundsToUi();
    FlushPendingFocus();
  });
  chrome_->SetLoadEndHandler([this]() { PushWindowBoundsToUi(); });

  content_ = new OsrClient(ipc_, OsrRole::Content);
  content_->SetWindowed(false);
  LayoutContentHole();
  content_->SetPaintHandler(paint);
  content_->SetNavStateHandler(nav);
  content_->SetAfterCreatedHandler([this]() {
    FlushPendingContentNavigate();
    FlushPendingFocus();
  });

  // 60 is a cap on BeginFrame *attempts*, not a target. Raising it past what
  // the GPU can retire under game load just backs up composite work on the CEF
  // UI thread, which is the same thread that dispatches injected mouse events —
  // so a higher cap directly delays :hover feedback.
  CefBrowserSettings chrome_settings;
  chrome_settings.background_color = CefColorSetARGB(0, 0, 0, 0);
  chrome_settings.windowless_frame_rate = 60;

  CefBrowserSettings content_settings;
  content_settings.background_color = CefColorSetARGB(255, 10, 12, 18);
  content_settings.windowless_frame_rate = 60;

  DestroyOwnedHost();
  owned_host_ = CreateMessageOnlyHwnd();
  if (!owned_host_) {
    std::fprintf(stderr, "cef: HWND_MESSAGE owner missing — refusing CreateDualOsr\n");
    chrome_ = nullptr;
    content_ = nullptr;
    created_ = false;
    if (ipc_) {
      SendPaintError("HWND_MESSAGE owner create failed");
    }
    return;
  }

  {
    CefWindowInfo wi;
    wi.SetAsWindowless(owned_host_);
    wi.shared_texture_enabled = TRUE;
    CefBrowserHost::CreateBrowser(wi, chrome_, chrome_url, chrome_settings, nullptr,
                                  nullptr);
  }
  {
    CefWindowInfo wi;
    wi.SetAsWindowless(owned_host_);
    wi.shared_texture_enabled = TRUE;
    CefBrowserHost::CreateBrowser(wi, content_, "about:blank", content_settings, nullptr,
                                  nullptr);
  }

  created_ = true;
  CefPostDelayedTask(TID_UI, base::BindOnce([](CefRefPtr<OsrClient> c, CefRefPtr<OsrClient> d) {
                       KickOsr(c);
                       KickOsr(d);
                     },
                                            chrome_, content_),
                     50);
  // DOM may not exist at OnAfterCreated — retry once UI has loaded.
  CefPostDelayedTask(
      TID_UI,
      base::BindOnce([](CefRefPtr<BrowserApp> self) { self->PushWindowBoundsToUi(); },
                     CefRefPtr<BrowserApp>(this)),
      200);
}

void BrowserApp::CreateBrowser(int w, int h, const std::string& url, bool hwnd_mode,
                               HWND parent) {
  if (parent && IsWindow(parent)) {
    parent_hwnd_ = parent;
  }

  if (created_) {
    if (hwnd_mode_ == hwnd_mode && !hwnd_mode && chrome_ && content_) {
      SetSurfaceSize(w, h);
      SetInnerWindow(0, 0, w, h);
      // file: → chrome UI; https/etc → content. Never LoadURL https into chrome.
      if (url.rfind("file:", 0) == 0) Navigate(url);
      else ContentNavigate(url);
      return;
    }
    if (hwnd_mode_ == hwnd_mode && hwnd_mode && client_ && client_->browser()) {
      if (parent_hwnd_ && IsWindow(parent_hwnd_)) {
        HWND child = CefChildHwnd(client_);
        if (child && GetParent(child) != parent_hwnd_) {
          SetParent(child, parent_hwnd_);
        }
      }
      client_->SetSize(w, h);
      Navigate(url);
      return;
    }
    ShutdownBrowser();
  }

  hwnd_mode_ = hwnd_mode;

  if (!hwnd_mode) {
    CreateDualOsr(w, h, url);
    return;
  }

  // Experimental HWND path — single windowed client.
  client_ = new OsrClient(ipc_, OsrRole::Content);
  client_->SetWindowed(true);
  client_->SetSize(w, h);

  CefWindowInfo window_info;
  CefBrowserSettings settings;
  settings.background_color = CefColorSetARGB(255, 255, 255, 255);

  if (!parent_hwnd_ || !IsWindow(parent_hwnd_)) {
    client_ = nullptr;
    created_ = false;
    return;
  }
  window_info.SetAsChild(parent_hwnd_,
                         CefRect(0, 0, w > 0 ? w : 1, h > 0 ? h : 1));

  CefBrowserHost::CreateBrowser(window_info, client_, url, settings, nullptr, nullptr);
  created_ = true;
}

void BrowserApp::OnOsrPaint(OsrRole role, HANDLE shared_handle, uint32_t w, uint32_t h) {
  const int layer = (role == OsrRole::Chrome) ? 0 : 1;
  if (!publisher_.CacheLayer(layer, shared_handle, w, h)) {
    SendPaintError("CacheLayer failed");
    return;
  }
  PublishComposite();
}

void BrowserApp::PublishComposite() {
  if (!ipc_ || !publisher_.has_chrome()) return;
  uint64_t remote = 0;
  if (!publisher_.PublishComposite(window_x_, window_y_, window_w_, window_h_, content_x_,
                                   content_y_, !content_blank_, &remote)) {
    SendPaintError("PublishComposite failed");
    return;
  }
  gameoverlay::cef::Envelope env;
  env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
  auto* paint = env.mutable_paint();
  paint->set_width(publisher_.chrome_w());
  paint->set_height(publisher_.chrome_h());
  paint->set_nt_handle(remote);
  paint->set_layout_generation(layout_generation_);
  ipc_->SendProto(env);
}

void BrowserApp::HandleChromeMessage(const std::string& json) {
  const std::string op = JsonGetOp(json);
  if (op == "content_navigate") {
    std::string url;
    if (JsonFindString(json, "url", &url)) ContentNavigate(url);
    return;
  }
  if (op == "go_back") {
    if (content_ && content_->browser()) content_->browser()->GoBack();
    return;
  }
  if (op == "go_forward") {
    if (content_ && content_->browser()) content_->browser()->GoForward();
    return;
  }
  if (op == "reload") {
    if (content_ && content_->browser()) content_->browser()->Reload();
    return;
  }
  if (op == "host_invoke") {
    unsigned int id = 0;
    std::string method, args, plugin_id;
    JsonFindUInt(json, "id", &id);
    JsonFindString(json, "method", &method);
    JsonFindString(json, "pluginId", &plugin_id);
    JsonFindString(json, "args", &args);
    if (!id || method.empty()) return;
    if (!ipc_) {
      DeliverBridgeResult(id, false, "", "host bridge not connected");
      return;
    }
    gameoverlay::cef::Envelope env;
    env.set_protocol_version(gameoverlay::cef::PROTOCOL_VERSION_1);
    auto* body = env.mutable_bridge_invoke();
    body->set_request_id(id);
    body->set_method(method);
    body->set_args_json(args);
    body->set_plugin_id(plugin_id);
    ipc_->SendProto(env);
    return;
  }
  // Chrome UI → host: convert in-process JSON to HostUiAction proto (never TCP JSON).
  if (op == "host") {
    std::string action;
    JsonFindString(json, "action", &action);
    if (action == "close") {
      SendHostUiAction(gameoverlay::cef::HOST_UI_ACTION_KIND_CLOSE);
    } else if (action == "minimize") {
      SendHostUiAction(gameoverlay::cef::HOST_UI_ACTION_KIND_MINIMIZE);
    } else if (action == "move") {
      SendHostUiAction(gameoverlay::cef::HOST_UI_ACTION_KIND_MOVE);
    } else if (action == "setBounds" || action == "set_bounds") {
      SendHostUiAction(gameoverlay::cef::HOST_UI_ACTION_KIND_SET_BOUNDS);
    }
    return;
  }
}

void BrowserApp::DeliverBridgeResult(uint32_t request_id, bool ok,
                                     const std::string& result_json,
                                     const std::string& error) {
  if (!chrome_ || !chrome_->browser()) return;
  const std::string script = "window.__goHostSettle&&window.__goHostSettle(" +
                             std::to_string(request_id) + "," +
                             (ok ? "true" : "false") + ",\"" +
                             JsonEscape(result_json) + "\",\"" + JsonEscape(error) +
                             "\");";
  chrome_->browser()->GetMainFrame()->ExecuteJavaScript(script, "", 0);
}

void BrowserApp::DeliverBridgePush(const std::string& message_json) {
  if (!chrome_ || !chrome_->browser() || message_json.empty()) return;
  const std::string script = "window.__goHostPush&&window.__goHostPush(\"" +
                             JsonEscape(message_json) + "\");";
  chrome_->browser()->GetMainFrame()->ExecuteJavaScript(script, "", 0);
}

void BrowserApp::PushNavToChrome(const std::string& nav_json) {
  if (!chrome_ || !chrome_->browser()) return;
  // Apply content navState into chrome UI: window.__goApplyNavState(obj)
  std::string url, title;
  JsonFindString(nav_json, "url", &url);
  JsonFindString(nav_json, "title", &title);
  bool loading = false, can_back = false, can_fwd = false;
  JsonFindBool(nav_json, "loading", &loading);
  JsonFindBool(nav_json, "canGoBack", &can_back);
  JsonFindBool(nav_json, "canGoForward", &can_fwd);
  content_blank_ = url.empty() || url == "about:blank";
  if (content_blank_) PublishComposite();

  char script[2048];
  std::snprintf(
      script, sizeof(script),
      "window.__goApplyNavState&&window.__goApplyNavState({url:\"%s\",title:\"%s\","
      "loading:%s,canGoBack:%s,canGoForward:%s});",
      JsonEscape(url).c_str(), JsonEscape(title).c_str(), loading ? "true" : "false",
      can_back ? "true" : "false", can_fwd ? "true" : "false");
  chrome_->browser()->GetMainFrame()->ExecuteJavaScript(script, "", 0);
}

void BrowserApp::ContentNavigate(const std::string& url) {
  content_blank_ = url.empty() || url == "about:blank";
  if (!content_ || !content_->browser()) {
    pending_content_url_ = url;
    return;
  }
  pending_content_url_.clear();
  content_->browser()->GetMainFrame()->LoadURL(url);
  if (content_blank_) PublishComposite();
}

void BrowserApp::FlushPendingFocus() {
  if (pending_focus_target_ == gameoverlay::cef::FOCUS_TARGET_UNSPECIFIED) return;
  CefRefPtr<OsrClient> t = ClientForFocusTarget(pending_focus_target_);
  if (!t || !t->browser()) return;
  const gameoverlay::cef::FocusTarget target = pending_focus_target_;
  pending_focus_target_ = gameoverlay::cef::FOCUS_TARGET_UNSPECIFIED;
  ApplySetFocus(pending_focus_, target);
}

void BrowserApp::FlushPendingContentNavigate() {
  if (pending_content_url_.empty()) return;
  const std::string url = pending_content_url_;
  pending_content_url_.clear();
  ContentNavigate(url);
}

void BrowserApp::Navigate(const std::string& url) {
  if (hwnd_mode_) {
    if (!client_ || !client_->browser()) return;
    client_->browser()->GetMainFrame()->LoadURL(url);
    return;
  }
  // OSR: navigate = chrome UI main frame only (never content https).
  if (!chrome_ || !chrome_->browser()) return;
  // Invalidate cache — remounted #app must get left/top/width/height again.
  last_js_x_ = last_js_y_ = INT_MIN;
  last_js_w_ = last_js_h_ = -1;
  chrome_->browser()->GetMainFrame()->LoadURL(url);
}

void BrowserApp::InjectMouse(CefRefPtr<OsrClient> t, const char* type, int x, int y,
                             int click_count, uint32_t modifiers,
                             cef_mouse_button_type_t button) {
  if (!t || !t->browser() || !type) return;
  auto host = t->browser()->GetHost();

  CefMouseEvent ev;
  ev.x = x;
  ev.y = y;
  ev.modifiers = modifiers;

  if (std::strcmp(type, "move") == 0) {
    host->SendMouseMoveEvent(ev, false);
  } else if (std::strcmp(type, "leave") == 0) {
    host->SendMouseMoveEvent(ev, true);
  } else if (std::strcmp(type, "down") == 0) {
    host->SendMouseClickEvent(ev, button, false, std::max(1, click_count));
  } else if (std::strcmp(type, "up") == 0) {
    host->SendMouseClickEvent(ev, button, true, std::max(1, click_count));
  }
}

void BrowserApp::InjectWheel(CefRefPtr<OsrClient> t, int x, int y, int dx, int dy,
                             uint32_t modifiers) {
  if (!t || !t->browser()) return;
  CefMouseEvent ev;
  ev.x = x;
  ev.y = y;
  ev.modifiers = modifiers;
  t->browser()->GetHost()->SendMouseWheelEvent(ev, dx, dy);
}

void BrowserApp::InjectKey(CefRefPtr<OsrClient> t, const char* type, uint32_t modifiers,
                           int windows_key_code, int native_key_code,
                           bool is_system_key, const std::string& character) {
  std::fprintf(stderr,
               "KEYDIAG inject type=%s vk=0x%02X native=0x%08X mods=0x%04X sys=%d "
               "chrlen=%zu client=%d browser=%d\n",
               type ? type : "(null)", windows_key_code,
               static_cast<unsigned>(native_key_code), modifiers, is_system_key ? 1 : 0,
               character.size(), t ? 1 : 0, (t && t->browser()) ? 1 : 0);
  if (!t || !t->browser() || !type) return;
  auto host = t->browser()->GetHost();

  CefKeyEvent ev;
  ev.modifiers = modifiers;
  ev.windows_key_code = windows_key_code;
  ev.native_key_code = native_key_code;
  ev.is_system_key = is_system_key;

  // Only CHAR events carry text. Synthesizing character from the virtual-key
  // code put U+0011 on every Ctrl press and 'A' on every VK_A keydown, which
  // corrupts text insertion and competes with accelerator handling (Ctrl+A).
  if (!character.empty()) {
    const std::u16string u16 = CefString(character).ToString16();
    if (!u16.empty()) {
      ev.character = u16[0];
      ev.unmodified_character = ev.character;
    }
  }

  if (std::strcmp(type, "keyDown") == 0) {
    ev.type = KEYEVENT_RAWKEYDOWN;
    host->SendKeyEvent(ev);
  } else if (std::strcmp(type, "keyDownKey") == 0) {
    ev.type = KEYEVENT_KEYDOWN;
    host->SendKeyEvent(ev);
  } else if (std::strcmp(type, "keyUp") == 0) {
    ev.type = KEYEVENT_KEYUP;
    host->SendKeyEvent(ev);
  } else if (std::strcmp(type, "char") == 0) {
    ev.type = KEYEVENT_CHAR;
    host->SendKeyEvent(ev);
  }
}

void BrowserApp::ShutdownBrowser() {
  if (chrome_ && chrome_->browser()) {
    chrome_->browser()->GetHost()->CloseBrowser(true);
  }
  if (content_ && content_->browser()) {
    content_->browser()->GetHost()->CloseBrowser(true);
  }
  if (client_ && client_->browser()) {
    client_->browser()->GetHost()->CloseBrowser(true);
  }
  chrome_ = nullptr;
  content_ = nullptr;
  client_ = nullptr;
  created_ = false;
  pending_content_url_.clear();
  pending_focus_target_ = gameoverlay::cef::FOCUS_TARGET_UNSPECIFIED;
  DestroyOwnedHost();
  parent_hwnd_ = nullptr;
  hwnd_mode_ = false;
  chrome_w_ = chrome_h_ = 0;
  window_x_ = window_y_ = window_w_ = window_h_ = 0;
  content_x_ = content_y_ = content_w_ = content_h_ = 0;
  content_blank_ = true;
}
