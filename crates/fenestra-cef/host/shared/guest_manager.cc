#include "guest_manager.h"

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <sstream>
#include <system_error>

#include "include/cef_parser.h"
#include "json_util.h"

const char kFenestraPopupGuestId[] = "__fenestra_popup";
const char kGuestBridgePrefix[] = "fenestra.guest.";
const char kGuestHostControlPrefix[] = "guest.";

namespace {

const char* const kGuestOperations[] = {
    "create",   "destroy", "navigate",         "setBounds",
    "setVisible", "setCovered", "focus", "reload", "goBack",
    "goForward", "list",   "get",              "setZoom",
    "executeJavaScript",   "downloadAction",
};

std::string SanitizePartition(const std::string& partition) {
  std::string output;
  output.reserve(partition.size());
  for (unsigned char c : partition) {
    if (std::isalnum(c) || c == '-' || c == '_') {
      output += static_cast<char>(c);
    } else {
      char buffer[4];
      std::snprintf(buffer, sizeof(buffer), "%02x", c);
      output += buffer;
    }
  }
  return output;
}

std::string BoolLiteral(bool value) {
  return value ? "true" : "false";
}

std::string CurrentUrl(const GuestView& guest) {
  if (!guest.browser) {
    return guest.url;
  }
  CefRefPtr<CefFrame> frame = guest.browser->GetMainFrame();
  const std::string url = frame ? frame->GetURL().ToString() : std::string();
  return url.empty() ? guest.url : url;
}

}  // namespace

GuestPopupPolicy ParseGuestPopupPolicy(const std::string& value,
                                       GuestPopupPolicy fallback) {
  if (value == "deny" || value == "block") {
    return GuestPopupPolicy::kDeny;
  }
  if (value == "allow") {
    return GuestPopupPolicy::kAllow;
  }
  if (value == "navigateSame" || value == "navigate_same" || value == "same") {
    return GuestPopupPolicy::kNavigateSame;
  }
  if (value == "openGuest" || value == "open_guest" || value == "guest") {
    return GuestPopupPolicy::kOpenGuest;
  }
  return fallback;
}

const char* GuestPopupPolicyName(GuestPopupPolicy policy) {
  switch (policy) {
    case GuestPopupPolicy::kAllow:
      return "allow";
    case GuestPopupPolicy::kNavigateSame:
      return "navigateSame";
    case GuestPopupPolicy::kOpenGuest:
      return "openGuest";
    case GuestPopupPolicy::kDeny:
    default:
      return "deny";
  }
}

const char* WindowOpenDispositionName(int disposition) {
  static const char* const kNames[] = {
      "unknown",        "currentTab",     "singletonTab",
      "newForegroundTab", "newBackgroundTab", "newPopup",
      "newWindow",      "saveToDisk",     "offTheRecord",
      "ignoreAction",   "switchToTab",    "newPictureInPicture",
      "newSplitView",
  };
  const int count = static_cast<int>(sizeof(kNames) / sizeof(kNames[0]));
  if (disposition < 0 || disposition >= count) {
    return "unknown";
  }
  return kNames[disposition];
}

bool IsValidGuestId(const std::string& id) {
  if (id.empty() || id.size() > 128) {
    return false;
  }
  return std::all_of(id.begin(), id.end(), [](unsigned char c) {
    return std::isalnum(c) || c == '-' || c == '_' || c == '.';
  });
}

bool IsGuestBridgeCommand(const std::string& command) {
  const std::string operation = GuestOperationName(command, kGuestBridgePrefix);
  if (operation.empty()) {
    return false;
  }
  for (const char* known : kGuestOperations) {
    if (operation == known) {
      return true;
    }
  }
  return false;
}

std::string GuestOperationName(const std::string& command,
                               const std::string& prefix) {
  if (command.rfind(prefix, 0) != 0) {
    return "";
  }
  return command.substr(prefix.size());
}

std::string DefaultGuestPartition(const std::string& id) {
  return "guest:" + id;
}

std::string GuestCachePath(const std::string& root_cache_path,
                           const std::string& partition) {
  if (root_cache_path.empty() || partition.empty()) {
    return "";
  }
  const std::filesystem::path path = std::filesystem::path(root_cache_path) /
                                     ("guest-" + SanitizePartition(partition));
  std::error_code error;
  std::filesystem::create_directories(path, error);
  if (error) {
    return "";
  }
  return path.string();
}

std::string HtmlDataUri(const std::string& body) {
  return "data:text/html;base64," +
         CefBase64Encode(body.data(), body.size()).ToString();
}

cef_color_t ParseGuestBackgroundColor(const std::string& value,
                                      cef_color_t fallback) {
  std::string digits = value;
  if (!digits.empty() && digits[0] == '#') {
    digits.erase(0, 1);
  }
  if (digits.size() != 6 && digits.size() != 8) {
    return fallback;
  }
  char* end = nullptr;
  const unsigned long parsed = std::strtoul(digits.c_str(), &end, 16);
  if (end != digits.c_str() + digits.size()) {
    return fallback;
  }
  if (digits.size() == 6) {
    return static_cast<cef_color_t>(0xff000000u | parsed);
  }
  return static_cast<cef_color_t>(parsed);
}

