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

class SabineGuestRequestContextHandler : public CefRequestContextHandler {
 public:
  SabineGuestRequestContextHandler(CefRefPtr<SabineOsrHandler> owner,
                                     std::string partition)
      : owner_(std::move(owner)), partition_(std::move(partition)) {}

  void OnRequestContextInitialized(
      CefRefPtr<CefRequestContext> request_context) override {
    CEF_REQUIRE_UI_THREAD();
    CefRefPtr<SabineOsrHandler> owner = owner_;
    owner_ = nullptr;
    if (owner) {
      owner->GuestRequestContextInitialized(partition_, request_context);
    }
  }

 private:
  CefRefPtr<SabineOsrHandler> owner_;
  const std::string partition_;

  IMPLEMENT_REFCOUNTING(SabineGuestRequestContextHandler);
};

GuestView* SabineOsrHandler::GuestForBrowser(
    const CefRefPtr<CefBrowser>& browser) {
  if (!browser) {
    return nullptr;
  }
  if (GuestView* guest = guests_.FindByBrowser(browser)) {
    return guest;
  }
  if (pending_guest_id_.empty() || (browser_ && browser_->IsSame(browser))) {
    return nullptr;
  }
  GuestView* pending = guests_.Find(pending_guest_id_);
  return pending && !pending->browser ? pending : nullptr;
}

std::string SabineOsrHandler::NextGuestId() {
  std::string id;
  do {
    id = "guest-" + std::to_string(++guest_serial_);
  } while (guests_.Find(id));
  return id;
}

CefRefPtr<CefRequestContext> SabineOsrHandler::CreateGuestRequestContext(
    const std::string& partition) {
  CEF_REQUIRE_UI_THREAD();
  if (partition.empty()) {
    return nullptr;
  }
  const auto cached = guest_contexts_.find(partition);
  if (cached != guest_contexts_.end()) {
    return cached->second;
  }
  CefRefPtr<CefCommandLine> command_line =
      CefCommandLine::GetGlobalCommandLine();
  const std::string root =
      command_line ? command_line->GetSwitchValue("root-cache-path").ToString()
                   : std::string();
  CefRequestContextSettings settings;
  const std::string cache_path = GuestCachePath(root, partition);
  if (!cache_path.empty()) {
    CefString(&settings.cache_path).FromString(cache_path);
  }
  CefRefPtr<SabineOsrHandler> self(this);
  CefRefPtr<CefRequestContext> context = CefRequestContext::CreateContext(
      settings, new SabineGuestRequestContextHandler(self, partition));
  if (context) {
    guest_contexts_[partition] = context;
  }
  return context;
}

void SabineOsrHandler::CreateGuest(GuestCreateRequest request,
                                     GuestCreateCallback callback) {
  CEF_REQUIRE_UI_THREAD();
  if (!IsValidGuestId(request.id)) {
    callback(false, "guest id is not valid");
    return;
  }
  CancelPendingGuest(request.id);
  if (guests_.Find(request.id)) {
    DestroyGuest(request.id);
  }

  request.partition =
      request.partition.empty() && request.id != kSabinePopupGuestId
          ? DefaultGuestPartition(request.id)
          : request.partition;
  if (request.partition.empty()) {
    ContinueCreateGuest(request, nullptr, std::move(callback));
    return;
  }

  const auto cached = guest_contexts_.find(request.partition);
  if (cached != guest_contexts_.end() &&
      initialized_guest_contexts_.find(request.partition) !=
          initialized_guest_contexts_.end()) {
    ContinueCreateGuest(request, cached->second, std::move(callback));
    return;
  }

  const std::string partition = request.partition;
  pending_guest_creates_[partition].push_back(
      PendingGuestCreate{std::move(request), std::move(callback)});
  if (cached != guest_contexts_.end()) {
    return;
  }
  if (!CreateGuestRequestContext(partition)) {
    GuestRequestContextInitialized(partition, nullptr);
  }
}

void SabineOsrHandler::GuestRequestContextInitialized(
    const std::string& partition,
    CefRefPtr<CefRequestContext> context) {
  CEF_REQUIRE_UI_THREAD();
  auto pending = pending_guest_creates_.find(partition);
  if (pending == pending_guest_creates_.end()) {
    if (context) {
      guest_contexts_[partition] = context;
      initialized_guest_contexts_.insert(partition);
    }
    return;
  }

  std::vector<PendingGuestCreate> creates = std::move(pending->second);
  pending_guest_creates_.erase(pending);
  if (!context) {
    guest_contexts_.erase(partition);
    for (PendingGuestCreate& create : creates) {
      create.callback(false, "failed to initialize the guest request context");
    }
    return;
  }

  guest_contexts_[partition] = context;
  initialized_guest_contexts_.insert(partition);
  for (PendingGuestCreate& create : creates) {
    ContinueCreateGuest(create.request, context, std::move(create.callback));
  }
}

