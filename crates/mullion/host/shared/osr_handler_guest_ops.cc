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
#include "mullion_bridge_js.h"
#include "osr_handler_util.h"

using namespace mullion_osr;

bool MullionOsrHandler::RunGuestOperation(const std::string& operation,
                                           const std::string& payload,
                                           std::string* response,
                                           std::string* error) {
  CEF_REQUIRE_UI_THREAD();
  *response = "{}";
  if (operation == "list") {
    const GuestRegistry& registry = guests_;
    *response = GuestListJson(registry.InZOrder());
    return true;
  }
  if (operation == "setCovered") {
    const bool covered = JsonBoolValue(payload, "covered", false);
    if (guests_.Covered() == covered) {
      return true;
    }
    guests_.SetCovered(covered);
    pending_guest_cover_ = covered;
    if (covered) {
      FocusGuest(std::string());
    }
    for (GuestView* guest : guests_.InZOrder()) {
      ApplyGuestVisibility(*guest);
    }
    if (covered && browser_) {
      browser_->GetHost()->Invalidate(PET_VIEW);
    }
    return true;
  }
  if (operation == "downloadAction") {
    return RunGuestDownloadAction(payload, error);
  }

  const std::string id = JsonStringValue(payload, "id");
  if (id.empty()) {
    *error = "guest." + operation + " requires an `id`";
    return false;
  }
  if (operation == "destroy") {
    if (!guests_.Find(id) && !HasPendingGuest(id)) {
      *error = "unknown guest id";
      return false;
    }
    DestroyGuest(id);
    *response = GuestIdJson(id);
    return true;
  }

  GuestView* guest = guests_.Find(id);
  if (!guest) {
    *error = "unknown guest id";
    return false;
  }
  CefRefPtr<CefBrowser> browser = guest->browser;
  if (operation == "get") {
    *response = GuestInfoJson(*guest);
    return true;
  }
  if (operation == "setBounds") {
    guest->bounds = ParseGuestBounds(payload, guest->bounds);
    ApplyGuestBounds(*guest);
    *response = GuestInfoJson(*guest);
    return true;
  }
  if (operation == "setVisible") {
    const bool visible = JsonBoolValue(payload, "visible", true);
    if (guest->visible != visible) {
      guest->visible = visible;
      ApplyGuestVisibility(*guest);
    }
    *response = GuestInfoJson(*guest);
    return true;
  }
  if (operation == "focus") {
    FocusGuest(id);
    return true;
  }
  if (!browser) {
    *error = "guest browser is not ready";
    return false;
  }
  if (operation == "navigate") {
    const std::string url = JsonStringValue(payload, "url");
    const std::string html = JsonStringValue(payload, "html");
    if (url.empty() && html.empty()) {
      *error = "guest.navigate requires a `url` or `html`";
      return false;
    }
    guest->url = url.empty() ? HtmlDataUri(html) : url;
    browser->GetMainFrame()->LoadURL(guest->url);
    return true;
  }
  if (operation == "reload") {
    if (JsonBoolValue(payload, "ignoreCache", false)) {
      browser->ReloadIgnoreCache();
    } else {
      browser->Reload();
    }
    return true;
  }
  if (operation == "goBack") {
    if (!browser->CanGoBack()) {
      *error = "guest cannot go back";
      return false;
    }
    browser->GoBack();
    return true;
  }
  if (operation == "goForward") {
    if (!browser->CanGoForward()) {
      *error = "guest cannot go forward";
      return false;
    }
    browser->GoForward();
    return true;
  }
  if (operation == "setZoom") {
    const double factor = JsonDoubleValue(
        payload, "zoomFactor", JsonDoubleValue(payload, "factor", 1.0));
    guest->zoom = factor;
    browser->GetHost()->SetZoomLevel(GuestZoomLevel(factor));
    *response = GuestInfoJson(*guest);
    return true;
  }
  if (operation == "executeJavaScript") {
    std::string code = JsonStringValue(payload, "code");
    if (code.empty()) {
      code = JsonStringValue(payload, "script");
    }
    if (code.empty()) {
      *error = "guest.executeJavaScript requires `code`";
      return false;
    }
    CefRefPtr<CefFrame> frame = browser->GetMainFrame();
    frame->ExecuteJavaScript(code, frame->GetURL(), 0);
    return true;
  }
  *error = "unsupported guest operation";
  return false;
}

