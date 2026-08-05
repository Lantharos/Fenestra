#ifndef MULLION_CEF_HOST_GUEST_MANAGER_H_
#define MULLION_CEF_HOST_GUEST_MANAGER_H_

#include <cstdint>
#include <map>
#include <string>
#include <vector>

#include "include/cef_browser.h"
#include "include/cef_download_handler.h"
#include "include/cef_download_item.h"
#include "include/cef_version.h"

#if defined(CEF_VERSION_MAJOR)
#define MULLION_CEF_MAJOR CEF_VERSION_MAJOR
#elif defined(CHROME_VERSION_MAJOR)
#define MULLION_CEF_MAJOR CHROME_VERSION_MAJOR
#else
#define MULLION_CEF_MAJOR 0
#endif

// CEF 133 added the |popup_id| parameter to CefLifeSpanHandler::OnBeforePopup.
#if MULLION_CEF_MAJOR >= 133
#define MULLION_CEF_POPUP_ID int popup_id,
#else
#define MULLION_CEF_POPUP_ID
#endif

// Guest id reserved for the legacy `mullion.popup` surface.
extern const char kMullionPopupGuestId[];

extern const char kGuestBridgePrefix[];
extern const char kGuestHostControlPrefix[];

enum class GuestPopupPolicy {
  kDeny = 0,
  kAllow = 1,
  kNavigateSame = 2,
  kOpenGuest = 3,
};

GuestPopupPolicy ParseGuestPopupPolicy(const std::string& value,
                                       GuestPopupPolicy fallback);
const char* GuestPopupPolicyName(GuestPopupPolicy policy);
const char* WindowOpenDispositionName(int disposition);

struct GuestView {
  std::string id;
  CefRefPtr<CefBrowser> browser;
  CefRect bounds;
  std::string url;
  std::string title;
  std::string partition;
  bool visible = true;
  bool allow_bridge = false;
  bool allow_downloads = true;
  std::vector<std::string> intercepted_shortcuts;
  bool intercept_horizontal_wheel = false;
  bool pending = false;
  bool painted = false;
  bool loading = false;
  GuestPopupPolicy popup_policy = GuestPopupPolicy::kDeny;
  double zoom = 1.0;
};

struct GuestCreateRequest {
  std::string id;
  std::string url;
  CefRect bounds = CefRect(0, 0, 1, 1);
  std::string partition;
  bool allow_bridge = false;
  bool allow_downloads = true;
  std::vector<std::string> intercepted_shortcuts;
  bool intercept_horizontal_wheel = false;
  bool visible = true;
  GuestPopupPolicy popup_policy = GuestPopupPolicy::kDeny;
  std::string background_color;
};

struct GuestDownload {
  std::string guest_id;
  std::string filename;
  CefRefPtr<CefBeforeDownloadCallback> before_callback;
  CefRefPtr<CefDownloadItemCallback> item_callback;
};

/// Resolves the `url` / `html` pair of a create payload into a loadable URL and
/// validates the optional guest id. Returns false and fills |error| when the
/// payload has neither document nor a usable id.
bool ParseGuestCreateRequest(const std::string& payload,
                             GuestCreateRequest* request,
                             std::string* error);
CefRect ParseGuestBounds(const std::string& payload, const CefRect& fallback);

bool IsValidGuestId(const std::string& id);
bool IsGuestBridgeCommand(const std::string& command);
std::string GuestOperationName(const std::string& command,
                               const std::string& prefix);
std::string DefaultGuestPartition(const std::string& id);
std::string GuestCachePath(const std::string& root_cache_path,
                           const std::string& partition);
std::string HtmlDataUri(const std::string& body);
cef_color_t ParseGuestBackgroundColor(const std::string& value,
                                      cef_color_t fallback);
double GuestZoomLevel(double factor);

/// Length-prefixed guest id that precedes every guest paint payload.
std::string GuestPayloadPrefix(const std::string& id);

std::string GuestInfoJson(const GuestView& guest);
std::string GuestListJson(const std::vector<const GuestView*>& guests);
std::string GuestIdJson(const std::string& id);
std::string GuestNavigatedJson(const GuestView& guest);
std::string GuestNewWindowJson(const std::string& id,
                               const std::string& url,
                               int disposition);
std::string GuestDownloadJson(const std::string& guest_id,
                              const std::string& download_id,
                              CefRefPtr<CefDownloadItem> item,
                              const std::string& state,
                              const std::string& filename);
/// Guest views keyed by id, with a separate bottom-to-top z-order used for
/// pointer hit testing and paint ordering.
class GuestRegistry {
 public:
  GuestView* Find(const std::string& id);
  const GuestView* Find(const std::string& id) const;
  GuestView* FindByBrowser(const CefRefPtr<CefBrowser>& browser);
  GuestView* Insert(GuestView guest);
  void Erase(const std::string& id);
  void Raise(const std::string& id);
  GuestView* TopmostAt(int x, int y);
  std::vector<GuestView*> InZOrder();
  std::vector<const GuestView*> InZOrder() const;
  bool Empty() const { return guests_.empty(); }
  void SetCovered(bool covered) { covered_ = covered; }
  bool Covered() const { return covered_; }

 private:
  std::map<std::string, GuestView> guests_;
  std::vector<std::string> z_order_;
  bool covered_ = false;
};

#endif