void SabineOsrHandler::ContinueCreateGuest(
    const GuestCreateRequest& request,
    CefRefPtr<CefRequestContext> context,
    GuestCreateCallback callback) {
  CEF_REQUIRE_UI_THREAD();

  GuestView guest;
  guest.id = request.id;
  guest.url = request.url;
  guest.bounds = request.bounds;
  guest.partition = request.partition;
  guest.visible = request.visible;
  guest.allow_bridge = request.allow_bridge;
  guest.allow_downloads = request.allow_downloads;
  guest.intercepted_shortcuts = request.intercepted_shortcuts;
  guest.intercept_horizontal_wheel = request.intercept_horizontal_wheel;
  guest.popup_policy = request.popup_policy;
  guest.pending = true;
  guests_.Insert(std::move(guest));
  guests_.Raise(request.id);
  pending_guest_id_ = request.id;

  CefBrowserSettings settings;
  settings.windowless_frame_rate =
      std::max(1, suspended_ ? background_frame_rate_ : active_frame_rate_);
  settings.background_color = ParseGuestBackgroundColor(
      request.background_color, CefColorSetARGB(0, 0, 0, 0));

  CefWindowInfo window_info;
  window_info.SetAsWindowless(kNullWindowHandle);

  CefRefPtr<CefDictionaryValue> extra_info = CefDictionaryValue::Create();
  extra_info->SetBool("sabineAllowBridge", request.allow_bridge);
  extra_info->SetString("sabineGuestId", request.id);

  CefRefPtr<CefBrowser> browser =
      CefBrowserHost::CreateBrowserSync(window_info, this, request.url, settings,
                                       extra_info, context);
  pending_guest_id_.clear();

  GuestView* created = guests_.Find(request.id);
  if (!browser) {
    if (created) {
      guests_.Erase(request.id);
    }
    callback(false, "failed to create the guest browser");
    return;
  }
  if (!created) {
    browser->GetHost()->CloseBrowser(true);
    callback(false, "guest was destroyed while it was being created");
    return;
  }
  created->browser = browser;
  created->pending = false;
  ApplyGuestBounds(*created);
  ApplyGuestVisibility(*created);
  const std::string result = GuestInfoJson(*created);
  EmitPrimaryEvent("guest.created", result);
  callback(true, result);
}

bool SabineOsrHandler::CancelPendingGuest(const std::string& id) {
  CEF_REQUIRE_UI_THREAD();
  std::vector<GuestCreateCallback> callbacks;
  for (auto partition = pending_guest_creates_.begin();
       partition != pending_guest_creates_.end();) {
    auto& creates = partition->second;
    for (auto create = creates.begin(); create != creates.end();) {
      if (create->request.id == id) {
        callbacks.push_back(std::move(create->callback));
        create = creates.erase(create);
      } else {
        ++create;
      }
    }
    if (creates.empty()) {
      partition = pending_guest_creates_.erase(partition);
    } else {
      ++partition;
    }
  }
  for (GuestCreateCallback& callback : callbacks) {
    callback(false, "guest creation canceled");
  }
  return !callbacks.empty();
}

bool SabineOsrHandler::HasPendingGuest(const std::string& id) const {
  for (const auto& entry : pending_guest_creates_) {
    for (const PendingGuestCreate& create : entry.second) {
      if (create.request.id == id) {
        return true;
      }
    }
  }
  return false;
}

void SabineOsrHandler::DestroyGuest(const std::string& id) {
  CEF_REQUIRE_UI_THREAD();
  const bool canceled = CancelPendingGuest(id);
  GuestView* guest = guests_.Find(id);
  if (!guest) {
    if (canceled) {
      EmitPrimaryEvent("guest.destroyed", GuestIdJson(id));
    }
    return;
  }
  const GuestView removed = *guest;
  guests_.Erase(id);
  if (focused_guest_id_ == id) {
    focused_guest_id_.clear();
  }
  for (auto entry = downloads_.begin(); entry != downloads_.end();) {
    if (entry->second.guest_id == id) {
      if (entry->second.item_callback) {
        entry->second.item_callback->Cancel();
      }
      entry = downloads_.erase(entry);
    } else {
      ++entry;
    }
  }
  SendGuestHidden(removed);
  if (removed.browser) {
    removed.browser->GetHost()->CloseBrowser(true);
  }
  EmitPrimaryEvent("guest.destroyed", GuestIdJson(id));
  if (id == kSabinePopupGuestId) {
    EmitBridgeEvent("\"popup.close\"", "{}");
  }
}

void SabineOsrHandler::FocusGuest(const std::string& id) {
  CEF_REQUIRE_UI_THREAD();
  if (!id.empty() && !guests_.Find(id)) {
    return;
  }
  if (focused_guest_id_ != id) {
    if (GuestView* previous = guests_.Find(focused_guest_id_)) {
      if (previous->browser) {
        previous->browser->GetHost()->SetFocus(false);
      }
    }
  }
  focused_guest_id_ = id;
  if (id.empty()) {
    if (browser_) {
      browser_->GetHost()->SetFocus(true);
    }
    return;
  }
  guests_.Raise(id);
  if (browser_) {
    browser_->GetHost()->SetFocus(false);
  }
  GuestView* guest = guests_.Find(id);
  if (guest && guest->browser) {
    guest->browser->GetHost()->SetFocus(true);
  }
}