bool MullionOsrHandler::HandleGuestBridgeCommand(const std::string& command,
                                                  const std::string& payload,
                                                  const std::string& browser_id,
                                                  const std::string& request_id) {
  if (command == "mullion.popup.open") {
    GuestCreateRequest request;
    std::string parse_error;
    if (!ParseGuestCreateRequest(payload, &request, &parse_error)) {
      ResolveBridgeResponse(browser_id, request_id, false,
                            JsonMessage(parse_error));
      return true;
    }
    request.id = kMullionPopupGuestId;
    request.allow_bridge = false;
    request.allow_downloads = false;
    request.visible = true;
    CefRefPtr<MullionOsrHandler> self(this);
    CreateGuest(
        std::move(request),
        [self, browser_id, request_id](bool success, const std::string& result) {
          self->ResolveBridgeResponse(
              browser_id, request_id, success,
              success ? "{\"accepted\":true}" : JsonMessage(result));
        });
    return true;
  }
  if (command == "mullion.popup.close") {
    DismissPopupGuest();
    ResolveBridgeResponse(browser_id, request_id, true, "{}");
    return true;
  }
  if (!IsGuestBridgeCommand(command)) {
    return false;
  }

  const std::string operation = GuestOperationName(command, kGuestBridgePrefix);
  if (operation == "create") {
    GuestCreateRequest request;
    std::string error;
    if (!ParseGuestCreateRequest(payload, &request, &error)) {
      ResolveBridgeResponse(browser_id, request_id, false, JsonMessage(error));
      return true;
    }
    if (request.id.empty()) {
      request.id = NextGuestId();
    }
    CefRefPtr<MullionOsrHandler> self(this);
    CreateGuest(
        std::move(request),
        [self, browser_id, request_id](bool success, const std::string& result) {
          self->ResolveBridgeResponse(browser_id, request_id, success,
                                      success ? result : JsonMessage(result));
        });
    return true;
  }
  if (operation == "capturePreview") {
    const std::string id = JsonStringValue(payload, "id");
    GuestView* guest = guests_.Find(id);
    if (!guest || !guest->browser) {
      ResolveBridgeResponse(browser_id, request_id, false,
                            JsonMessage("unknown guest id"));
      return true;
    }
    if (!guest->visible || view_hidden_ || guests_.Covered()) {
      ResolveBridgeResponse(browser_id, request_id, false,
                            JsonMessage("guest browser is not visible"));
      return true;
    }
    std::string request = browser_id;
    request.push_back('\0');
    request += request_id;
    request.push_back('\0');
    request += id;
    if (!SendMessage(kGuestCaptureRequested, 0, 0, 0, 0, request.data(),
                     static_cast<uint32_t>(request.size()))) {
      ResolveBridgeResponse(browser_id, request_id, false,
                            JsonMessage("failed to request guest capture"));
    }
    return true;
  }
  std::string response;
  std::string error;
  if (!RunGuestOperation(operation, payload, &response, &error)) {
    ResolveBridgeResponse(browser_id, request_id, false, JsonMessage(error));
    return true;
  }
  ResolveBridgeResponse(browser_id, request_id, true, response);
  return true;
}

void MullionOsrHandler::ApplyHostControl(const std::string& command,
                                          const std::string& value) {
  CEF_REQUIRE_UI_THREAD();
  const std::string operation =
      GuestOperationName(command, kGuestHostControlPrefix);
  if (operation.empty()) {
    return;
  }
  if (operation == "create") {
    GuestCreateRequest request;
    std::string error;
    if (!ParseGuestCreateRequest(value, &request, &error)) {
      std::cerr << "guest.create failed: " << error << std::endl;
      return;
    }
    if (request.id.empty()) {
      request.id = NextGuestId();
    }
    CreateGuest(std::move(request), [](bool success, const std::string& result) {
      if (!success) {
        std::cerr << "guest.create failed: " << result << std::endl;
      }
    });
    return;
  }
  std::string response;
  std::string error;
  if (!RunGuestOperation(operation, value, &response, &error)) {
    std::cerr << "guest." << operation << " failed: " << error << std::endl;
  }
}

