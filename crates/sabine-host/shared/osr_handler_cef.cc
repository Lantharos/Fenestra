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
#include "osr_handler_util.h"

using namespace sabine_osr;

bool SabineOsrHandler::OnCursorChange(CefRefPtr<CefBrowser> browser,
                                    CefCursorHandle cursor,
                                    cef_cursor_type_t type,
                                    const CefCursorInfo& custom_cursor_info) {
  const std::string name = CursorName(type);
  SendMessage(4, 0, 0, 0, 0, name.data(), static_cast<uint32_t>(name.size()));
  return true;
}

void SabineOsrHandler::OnBeforeContextMenu(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    CefRefPtr<CefContextMenuParams> params,
    CefRefPtr<CefMenuModel> model) {
  CEF_REQUIRE_UI_THREAD();
  model->Clear();
}

void SabineOsrHandler::OnDraggableRegionsChanged(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    const std::vector<CefDraggableRegion>& regions) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser_ || !browser_->IsSame(browser)) {
    return;
  }
  constexpr size_t kEntryLen = 20;
  std::vector<char> payload(4 + regions.size() * kEntryLen, 0);
  PutU32(&payload, 0, static_cast<uint32_t>(regions.size()));
  for (size_t index = 0; index < regions.size(); ++index) {
    const auto& region = regions[index];
    const size_t offset = 4 + index * kEntryLen;
    PutI32(&payload, offset, region.bounds.x);
    PutI32(&payload, offset + 4, region.bounds.y);
    PutI32(&payload, offset + 8, region.bounds.width);
    PutI32(&payload, offset + 12, region.bounds.height);
    PutU32(&payload, offset + 16, region.draggable ? 1 : 0);
  }
  SendMessage(kDraggableRegionsChanged, 0, 0, 0, 0, payload.data(),
              static_cast<uint32_t>(payload.size()));
}

bool SabineOsrHandler::OnContextMenuCommand(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    CefRefPtr<CefContextMenuParams> params,
    int command_id,
    EventFlags event_flags) {
  CEF_REQUIRE_UI_THREAD();
  return true;
}

void SabineOsrHandler::OnAfterCreated(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  browsers_.push_back(browser);
  const bool primary_browser = !browser_;
  if (primary_browser) {
    browser_ = browser;
  } else if (GuestView* pending = guests_.Find(pending_guest_id_)) {
    if (!pending->browser) {
      pending->browser = browser;
      pending->pending = false;
    }
  }
  CefRefPtr<CefBrowserHost> host = browser->GetHost();
  host->SetWindowlessFrameRate(
      std::max(1, suspended_ ? background_frame_rate_ : active_frame_rate_));
  if (!primary_browser) {
    return;
  }
  if (std::getenv("SABINE_TRACE")) {
    std::fprintf(stderr, "Sabine CEF: primary browser ready windowless=%d\n",
                 host->IsWindowRenderingDisabled() ? 1 : 0);
    std::fflush(stderr);
  }
  if (!ConnectSocket()) {
    std::fprintf(stderr, "Sabine OSR: failed to connect native host\n");
    std::fflush(stderr);
    return;
  }
  StartCommandReader();
  host->WasResized();
  host->Invalidate(PET_VIEW);
}

bool SabineOsrHandler::DoClose(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  if (browsers_.size() == 1) {
    closing_ = true;
  }
  return false;
}

void SabineOsrHandler::OnBeforeClose(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  if (GuestView* guest = guests_.FindByBrowser(browser)) {
    const std::string id = guest->id;
    guest->browser = nullptr;
    DestroyGuest(id);
  } else if (browser_ && browser_->IsSame(browser)) {
    std::vector<std::string> ids;
    for (const GuestView* view : guests_.InZOrder()) {
      ids.push_back(view->id);
    }
    for (const auto& id : ids) {
      DestroyGuest(id);
    }
  }
  for (auto it = browsers_.begin(); it != browsers_.end(); ++it) {
    if ((*it)->IsSame(browser)) {
      browsers_.erase(it);
      break;
    }
  }
  if (browsers_.empty()) {
    UnregisterHandler(this);
    if (g_instance == this) {
      g_instance = nullptr;
      const auto remaining = SnapshotHandlers();
      if (!remaining.empty()) {
        g_instance = remaining.front();
      }
    }
    // Only quit when every OSR window/handler is gone — multi-window apps share
    // one CEF process via the profile singleton handoff path.
    if (!HasRegisteredHandlers()) {
      CefQuitMessageLoop();
    }
  }
}