double GuestZoomLevel(double factor) {
  const double clamped = std::min(5.0, std::max(0.25, factor));
  return std::log(clamped) / std::log(1.2);
}

std::string GuestPayloadPrefix(const std::string& id) {
  std::string prefix;
  const size_t length = std::min<size_t>(id.size(), 0xffff);
  prefix += static_cast<char>(length & 0xff);
  prefix += static_cast<char>((length >> 8) & 0xff);
  prefix.append(id, 0, length);
  return prefix;
}

CefRect ParseGuestBounds(const std::string& payload, const CefRect& fallback) {
  const std::string nested = JsonObjectValue(payload, "bounds");
  const std::string& source = nested.empty() ? payload : nested;
  CefRect bounds;
  bounds.x = JsonIntValue(source, "x", fallback.x);
  bounds.y = JsonIntValue(source, "y", fallback.y);
  bounds.width = std::max(1, JsonIntValue(source, "width", fallback.width));
  bounds.height = std::max(1, JsonIntValue(source, "height", fallback.height));
  return bounds;
}

bool ParseGuestCreateRequest(const std::string& payload,
                             GuestCreateRequest* request,
                             std::string* error) {
  const std::string id = JsonStringValue(payload, "id");
  if (!id.empty()) {
    if (!IsValidGuestId(id)) {
      *error =
          "guest id may only contain ASCII alphanumerics, '-', '_', and '.'";
      return false;
    }
    request->id = id;
  }
  const std::string url = JsonStringValue(payload, "url");
  const std::string html = JsonStringValue(payload, "html");
  if (url.empty() && html.empty()) {
    *error = "guest.create requires a non-empty `url` or `html`";
    return false;
  }
  request->url = url.empty() ? HtmlDataUri(html) : url;
  request->bounds = ParseGuestBounds(payload, CefRect(0, 0, 1, 1));
  request->partition = JsonStringValue(payload, "partition");
  request->allow_bridge =
      JsonBoolValue(payload, "allowBridge",
                    JsonBoolValue(payload, "allow_bridge", false));
  request->allow_downloads =
      JsonBoolValue(payload, "allowDownloads",
                    JsonBoolValue(payload, "allow_downloads", true));
  request->visible = JsonBoolValue(payload, "visible", true);
  std::string policy = JsonStringValue(payload, "popupPolicy");
  if (policy.empty()) {
    policy = JsonStringValue(payload, "popup_policy");
  }
  request->popup_policy =
      ParseGuestPopupPolicy(policy, GuestPopupPolicy::kDeny);
  request->background_color = JsonStringValue(payload, "backgroundColor");
  if (request->background_color.empty()) {
    request->background_color = JsonStringValue(payload, "background_color");
  }
  return true;
}

std::string GuestInfoJson(const GuestView& guest) {
  const bool loading =
      guest.browser ? guest.browser->IsLoading() : guest.loading;
  const bool can_go_back = guest.browser && guest.browser->CanGoBack();
  const bool can_go_forward = guest.browser && guest.browser->CanGoForward();
  std::ostringstream output;
  output << "{\"id\":\"" << JsonEscape(guest.id) << "\",\"url\":\""
         << JsonEscape(CurrentUrl(guest)) << "\",\"title\":\""
         << JsonEscape(guest.title) << "\",\"bounds\":{\"x\":" << guest.bounds.x
         << ",\"y\":" << guest.bounds.y
         << ",\"width\":" << guest.bounds.width
         << ",\"height\":" << guest.bounds.height << "},\"x\":" << guest.bounds.x
         << ",\"y\":" << guest.bounds.y << ",\"width\":" << guest.bounds.width
         << ",\"height\":" << guest.bounds.height
         << ",\"visible\":" << BoolLiteral(guest.visible)
         << ",\"loading\":" << BoolLiteral(loading)
         << ",\"canGoBack\":" << BoolLiteral(can_go_back)
         << ",\"canGoForward\":" << BoolLiteral(can_go_forward)
         << ",\"partition\":\"" << JsonEscape(guest.partition)
         << "\",\"allowBridge\":" << BoolLiteral(guest.allow_bridge)
         << ",\"popupPolicy\":\"" << GuestPopupPolicyName(guest.popup_policy)
         << "\",\"zoomFactor\":" << guest.zoom << "}";
  return output.str();
}

std::string GuestListJson(const std::vector<const GuestView*>& guests) {
  std::string output = "{\"guests\":[";
  bool first = true;
  for (const GuestView* guest : guests) {
    if (!first) {
      output += ",";
    }
    first = false;
    output += GuestInfoJson(*guest);
  }
  output += "]}";
  return output;
}