void SabineOsrHandler::DismissPopupGuest() {
  if (guests_.Find(kSabinePopupGuestId)) {
    DestroyGuest(kSabinePopupGuestId);
  }
}

void SabineOsrHandler::ApplyGuestBounds(GuestView& guest) {
  if (!guest.browser) {
    return;
  }
  CefRefPtr<CefBrowserHost> host = guest.browser->GetHost();
  host->NotifyScreenInfoChanged();
  host->WasResized();
  host->Invalidate(PET_VIEW);
}

void SabineOsrHandler::ApplyGuestVisibility(GuestView& guest) {
  if (!guest.browser) {
    return;
  }
  CefRefPtr<CefBrowserHost> host = guest.browser->GetHost();
  const bool hidden = !guest.visible || view_hidden_ || guests_.Covered();
  host->WasHidden(hidden);
  if (!hidden) {
    host->Invalidate(PET_VIEW);
    return;
  }
  if (!guest.visible) {
    SendGuestHidden(guest);
  }
  if (!guest.visible && focused_guest_id_ == guest.id) {
    host->SetFocus(false);
    focused_guest_id_.clear();
    if (browser_) {
      browser_->GetHost()->SetFocus(true);
    }
  }
}

void SabineOsrHandler::ApplyGuestLifecycle() {
  CEF_REQUIRE_UI_THREAD();
  const int frame_rate =
      std::max(1, suspended_ ? background_frame_rate_ : active_frame_rate_);
  for (GuestView* guest : guests_.InZOrder()) {
    if (!guest->browser) {
      continue;
    }
    guest->browser->GetHost()->SetWindowlessFrameRate(frame_rate);
    ApplyGuestVisibility(*guest);
  }
}

void SabineOsrHandler::NotifyGuestScreenInfo() {
  CEF_REQUIRE_UI_THREAD();
  for (GuestView* guest : guests_.InZOrder()) {
    ApplyGuestBounds(*guest);
  }
}

bool SabineOsrHandler::SendGuestPaint(const GuestView& guest,
                                        const void* buffer,
                                        int width,
                                        int height,
                                        const RectList& dirty_rects) {
  return SendPaintBatch(kGuestFrame, guest.id, guest.bounds.x, guest.bounds.y,
                        buffer, width, height, dirty_rects);
}

void SabineOsrHandler::SendGuestHidden(const GuestView& guest) {
  SendMessage(kGuestHidden, 0, 0, guest.bounds.x, guest.bounds.y,
              guest.id.data(), static_cast<uint32_t>(guest.id.size()));
  if (guest.id == kSabinePopupGuestId) {
    SendMessage(kPopupHidden, 0, 0, 0, 0, nullptr, 0);
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

bool SabineOsrHandler::OnBeforePopup(CefRefPtr<CefBrowser> browser,
                                       CefRefPtr<CefFrame> frame,
                                       SABINE_CEF_POPUP_ID
                                       const CefString& target_url,
                                       const CefString& target_frame_name,
                                       cef_window_open_disposition_t target_disposition,
                                       bool user_gesture,
                                       const CefPopupFeatures& popup_features,
                                       CefWindowInfo& window_info,
                                       CefRefPtr<CefClient>& client,
                                       CefBrowserSettings& settings,
                                       CefRefPtr<CefDictionaryValue>& extra_info,
                                       bool* no_javascript_access) {
  CEF_REQUIRE_UI_THREAD();
  GuestView* guest = GuestForBrowser(browser);
  if (!guest) {
    return true;
  }
  const std::string url = target_url.ToString();
  const int disposition = static_cast<int>(target_disposition);
  switch (guest->popup_policy) {
    case GuestPopupPolicy::kNavigateSame:
      if (!url.empty()) {
        guest->url = url;
        browser->GetMainFrame()->LoadURL(url);
      }
      return true;
    // A CEF-owned popup browser has no surface in the compositor, so an allowed
    // popup becomes another guest view instead.
    case GuestPopupPolicy::kAllow:
    case GuestPopupPolicy::kOpenGuest: {
      if (url.empty()) {
        return true;
      }
      GuestCreateRequest request;
      request.id = NextGuestId();
      request.url = url;
      request.bounds = guest->bounds;
      request.partition = guest->partition;
      request.allow_downloads = guest->allow_downloads;
      request.popup_policy = guest->popup_policy;
      CreateGuest(std::move(request),
                  [](bool success, const std::string& result) {
                    if (!success) {
                      std::cerr << "guest popup failed: " << result << std::endl;
                    }
                  });
      return true;
    }
    case GuestPopupPolicy::kDeny:
    default:
      EmitPrimaryEvent("guest.newWindow",
                       GuestNewWindowJson(guest->id, url, disposition));
      return true;
  }
}