void SabineOsrHandler::OnLoadError(CefRefPtr<CefBrowser> browser,
                                 CefRefPtr<CefFrame> frame,
                                 ErrorCode errorCode,
                                 const CefString& errorText,
                                 const CefString& failedUrl) {
  CEF_REQUIRE_UI_THREAD();
  if (errorCode == ERR_ABORTED) {
    return;
  }
  if (GuestView* guest = GuestForBrowser(browser)) {
    std::cerr << "guest " << guest->id << " load failed: " << errorText.ToString()
              << " url=" << failedUrl.ToString() << std::endl;
    if (guest->id == kSabinePopupGuestId) {
      DestroyGuest(guest->id);
    }
    return;
  }
  std::stringstream body;
  body << "<!doctype html><meta charset=\"utf-8\"><body style=\"margin:0;"
          "font:14px system-ui;background:#111;color:#eee;padding:24px\">"
       << "<h2>Failed to load</h2><p>" << std::string(failedUrl) << "</p><p>"
       << std::string(errorText) << "</p></body>";
  frame->LoadURL(HtmlDataUri(body.str()));
}

void SabineOsrHandler::OnLoadStart(CefRefPtr<CefBrowser> browser,
                                 CefRefPtr<CefFrame> frame,
                                 TransitionType transition_type) {
  CEF_REQUIRE_UI_THREAD();
  if (!frame->IsMain()) {
    return;
  }
  GuestView* guest = GuestForBrowser(browser);
  if (!guest) {
    InstallTransparentBackground(frame);
  }
  if (!guest || guest->allow_bridge) {
    InstallBridge(browser, frame);
  }
}

void SabineOsrHandler::OnLoadEnd(CefRefPtr<CefBrowser> browser,
                               CefRefPtr<CefFrame> frame,
                               int httpStatusCode) {
  CEF_REQUIRE_UI_THREAD();
  if (!frame->IsMain()) {
    return;
  }
  GuestView* guest = GuestForBrowser(browser);
  if (!guest) {
    InstallTransparentBackground(frame);
    InstallBridge(browser, frame);
    return;
  }
  if (guest->allow_bridge) {
    InstallBridge(browser, frame);
  }
  guest->url = frame->GetURL();
  EmitPrimaryEvent("guest.navigated", GuestNavigatedJson(*guest));
}

void SabineOsrHandler::OnLoadingStateChange(CefRefPtr<CefBrowser> browser,
                                              bool isLoading,
                                              bool canGoBack,
                                              bool canGoForward) {
  CEF_REQUIRE_UI_THREAD();
  GuestView* guest = GuestForBrowser(browser);
  if (!guest || guest->loading == isLoading) {
    return;
  }
  guest->loading = isLoading;
  EmitPrimaryEvent("guest.loading",
                   "{\"id\":\"" + JsonEscape(guest->id) + "\",\"loading\":" +
                       (isLoading ? "true" : "false") + "}");
}

void SabineOsrHandler::OnTitleChange(CefRefPtr<CefBrowser> browser,
                                       const CefString& title) {
  CEF_REQUIRE_UI_THREAD();
  GuestView* guest = GuestForBrowser(browser);
  if (!guest) {
    return;
  }
  guest->title = title.ToString();
  EmitPrimaryEvent("guest.title", "{\"id\":\"" + JsonEscape(guest->id) +
                                      "\",\"title\":\"" +
                                      JsonEscape(guest->title) + "\"}");
}

void SabineOsrHandler::OnAddressChange(CefRefPtr<CefBrowser> browser,
                                         CefRefPtr<CefFrame> frame,
                                         const CefString& url) {
  CEF_REQUIRE_UI_THREAD();
  if (!frame || !frame->IsMain()) {
    return;
  }
  GuestView* guest = GuestForBrowser(browser);
  if (!guest) {
    return;
  }
  guest->url = url.ToString();
  EmitPrimaryEvent("guest.navigated", GuestNavigatedJson(*guest));
}

void SabineOsrHandler::OnFaviconURLChange(
    CefRefPtr<CefBrowser> browser,
    const std::vector<CefString>& icon_urls) {
  CEF_REQUIRE_UI_THREAD();
  GuestView* guest = GuestForBrowser(browser);
  if (!guest) {
    return;
  }
  std::vector<std::string> favicons;
  favicons.reserve(icon_urls.size());
  for (const CefString& icon : icon_urls) {
    favicons.push_back(icon.ToString());
  }
  EmitPrimaryEvent("guest.favicon", GuestFaviconJson(guest->id, favicons));
}

bool SabineOsrHandler::OnBeforeBrowse(CefRefPtr<CefBrowser> browser,
                                    CefRefPtr<CefFrame> frame,
                                    CefRefPtr<CefRequest> request,
                                    bool user_gesture,
                                    bool is_redirect) {
  CEF_REQUIRE_UI_THREAD();
  const std::string url = request->GetURL();
  return HandleWindowCommand(browser, url) || HandleBridgeCommand(browser, frame, url);
}

