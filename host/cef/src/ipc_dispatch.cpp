#include "ipc_dispatch.h"

#include "browser_app.h"

#include "include/cef_browser.h"

#include <cstdio>

namespace {

CefRefPtr<OsrClient> TargetFromFocus(BrowserApp& app,
                                     gameoverlay::cef::FocusTarget target) {
  return app.ClientForFocusTarget(target);
}

cef_mouse_button_type_t MouseButtonFromProto(gameoverlay::cef::MouseButton btn) {
  switch (btn) {
    case gameoverlay::cef::MOUSE_BUTTON_MIDDLE:
      return MBT_MIDDLE;
    case gameoverlay::cef::MOUSE_BUTTON_RIGHT:
      return MBT_RIGHT;
    case gameoverlay::cef::MOUSE_BUTTON_LEFT:
    default:
      return MBT_LEFT;
  }
}

const char* MouseTypeFromProto(gameoverlay::cef::MouseEventType type) {
  switch (type) {
    case gameoverlay::cef::MOUSE_EVENT_TYPE_MOVE:
      return "move";
    case gameoverlay::cef::MOUSE_EVENT_TYPE_DOWN:
      return "down";
    case gameoverlay::cef::MOUSE_EVENT_TYPE_UP:
      return "up";
    case gameoverlay::cef::MOUSE_EVENT_TYPE_LEAVE:
      return "leave";
    default:
      return "";
  }
}

const char* KeyTypeFromProto(gameoverlay::cef::KeyEventType type) {
  switch (type) {
    case gameoverlay::cef::KEY_EVENT_TYPE_RAW_DOWN:
      return "keyDown";  // existing path → KEYEVENT_RAWKEYDOWN
    case gameoverlay::cef::KEY_EVENT_TYPE_KEY_DOWN:
      return "keyDownKey";  // KEYEVENT_KEYDOWN
    case gameoverlay::cef::KEY_EVENT_TYPE_KEY_UP:
      return "keyUp";
    case gameoverlay::cef::KEY_EVENT_TYPE_CHAR:
      return "char";
    default:
      return "";
  }
}

}  // namespace

void DispatchEnvelope(BrowserApp& app, const gameoverlay::cef::Envelope& env) {
  using gameoverlay::cef::Envelope;

  switch (env.body_case()) {
    case Envelope::kHello:
    case Envelope::kHelloAck:
    case Envelope::kError:
    case Envelope::kReady:
    case Envelope::kPaint:
    case Envelope::kPaintError:
    case Envelope::kNavState:
    case Envelope::kHostUiAction:
    case Envelope::kBridgeInvoke:
      // CEF→host / handshake noise — ignore on ingest.
      return;

    case Envelope::kBridgeResult: {
      const auto& m = env.bridge_result();
      app.DeliverBridgeResult(m.request_id(), m.ok(), m.result_json(), m.error());
      return;
    }
    case Envelope::kBridgePush: {
      app.DeliverBridgePush(env.bridge_push().message_json());
      return;
    }

    case Envelope::kCreateSession: {
      if (!app.IpcNegotiated()) {
        std::fprintf(stderr,
                     "cef ipc: CreateSession before HelloAck — rejected\n");
        return;
      }
      const auto& m = env.create_session();
      if (m.mode() != gameoverlay::cef::SESSION_MODE_OSR) {
        std::fprintf(stderr,
                     "cef ipc: CreateSession mode=%d rejected (OSR only)\n",
                     static_cast<int>(m.mode()));
        return;
      }
      app.CreateSessionOsr(static_cast<int>(m.width()), static_cast<int>(m.height()),
                           m.url());
      return;
    }
    case Envelope::kShutdown: {
      app.ShutdownSession(env.shutdown().quit_message_loop());
      return;
    }
    case Envelope::kSetSurfaceSize: {
      const auto& m = env.set_surface_size();
      app.ApplySetSurfaceSize(static_cast<int>(m.width()),
                              static_cast<int>(m.height()));
      return;
    }
    case Envelope::kSetInnerBounds: {
      const auto& m = env.set_inner_bounds();
      app.ApplySetInnerBounds(m.x(), m.y(), static_cast<int>(m.width()),
                              static_cast<int>(m.height()), m.layout_generation());
      return;
    }
    case Envelope::kSetContentRect: {
      const auto& m = env.set_content_rect();
      app.ApplySetContentRect(m.clear(), m.x(), m.y(),
                              static_cast<int>(m.width()),
                              static_cast<int>(m.height()));
      return;
    }
    case Envelope::kSetFocus: {
      const auto& m = env.set_focus();
      app.ApplySetFocus(m.focus(), m.target());
      return;
    }
    case Envelope::kSetHidden: {
      app.ApplySetHidden(env.set_hidden().hidden());
      return;
    }
    case Envelope::kNavigate: {
      app.Navigate(env.navigate().url());
      return;
    }
    case Envelope::kContentNavigate: {
      app.ContentNavigate(env.content_navigate().url());
      return;
    }
    case Envelope::kGoBack: {
      app.GoBack();
      return;
    }
    case Envelope::kGoForward: {
      app.GoForward();
      return;
    }
    case Envelope::kReload: {
      app.Reload();
      return;
    }
    case Envelope::kKeyEvent: {
      const auto& m = env.key_event();
      const char* type = KeyTypeFromProto(m.type());
      if (!type[0]) return;
      app.InjectKey(TargetFromFocus(app, m.target()), type, m.modifiers(),
                    static_cast<int>(m.windows_key_code()),
                    static_cast<int>(m.native_key_code()), m.is_system_key(),
                    m.character());
      return;
    }
    case Envelope::kMouseEvent: {
      const auto& m = env.mouse_event();
      const char* type = MouseTypeFromProto(m.type());
      if (!type[0]) return;
      app.InjectMouse(TargetFromFocus(app, m.target()), type, m.x(), m.y(),
                      static_cast<int>(m.click_count() ? m.click_count() : 1),
                      m.modifiers(), MouseButtonFromProto(m.button()));
      return;
    }
    case Envelope::kWheelEvent: {
      const auto& m = env.wheel_event();
      app.InjectWheel(TargetFromFocus(app, m.target()), m.x(), m.y(), m.delta_x(),
                      m.delta_y(), m.modifiers());
      return;
    }
    case Envelope::BODY_NOT_SET:
    default:
      return;
  }
}
