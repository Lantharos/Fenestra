#include "osr/handler.h"

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

#include "guest/input.h"
#include "guest/manager.h"
#include "include/cef_app.h"
#include "include/cef_browser.h"
#include "include/cef_parser.h"
#include "include/cef_request_context_handler.h"
#include "include/cef_task.h"
#include "include/internal/cef_types.h"
#include "include/wrapper/cef_helpers.h"
#include "common/json.h"
#include "sabine_bridge_js.h"
#include "osr/utilities.h"

using namespace sabine_osr;

bool SabineOsrHandler::OnProcessMessageReceived(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    CefProcessId source_process,
    CefRefPtr<CefProcessMessage> message) {
  CEF_REQUIRE_UI_THREAD();
  if (source_process != PID_RENDERER || !message ||
      message->GetName() != "sabine.native") {
    return false;
  }
  CefRefPtr<CefListValue> arguments = message->GetArgumentList();
  if (!arguments || arguments->GetSize() != 1 ||
      arguments->GetType(0) != VTYPE_STRING) {
    return true;
  }
  const std::string payload = arguments->GetString(0);
  return HandleWindowCommand(browser, payload) ||
         HandleBridgeCommand(browser, frame, payload);
}

bool SabineOsrHandler::HandleWindowCommand(CefRefPtr<CefBrowser> browser,
	                                         const std::string& url) {
  const std::string prefix = "sabine://window/";
  if (url.rfind(prefix, 0) != 0) {
    return false;
  }
  std::string command = url.substr(prefix.size());
  const size_t query = command.find_first_of("?#");
  if (query != std::string::npos) {
    command = command.substr(0, query);
  }
  if (command == "close") {
    // Ask the native host to tear down its window; it replies with "close\n"
    // which CloseBrowsers this surface. Do not quit the shared CEF process.
    RequestNativeClose();
  } else if (command == "start-drag" || command == "drag") {
    SendMessage(6, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "minimize") {
    SendMessage(7, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "maximize" || command == "toggle-maximize") {
    SendMessage(8, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "fullscreen") {
    SendMessage(27, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "exit-fullscreen") {
    SendMessage(28, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "show") {
    SendMessage(9, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "hide") {
    SendMessage(10, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "focus") {
    const std::string activation_token = QueryValue(url, "activationToken");
    SendMessage(11, 0, 0, 0, 0, activation_token.data(),
                static_cast<uint32_t>(activation_token.size()));
  }
  return true;
}

bool SabineOsrHandler::HandleBridgeCommand(CefRefPtr<CefBrowser> browser,
                                         CefRefPtr<CefFrame> frame,
                                         const std::string& url) {
  const std::string prefix = "sabine://bridge/";
  if (url.rfind(prefix, 0) != 0) {
    return false;
  }
  const std::string request_id = BridgeRequestId(url);
  const std::string command = QueryValue(url, "name");
  const std::string payload = QueryValue(url, "payload");
  const std::string browser_id = std::to_string(browser->GetIdentifier());
  const std::string origin = UrlOrigin(frame ? std::string(frame->GetURL()) : "");
  if (request_id.empty() || command.empty()) {
    ResolveBridgeResponse(browser_id, request_id, false,
                          "{\"message\":\"Malformed Sabine bridge request\"}");
    return true;
  }
  const GuestView* guest = GuestForBrowser(browser);
  if (guest && !guest->allow_bridge) {
    ResolveBridgeResponse(
        browser_id, request_id, false,
        "{\"message\":\"Sabine bridge is unavailable inside guest views\"}");
    return true;
  }
  if (guest) {
    if (IsGuestBridgeCommand(command) || command == "sabine.popup.open" ||
        command == "sabine.popup.close") {
      ResolveBridgeResponse(browser_id, request_id, false,
                            "{\"message\":\"Guest views cannot manage other "
                            "guest views\"}");
      return true;
    }
  } else if (HandleGuestBridgeCommand(command, payload, browser_id,
                                      request_id)) {
    return true;
  }
  if (bridge_commands_.find(command) == bridge_commands_.end()) {
    ResolveBridgeResponse(
        browser_id, request_id, false,
        "{\"message\":\"Sabine bridge command is not allowlisted\"}");
    return true;
  }
  const std::string request_line =
      "SABINE_BRIDGE_REQUEST\t" + browser_id + "\t" + request_id + "\t" +
      origin + "\t" + command + "\t" + (payload.empty() ? "{}" : payload);
  SendMessage(kBridgeRequest, 0, 0, 0, 0, request_line.data(),
              static_cast<uint32_t>(request_line.size()));
  return true;
}

void SabineOsrHandler::RequestNativeClose() {
  SendMessage(5, 0, 0, 0, 0, nullptr, 0);
}

void SabineOsrHandler::CloseFromNativeDisconnect() {
  CEF_REQUIRE_UI_THREAD();
  if (close_requested_) {
    return;
  }
  close_requested_ = true;
  if (browser_) {
    browser_->GetHost()->CloseBrowser(false);
  }
}

void SabineOsrHandler::InstallBridge(CefRefPtr<CefBrowser> browser,
	                                   CefRefPtr<CefFrame> frame) {
	  frame->ExecuteJavaScript(BridgeInstallScript(bridge_commands_), frame->GetURL(),
	                           0);
	}

void SabineOsrHandler::InstallTransparentBackground(CefRefPtr<CefFrame> frame) {
  if (!transparent_background_) {
    return;
  }
  frame->ExecuteJavaScript(
      "(function(){"
      "if(document.documentElement){document.documentElement.style.background='transparent';}"
      "if(document.body){document.body.style.background='transparent';}"
      "if(!document.querySelector('style[data-sabine-transparent-background]')){"
      "const style=document.createElement('style');"
      "style.setAttribute('data-sabine-transparent-background','');"
      "style.textContent='html,body{background:transparent!important;}';"
      "document.head&&document.head.appendChild(style);"
      "}"
      "})();",
      frame->GetURL(), 0);
}

void SabineOsrHandler::ResolveBridgeResponse(const std::string& browser_id,
                                           const std::string& request_id,
                                           bool ok,
                                           const std::string& payload) {
  CEF_REQUIRE_UI_THREAD();
  const int expected_id = std::atoi(browser_id.c_str());
  CefRefPtr<CefBrowser> target;
  for (auto& browser : browsers_) {
    if (browser->GetIdentifier() == expected_id) {
      target = browser;
      break;
    }
  }
  if (!target || request_id.empty()) {
    return;
  }
  const std::string script =
      "window.__sabineBridgeResolve&&window.__sabineBridgeResolve(" +
      JsString(request_id) + "," + (ok ? "true" : "false") + "," +
      (payload.empty() ? "null" : payload) + ");";
  target->GetMainFrame()->ExecuteJavaScript(script, target->GetMainFrame()->GetURL(),
                                            0);
}

void SabineOsrHandler::EmitBridgeEvent(const std::string& name_json,
                                         const std::string& payload) {
  CEF_REQUIRE_UI_THREAD();
  const std::string script =
      "window.__sabineBridgeEmit&&window.__sabineBridgeEmit(" +
      name_json + "," + (payload.empty() ? "null" : payload) + ");";
  for (auto& browser : browsers_) {
    const GuestView* guest = guests_.FindByBrowser(browser);
    if (guest && !guest->allow_bridge) {
      continue;
    }
    browser->GetMainFrame()->ExecuteJavaScript(
        script, browser->GetMainFrame()->GetURL(), 0);
  }
}

void SabineOsrHandler::EmitPrimaryEvent(const std::string& name,
                                        const std::string& payload) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser_) {
    return;
  }
  CefRefPtr<CefFrame> frame = browser_->GetMainFrame();
  if (!frame) {
    return;
  }
  frame->ExecuteJavaScript(
      "window.__sabineBridgeEmit&&window.__sabineBridgeEmit(" +
          JsString(name) + "," + (payload.empty() ? "null" : payload) + ");",
      frame->GetURL(), 0);
}