bool SabineOsrHandler::GetScreenInfo(CefRefPtr<CefBrowser> browser,
                                   CefScreenInfo& screen_info) {
  screen_info.device_scale_factor = scale_;
  screen_info.depth = 32;
  screen_info.depth_per_component = 8;
  if (const GuestView* guest = GuestForBrowser(browser)) {
    screen_info.rect = CefRect(0, 0, guest->bounds.width, guest->bounds.height);
    screen_info.available_rect = screen_info.rect;
    return true;
  }
  screen_info.rect = CefRect(0, 0, width_, height_);
  screen_info.available_rect = CefRect(0, 0, width_, height_);
  return true;
}

void SabineOsrHandler::GetViewRect(CefRefPtr<CefBrowser> browser, CefRect& rect) {
  if (const GuestView* guest = GuestForBrowser(browser)) {
    rect = CefRect(0, 0, guest->bounds.width, guest->bounds.height);
    return;
  }
  rect = CefRect(0, 0, width_, height_);
}

void SabineOsrHandler::OnPopupShow(CefRefPtr<CefBrowser> browser, bool show) {
  if (GuestView* guest = GuestForBrowser(browser)) {
    if (!show) {
      const std::string popup_id = guest->id + "/popup";
      SendMessage(kGuestHidden, 0, 0, guest->bounds.x + guest_popup_rect_.x,
                  guest->bounds.y + guest_popup_rect_.y, popup_id.data(),
                  static_cast<uint32_t>(popup_id.size()));
      guest_popup_rect_ = CefRect();
    }
    return;
  }
  if (!show) {
    SendMessage(kPopupHidden, 0, 0, 0, 0, nullptr, 0);
  }
}

void SabineOsrHandler::OnPopupSize(CefRefPtr<CefBrowser> browser,
                                 const CefRect& rect) {
  if (GuestForBrowser(browser)) {
    if (guest_popup_rect_.x != rect.x || guest_popup_rect_.y != rect.y ||
        guest_popup_rect_.width != rect.width ||
        guest_popup_rect_.height != rect.height) {
      // Force compositor to drop the previous guest popup overlay bounds.
      if (GuestView* guest = GuestForBrowser(browser)) {
        const std::string popup_id = guest->id + "/popup";
        SendMessage(kGuestHidden, 0, 0, guest->bounds.x + guest_popup_rect_.x,
                    guest->bounds.y + guest_popup_rect_.y, popup_id.data(),
                    static_cast<uint32_t>(popup_id.size()));
      }
    }
    guest_popup_rect_ = rect;
    return;
  }
  if (popup_rect_.x != rect.x || popup_rect_.y != rect.y ||
      popup_rect_.width != rect.width || popup_rect_.height != rect.height) {
    SendMessage(kPopupHidden, 0, 0, 0, 0, nullptr, 0);
  }
  popup_rect_ = rect;
}

void SabineOsrHandler::OnPaint(CefRefPtr<CefBrowser> browser,
                               PaintElementType type,
                               const RectList& dirtyRects,
                               const void* buffer,
                               int width,
                               int height) {
  static bool traced_first_software_paint = false;
  if (!traced_first_software_paint && std::getenv("SABINE_TRACE")) {
    traced_first_software_paint = true;
    std::fprintf(stderr, "Sabine CEF: first software paint size=%dx%d\n", width,
                 height);
    std::fflush(stderr);
  }
  if (GuestView* guest = GuestForBrowser(browser)) {
    if (type == PET_POPUP) {
      SendPaintBatch(kGuestFrame, guest->id + "/popup",
                     guest->bounds.x + guest_popup_rect_.x,
                     guest->bounds.y + guest_popup_rect_.y, buffer, width,
                     height, dirtyRects);
      return;
    }
    if (!guest->painted) {
      guest->painted = true;
      if (guest->id == kSabinePopupGuestId) {
        EmitBridgeEvent("\"popup.open\"", "{}");
      }
    }
    if (guest->visible) {
      SendGuestPaint(*guest, buffer, width, height, dirtyRects);
    }
    return;
  }
  if (view_hidden_ || !browser_ || !browser_->IsSame(browser)) {
    return;
  }
  const uint32_t kind = type == PET_POPUP ? kPopupFrame : kMainFrame;
  const int32_t x = type == PET_POPUP ? popup_rect_.x : 0;
  const int32_t y = type == PET_POPUP ? popup_rect_.y : 0;
  const bool sent =
      SendPaintBatch(kind, std::string(), x, y, buffer, width, height, dirtyRects);
  if (sent && type == PET_VIEW && pending_guest_cover_ && guests_.Covered()) {
    pending_guest_cover_ = false;
    for (GuestView* guest : guests_.InZOrder()) {
      if (guest->visible) {
        SendGuestHidden(*guest);
      }
    }
  }
}
