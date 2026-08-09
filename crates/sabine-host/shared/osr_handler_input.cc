#include "osr_handler.h"

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iostream>
#include <limits>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#else
#include <sys/socket.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <unistd.h>
#endif

#include "guest_input.h"
#include "guest_manager.h"
#include "include/cef_app.h"
#include "include/cef_browser.h"
#include "include/cef_parser.h"
#include "include/cef_request_context_handler.h"
#include "include/cef_task.h"
#include "include/internal/cef_types.h"
#include "include/wrapper/cef_helpers.h"
#include "json_util.h"
#include "sabine_bridge_js.h"
#include "osr_handler_ime.h"
#include "osr_handler_screen.h"
#include "osr_handler_util.h"
#if defined(OS_WIN)
#include "osr_accel_d3d11_win.h"
#endif

using namespace sabine_osr;

void SabineOsrHandler::HandleControlLine(const std::string& line) {
  CEF_REQUIRE_UI_THREAD();
  std::string browser_id;
  std::string request_id;
  std::string name_json;
  std::string payload;
  std::string command;
  std::string value;
  bool ok = false;
  if (ParseBridgeResponse(line, &browser_id, &request_id, &ok, &payload)) {
    ResolveBridgeResponse(browser_id, request_id, ok, payload);
    return;
  }
  if (ParseBridgeEvent(line, &name_json, &payload)) {
    EmitBridgeEvent(name_json, payload);
    return;
  }
  if (ParseHostControl(line, &command, &value)) {
    ApplyHostControl(command, value);
    return;
  }
  const auto parts = Split(line, '\t');
#if defined(OS_WIN)
  if (parts.size() == 2 && parts[0] == "accel_release") {
    ReleaseAcceleratedD3d11Frame(std::strtoull(parts[1].c_str(), nullptr, 10));
    return;
  }
#endif
  if (parts.empty() || !browser_) {
    return;
  }
  if (TryHandleScreenOriginControl(this, parts)) {
    return;
  }
  CefRefPtr<CefBrowser> target_browser = browser_;
  GuestView* pointer_guest = nullptr;
  int pointer_x = parts.size() >= 3 ? std::atoi(parts[1].c_str()) : 0;
  int pointer_y = parts.size() >= 3 ? std::atoi(parts[2].c_str()) : 0;
  const bool pointer_event =
      parts[0] == "mouse_move" || parts[0] == "mouse_click" ||
      parts[0] == "mouse_wheel" || parts[0] == "mouse_navigation";
  if (pointer_event) {
    pointer_guest = guests_.TopmostAt(pointer_x, pointer_y);
    if (pointer_guest && pointer_guest->browser) {
      target_browser = pointer_guest->browser;
      pointer_x -= pointer_guest->bounds.x;
      pointer_y -= pointer_guest->bounds.y;
      if (parts[0] == "mouse_click") {
        FocusGuest(pointer_guest->id);
      }
    } else if (parts[0] == "mouse_click") {
      DismissPopupGuest();
      FocusGuest(std::string());
    }
  } else if ((parts[0] == "key" || parts[0].rfind("ime_", 0) == 0) &&
             !focused_guest_id_.empty()) {
    GuestView* guest = guests_.Find(focused_guest_id_);
    if (guest && guest->browser) {
      target_browser = guest->browser;
    }
  }
  CefRefPtr<CefBrowserHost> host = target_browser->GetHost();
  if (TryHandleImeControl(host, parts)) {
    return;
  }
  if (parts[0] == "resize" && parts.size() >= 4) {
    const int width = std::max(1, std::atoi(parts[1].c_str()));
    const int height = std::max(1, std::atoi(parts[2].c_str()));
    const float scale =
        std::max(0.25f, static_cast<float>(std::atof(parts[3].c_str())));
    // Same-size configures after interactive move must not Invalidate — that
    // flashes the OSR surface until the next paint lands.
    if (width == width_ && height == height_ &&
        std::fabs(scale - scale_) < 0.0001f) {
      return;
    }
    width_ = width;
    height_ = height;
    scale_ = scale;
    host->NotifyScreenInfoChanged();
    host->WasResized();
    host->Invalidate(PET_VIEW);
    NotifyGuestScreenInfo();
  } else if (parts[0] == "mouse_move" && parts.size() >= 5) {
    CefMouseEvent event;
    event.x = pointer_x;
    event.y = pointer_y;
    event.modifiers = std::strtoul(parts[3].c_str(), nullptr, 10);
    host->SendMouseMoveEvent(event, std::atoi(parts[4].c_str()) != 0);
  } else if (parts[0] == "mouse_click" && parts.size() >= 7) {
    CefMouseEvent event;
    event.x = pointer_x;
    event.y = pointer_y;
    const auto button = MouseButtonFromString(parts[3]);
    event.modifiers = std::strtoul(parts[4].c_str(), nullptr, 10);
    const bool up = std::atoi(parts[5].c_str()) != 0;
    const int click_count = std::max(1, std::atoi(parts[6].c_str()));
    host->SendMouseClickEvent(event, button, up, click_count);
    if (up && parts[3] == "right") {
      const std::string script =
          std::string("(function(){const target=document.elementFromPoint(") +
          std::to_string(event.x) + "," + std::to_string(event.y) +
          ")||document.body||window;const init={bubbles:true,cancelable:true,button:2,buttons:0,clientX:" +
          std::to_string(event.x) + ",clientY:" + std::to_string(event.y) +
          ",screenX:" + std::to_string(event.x) + ",screenY:" +
          std::to_string(event.y) + ",ctrlKey:" +
          ((event.modifiers & (1 << 2)) ? "true" : "false") + ",shiftKey:" +
          ((event.modifiers & (1 << 1)) ? "true" : "false") + ",altKey:" +
          ((event.modifiers & (1 << 3)) ? "true" : "false") + ",metaKey:" +
          ((event.modifiers & (1 << 7)) ? "true" : "false") +
          "};target.dispatchEvent(new MouseEvent('contextmenu',init));})();";
      target_browser->GetMainFrame()->ExecuteJavaScript(
          script, target_browser->GetMainFrame()->GetURL(), 0);
    }
  } else if (parts[0] == "mouse_navigation" && parts.size() >= 5) {
    const int x = pointer_x;
    const int y = pointer_y;
    const int button = std::atoi(parts[3].c_str());
    const uint32_t modifiers = std::strtoul(parts[4].c_str(), nullptr, 10);
    const std::string script =
        std::string("(function(){const target=document.elementFromPoint(") + std::to_string(x) +
        "," + std::to_string(y) + ")||window;const init={bubbles:true,cancelable:true,button:" +
        std::to_string(button) + ",buttons:0,clientX:" + std::to_string(x) +
        ",clientY:" + std::to_string(y) + ",screenX:" + std::to_string(x) +
        ",screenY:" + std::to_string(y) + ",ctrlKey:" +
        ((modifiers & (1 << 2)) ? "true" : "false") + ",shiftKey:" +
        ((modifiers & (1 << 1)) ? "true" : "false") + ",altKey:" +
        ((modifiers & (1 << 3)) ? "true" : "false") + ",metaKey:" +
        ((modifiers & (1 << 7)) ? "true" : "false") +
        "};const up=new MouseEvent('mouseup',init);const aux=new MouseEvent('auxclick',init);"
        "const canceled=(target.dispatchEvent(up)===false)||up.defaultPrevented||"
        "(target.dispatchEvent(aux)===false)||aux.defaultPrevented;"
        "if(!canceled){if(" +
        std::to_string(button) +
        "===3)history.back();else if(" + std::to_string(button) +
        "===4)history.forward();}})();";
    target_browser->GetMainFrame()->ExecuteJavaScript(
        script, target_browser->GetMainFrame()->GetURL(), 0);
  } else if (parts[0] == "mouse_wheel" && parts.size() >= 6) {
    const double dx = std::atof(parts[3].c_str());
    const double dy = std::atof(parts[4].c_str());
    const uint32_t modifiers = std::strtoul(parts[5].c_str(), nullptr, 10);
    if (pointer_guest && pointer_guest->intercept_horizontal_wheel &&
        IsPredominantlyHorizontalWheel(dx, dy)) {
      EmitPrimaryEvent(
          "guest.wheel",
          GuestWheelJson(pointer_guest->id, dx, dy, modifiers));
      return;
    }
    CefMouseEvent event;
    event.x = pointer_x;
    event.y = pointer_y;
    event.modifiers = modifiers;
    host->SendMouseWheelEvent(event, static_cast<int>(dx),
                              static_cast<int>(dy));
  } else if (parts[0] == "key" && parts.size() >= 6) {
    const bool pressed = std::atoi(parts[1].c_str()) != 0;
    const std::string key = DecodeUriComponent(parts[2]);
    const std::string text = DecodeUriComponent(parts[3]);
    const uint32_t modifiers = std::strtoul(parts[4].c_str(), nullptr, 10);
    const bool repeat =
        (parts.size() >= 6 && std::atoi(parts[5].c_str()) != 0) ||
        (modifiers & kGuestModRepeat) != 0;
    GuestView* focused_guest =
        focused_guest_id_.empty() ? nullptr : guests_.Find(focused_guest_id_);
    if (focused_guest && !focused_guest->intercepted_shortcuts.empty()) {
      if (const std::string* accelerator = MatchInterceptedShortcut(
              focused_guest->intercepted_shortcuts, key, modifiers)) {
        if (pressed) {
          EmitPrimaryEvent("guest.shortcut",
                           GuestShortcutJson(focused_guest->id, *accelerator,
                                             key, repeat, modifiers));
        }
        return;
      }
    }
    const int key_code = KeyCodeForName(key);
    CefKeyEvent event;
    event.type = pressed ? KEYEVENT_RAWKEYDOWN : KEYEVENT_KEYUP;
    event.modifiers = modifiers;
    event.windows_key_code = key_code;
    host->SendKeyEvent(event);
    if (pressed && !text.empty()) {
      for (char16_t ch : Utf8ToUtf16(text)) {
        CefKeyEvent char_event;
        char_event.type = KEYEVENT_CHAR;
        char_event.modifiers = modifiers;
        char_event.windows_key_code = ch;
        char_event.native_key_code = ch;
        char_event.character = ch;
        char_event.unmodified_character = ch;
        host->SendKeyEvent(char_event);
      }
    }
	  } else if (parts[0] == "focus" && parts.size() >= 2) {
	    const bool focused = std::atoi(parts[1].c_str()) != 0;
	    GuestView* guest = guests_.Find(focused_guest_id_);
	    if (guest && guest->browser) {
	      guest->browser->GetHost()->SetFocus(focused);
	    } else {
	      host->SetFocus(focused);
	    }
	  } else if (parts[0] == "lifecycle" && parts.size() >= 3) {
	    const std::string reason =
	        parts.size() >= 4 ? DecodeUriComponent(parts[3]) : "";
	    ApplyLifecycle(parts[1], std::max(1, std::atoi(parts[2].c_str())),
	                   reason);
	  } else if (parts[0] == "close") {
    // Close only this browser. CefQuitMessageLoop runs from OnBeforeClose
    // when the last OSR handler is gone so sibling windows stay alive.
    if (close_requested_) {
      return;
    }
    close_requested_ = true;
    host->CloseBrowser(false);
	  } else if (parts[0] == "file_drag_ended" && parts.size() >= 4) {
	    FinishNativeFileDrag(std::atoi(parts[1].c_str()),
	                         std::atoi(parts[2].c_str()), parts[3]);
	  }
	}

