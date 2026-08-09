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

bool SabineOsrHandler::RunGuestDownloadAction(const std::string& payload,
                                                std::string* error) {
  CEF_REQUIRE_UI_THREAD();
  const std::string download_id = JsonStringValue(payload, "downloadId");
  if (download_id.empty()) {
    *error = "guest.downloadAction requires a `downloadId`";
    return false;
  }
  const auto entry = downloads_.find(download_id);
  if (entry == downloads_.end()) {
    *error = "unknown downloadId";
    return false;
  }
  GuestDownload& download = entry->second;
  std::string action = JsonStringValue(payload, "action");
  if (action.empty()) {
    action = "accept";
  }
  if (action == "accept") {
    if (!download.before_callback) {
      return true;
    }
    const std::string save_path = JsonStringValue(payload, "savePath");
    const bool show_dialog = JsonBoolValue(payload, "showDialog", false);
    CefRefPtr<CefBeforeDownloadCallback> callback = download.before_callback;
    download.before_callback = nullptr;
    callback->Continue(save_path, show_dialog);
    return true;
  }
  if (action == "cancel") {
    if (download.item_callback) {
      download.item_callback->Cancel();
    }
    downloads_.erase(entry);
    return true;
  }
  if (action == "pause") {
    if (download.item_callback) {
      download.item_callback->Pause();
    }
    return true;
  }
  if (action == "resume") {
    if (download.item_callback) {
      download.item_callback->Resume();
    }
    return true;
  }
  *error = "guest.downloadAction `action` must be accept, cancel, pause, or "
           "resume";
  return false;
}

bool SabineOsrHandler::OnBeforeDownload(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefDownloadItem> download_item,
    const CefString& suggested_name,
    CefRefPtr<CefBeforeDownloadCallback> callback) {
  CEF_REQUIRE_UI_THREAD();
  GuestView* guest = GuestForBrowser(browser);
  if (guest && !guest->allow_downloads) {
    return true;
  }
  const std::string download_id =
      std::to_string(download_item ? download_item->GetId() : 0);
  GuestDownload download;
  download.guest_id = guest ? guest->id : std::string();
  download.filename = suggested_name.ToString();
  // Guests wait for `sabine.guest.downloadAction`; the primary browser keeps
  // the default download behaviour it had before guests existed.
  download.before_callback = guest ? callback : nullptr;
  downloads_[download_id] = download;
  EmitPrimaryEvent("guest.download",
                   GuestDownloadJson(download.guest_id, download_id,
                                     download_item, "requested",
                                     download.filename));
  return guest != nullptr;
}

void SabineOsrHandler::OnDownloadUpdated(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefDownloadItem> download_item,
    CefRefPtr<CefDownloadItemCallback> callback) {
  CEF_REQUIRE_UI_THREAD();
  if (!download_item) {
    return;
  }
  const std::string download_id = std::to_string(download_item->GetId());
  const auto entry = downloads_.find(download_id);
  if (entry == downloads_.end()) {
    return;
  }
  entry->second.item_callback = callback;
  std::string state = "progress";
  if (download_item->IsComplete()) {
    state = "completed";
  } else if (download_item->IsCanceled()) {
    state = "canceled";
  } else if (!download_item->IsInProgress()) {
    state = "interrupted";
  }
  EmitPrimaryEvent("guest.download",
                   GuestDownloadJson(entry->second.guest_id, download_id,
                                     download_item, state,
                                     entry->second.filename));
  if (state != "progress") {
    downloads_.erase(entry);
  }
}