std::string GuestIdJson(const std::string& id) {
  return "{\"id\":\"" + JsonEscape(id) + "\"}";
}

std::string GuestNavigatedJson(const GuestView& guest) {
  std::ostringstream output;
  output << "{\"id\":\"" << JsonEscape(guest.id) << "\",\"url\":\""
         << JsonEscape(CurrentUrl(guest)) << "\",\"title\":\""
         << JsonEscape(guest.title) << "\",\"canGoBack\":"
         << BoolLiteral(guest.browser && guest.browser->CanGoBack())
         << ",\"canGoForward\":"
         << BoolLiteral(guest.browser && guest.browser->CanGoForward()) << "}";
  return output.str();
}

std::string GuestNewWindowJson(const std::string& id,
                               const std::string& url,
                               int disposition) {
  std::ostringstream output;
  output << "{\"id\":\"" << JsonEscape(id) << "\",\"url\":\""
         << JsonEscape(url) << "\",\"disposition\":\""
         << WindowOpenDispositionName(disposition) << "\"}";
  return output.str();
}

std::string GuestDownloadJson(const std::string& guest_id,
                              const std::string& download_id,
                              CefRefPtr<CefDownloadItem> item,
                              const std::string& state,
                              const std::string& filename) {
  const bool valid = item && item->IsValid();
  std::ostringstream output;
  output << "{\"guestId\":\"" << JsonEscape(guest_id)
         << "\",\"downloadId\":\"" << JsonEscape(download_id)
         << "\",\"url\":\""
         << JsonEscape(valid ? item->GetURL().ToString() : std::string())
         << "\",\"filename\":\"" << JsonEscape(filename)
         << "\",\"mimeType\":\""
         << JsonEscape(valid ? item->GetMimeType().ToString() : std::string())
         << "\",\"totalBytes\":"
         << (valid ? item->GetTotalBytes() : 0) << ",\"receivedBytes\":"
         << (valid ? item->GetReceivedBytes() : 0) << ",\"state\":\"" << state
         << "\",\"savePath\":\""
         << JsonEscape(valid ? item->GetFullPath().ToString() : std::string())
         << "\"}";
  return output.str();
}

GuestView* GuestRegistry::Find(const std::string& id) {
  if (id.empty()) {
    return nullptr;
  }
  const auto entry = guests_.find(id);
  return entry == guests_.end() ? nullptr : &entry->second;
}

const GuestView* GuestRegistry::Find(const std::string& id) const {
  if (id.empty()) {
    return nullptr;
  }
  const auto entry = guests_.find(id);
  return entry == guests_.end() ? nullptr : &entry->second;
}

GuestView* GuestRegistry::FindByBrowser(const CefRefPtr<CefBrowser>& browser) {
  if (!browser) {
    return nullptr;
  }
  for (auto& entry : guests_) {
    if (entry.second.browser && entry.second.browser->IsSame(browser)) {
      return &entry.second;
    }
  }
  return nullptr;
}

GuestView* GuestRegistry::Insert(GuestView guest) {
  const std::string id = guest.id;
  guests_[id] = std::move(guest);
  if (std::find(z_order_.begin(), z_order_.end(), id) == z_order_.end()) {
    z_order_.push_back(id);
  }
  return &guests_[id];
}

void GuestRegistry::Erase(const std::string& id) {
  guests_.erase(id);
  z_order_.erase(std::remove(z_order_.begin(), z_order_.end(), id),
                 z_order_.end());
}

void GuestRegistry::Raise(const std::string& id) {
  if (guests_.find(id) == guests_.end()) {
    return;
  }
  z_order_.erase(std::remove(z_order_.begin(), z_order_.end(), id),
                 z_order_.end());
  z_order_.push_back(id);
}

GuestView* GuestRegistry::TopmostAt(int x, int y) {
  if (covered_) {
    return nullptr;
  }
  for (auto id = z_order_.rbegin(); id != z_order_.rend(); ++id) {
    GuestView* guest = Find(*id);
    if (!guest || !guest->visible) {
      continue;
    }
    const CefRect& bounds = guest->bounds;
    if (x >= bounds.x && y >= bounds.y && x < bounds.x + bounds.width &&
        y < bounds.y + bounds.height) {
      return guest;
    }
  }
  return nullptr;
}

std::vector<GuestView*> GuestRegistry::InZOrder() {
  std::vector<GuestView*> ordered;
  ordered.reserve(z_order_.size());
  for (const auto& id : z_order_) {
    if (GuestView* guest = Find(id)) {
      ordered.push_back(guest);
    }
  }
  return ordered;
}

std::vector<const GuestView*> GuestRegistry::InZOrder() const {
  std::vector<const GuestView*> ordered;
  ordered.reserve(z_order_.size());
  for (const auto& id : z_order_) {
    if (const GuestView* guest = Find(id)) {
      ordered.push_back(guest);
    }
  }
  return ordered;
}
