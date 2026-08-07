#pragma once

#include "include/cef_v8.h"
#include "include/cef_browser.h"

#include <string>

/** Tiny V8 binding: window.goBrowser.postMessage(obj|string) → browser process. */
class GoBrowserHandler : public CefV8Handler {
 public:
  explicit GoBrowserHandler(CefRefPtr<CefFrame> frame) : frame_(frame) {}

  bool Execute(const CefString& name,
               CefRefPtr<CefV8Value> /*object*/,
               const CefV8ValueList& arguments,
               CefRefPtr<CefV8Value>& /*retval*/,
               CefString& /*exception*/) override {
    if (name != "postMessage" || arguments.empty() || !frame_) return false;

    std::string json;
    if (arguments[0]->IsString()) {
      json = arguments[0]->GetStringValue().ToString();
    } else {
      CefRefPtr<CefV8Context> ctx = CefV8Context::GetCurrentContext();
      if (!ctx) return false;
      CefRefPtr<CefV8Value> global = ctx->GetGlobal();
      CefRefPtr<CefV8Value> json_obj = global->GetValue("JSON");
      if (!json_obj || !json_obj->IsObject()) return false;
      CefRefPtr<CefV8Value> stringify = json_obj->GetValue("stringify");
      if (!stringify || !stringify->IsFunction()) return false;
      CefV8ValueList args;
      args.push_back(arguments[0]);
      CefRefPtr<CefV8Value> result = stringify->ExecuteFunction(json_obj, args);
      if (!result || !result->IsString()) return false;
      json = result->GetStringValue().ToString();
    }
    if (json.empty()) return false;

    CefRefPtr<CefProcessMessage> msg = CefProcessMessage::Create("goBrowser");
    msg->GetArgumentList()->SetString(0, json);
    frame_->SendProcessMessage(PID_BROWSER, msg);
    return true;
  }

 private:
  CefRefPtr<CefFrame> frame_;
  IMPLEMENT_REFCOUNTING(GoBrowserHandler);
};