void SabineOsrHandler::ApplyLifecycle(const std::string& state,
                                        int frame_rate,
                                        const std::string& reason) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser_) {
    return;
  }
  CefRefPtr<CefBrowserHost> host = browser_->GetHost();
  if (state == "active") {
    const bool was_hidden = view_hidden_;
    suspended_ = false;
    view_hidden_ = false;
    host->SetWindowlessFrameRate(std::max(1, frame_rate));
    if (was_hidden) {
      host->WasHidden(false);
      host->WasResized();
      host->Invalidate(PET_VIEW);
    }
    ApplyGuestLifecycle();
    DispatchLifecycle("active", reason);
    return;
  }
  if (state == "hibernate") {
    DispatchLifecycle("hibernate", reason);
    suspended_ = true;
    view_hidden_ = true;
    host->SetWindowlessFrameRate(std::max(1, background_frame_rate_));
    host->WasHidden(true);
    ApplyGuestLifecycle();
    return;
  }
  // Suspend: throttle only. Do not WasHidden — Wayland fires brief blur /
  // occlusion around interactive move, and hiding blanks the OSR surface.
  suspended_ = true;
  host->SetWindowlessFrameRate(std::max(1, frame_rate));
  ApplyGuestLifecycle();
  DispatchLifecycle("suspended", reason);
}

void SabineOsrHandler::DispatchLifecycle(const std::string& state,
                                           const std::string& reason) {
  CEF_REQUIRE_UI_THREAD();
  const std::string script =
      "window.__sabineLifecycleSet&&window.__sabineLifecycleSet(" +
      JsString(state) + "," + JsString(reason) + ");";
  for (auto& browser : browsers_) {
    browser->GetMainFrame()->ExecuteJavaScript(
        script, browser->GetMainFrame()->GetURL(), 0);
  }
}
