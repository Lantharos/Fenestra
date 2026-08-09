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

namespace {

std::string FileUriToPath(const std::string& value) {
  const std::string prefix = "file://";
  if (value.rfind(prefix, 0) != 0) {
    return value;
  }
  std::string path = value.substr(prefix.size());
  const std::string host_prefix = "localhost/";
  if (path.rfind(host_prefix, 0) == 0) {
    path = path.substr(host_prefix.size());
  }
  std::string decoded;
  decoded.reserve(path.size());
  for (size_t i = 0; i < path.size(); ++i) {
    if (path[i] == '%' && i + 2 < path.size()) {
      char hex[3] = {path[i + 1], path[i + 2], 0};
      char* end = nullptr;
      const long byte = std::strtol(hex, &end, 16);
      if (end == hex + 2) {
        decoded.push_back(static_cast<char>(byte));
        i += 2;
        continue;
      }
    }
    decoded.push_back(path[i] == '?' || path[i] == '#' ? '\0' : path[i]);
  }
  return decoded;
}

std::string BuildFileDragPayload(const std::vector<std::string>& paths) {
  std::string output = "{\"paths\":[";
  bool first = true;
  for (const auto& path : paths) {
    if (!first) {
      output += ",";
    }
    first = false;
    output += '"';
    output += JsonEscape(path);
    output += '"';
  }
  output += "]}";
  return output;
}

cef_drag_operations_mask_t DragOperationFromName(const std::string& operation) {
  if (operation == "copy") {
    return DRAG_OPERATION_COPY;
  }
  if (operation == "move") {
    return DRAG_OPERATION_MOVE;
  }
  if (operation == "link") {
    return DRAG_OPERATION_LINK;
  }
  return DRAG_OPERATION_NONE;
}

}  // namespace

bool SabineOsrHandler::StartDragging(CefRefPtr<CefBrowser> browser,
                                   CefRefPtr<CefDragData> drag_data,
                                   cef_drag_operations_mask_t allowed_ops,
                                   int x,
                                   int y) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser || !drag_data || socket_fd_ < 0) {
    return false;
  }

  std::vector<std::string> paths;

  if (drag_data->IsFile()) {
    std::vector<CefString> file_paths;
    if (drag_data->GetFilePaths(file_paths) && !file_paths.empty()) {
      for (const auto& file_path : file_paths) {
        paths.push_back(FileUriToPath(file_path.ToString()));
      }
    }
    if (paths.empty()) {
      const std::string file_name = drag_data->GetFileName().ToString();
      if (!file_name.empty()) {
        paths.push_back(FileUriToPath(file_name));
      }
    }
  }

  if (paths.empty()) {
    const std::string fragment_text = drag_data->GetFragmentText().ToString();
    const std::string link_url = drag_data->GetLinkURL().ToString();
    std::stringstream stream;
    if (!fragment_text.empty()) {
      stream << fragment_text;
    } else if (!link_url.empty()) {
      stream << link_url;
    }
    std::string line;
    while (std::getline(stream, line)) {
      std::string trimmed = line;
      while (!trimmed.empty() && (trimmed.back() == '\r' || trimmed.back() == '\n')) {
        trimmed.pop_back();
      }
      if (trimmed.empty()) continue;
      paths.push_back(FileUriToPath(trimmed));
    }
  }

  if (paths.empty()) {
    return false;
  }

  const std::string payload = BuildFileDragPayload(paths);
  if (!SendMessage(kFileDragRequested, 0, 0, x, y, payload.data(),
                   static_cast<uint32_t>(payload.size()))) {
    return false;
  }

  // The Rust host owns the system drag via winit. Keep CEF's drag source
  // alive until the host reports completion through file_drag_ended.
  drag_source_browser_ = browser;
  (void)allowed_ops;
  return true;
}

void SabineOsrHandler::UpdateDragCursor(CefRefPtr<CefBrowser> browser,
                                      DragOperation operation) {
  // No-op: cursor changes are driven by the host's window manager.
  (void)browser;
  (void)operation;
}

void SabineOsrHandler::FinishNativeFileDrag(int x,
                                             int y,
                                             const std::string& operation) {
  CEF_REQUIRE_UI_THREAD();
  CefRefPtr<CefBrowser> browser = drag_source_browser_;
  drag_source_browser_ = nullptr;
  if (!browser) {
    browser = browser_;
  }
  if (!browser) {
    return;
  }
  CefRefPtr<CefBrowserHost> host = browser->GetHost();
  if (!host) {
    return;
  }
  host->DragSourceEndedAt(x, y, DragOperationFromName(operation));
  host->DragSourceSystemDragEnded();
}
