#include "osr/handler.h"

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cmath>
#include <cstdint>
#include <sys/types.h>
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

namespace sabine_osr {

SabineOsrHandler* g_instance = nullptr;
std::mutex g_handlers_mutex;
std::vector<SabineOsrHandler*> g_handlers;
const size_t kSharedPaintThreshold = 256 * 1024;
const size_t kBatchEntryLen = 28;
#ifndef MFD_CLOEXEC
constexpr unsigned int MFD_CLOEXEC = 0x0001U;
#endif

void RegisterHandler(SabineOsrHandler* handler) {
  std::lock_guard<std::mutex> lock(g_handlers_mutex);
  g_handlers.push_back(handler);
}

void UnregisterHandler(SabineOsrHandler* handler) {
  std::lock_guard<std::mutex> lock(g_handlers_mutex);
  g_handlers.erase(std::remove(g_handlers.begin(), g_handlers.end(), handler),
                   g_handlers.end());
}

bool HasRegisteredHandlers() {
  std::lock_guard<std::mutex> lock(g_handlers_mutex);
  return !g_handlers.empty();
}

std::vector<SabineOsrHandler*> SnapshotHandlers() {
  std::lock_guard<std::mutex> lock(g_handlers_mutex);
  return g_handlers;
}

int SwitchInt(CefRefPtr<CefCommandLine> command_line,
              const std::string& name,
              int fallback) {
  const std::string value = command_line->GetSwitchValue(name);
  if (value.empty()) {
    return fallback;
  }
  return std::atoi(value.c_str());
}

float SwitchFloat(CefRefPtr<CefCommandLine> command_line,
                  const std::string& name,
                  float fallback) {
  const std::string value = command_line->GetSwitchValue(name);
  if (value.empty()) {
    return fallback;
  }
  return std::atof(value.c_str());
}

std::vector<std::string> Split(const std::string& value, char separator) {
  std::vector<std::string> parts;
  std::stringstream stream(value);
  std::string item;
  while (std::getline(stream, item, separator)) {
    parts.push_back(item);
  }
  return parts;
}

std::vector<std::string> BridgeCommands(CefRefPtr<CefCommandLine> command_line) {
  std::vector<std::string> commands;
  for (const auto& item :
       Split(std::string(command_line->GetSwitchValue("sabine-bridge-commands")), ',')) {
    if (!item.empty()) {
      commands.push_back(item);
    }
  }
  return commands;
}

std::string DecodeUriComponent(const std::string& value) {
  return CefURIDecode(
             value, true,
             static_cast<cef_uri_unescape_rule_t>(
                 UU_SPACES | UU_PATH_SEPARATORS |
                 UU_URL_SPECIAL_CHARS_EXCEPT_PATH_SEPARATORS |
                 UU_REPLACE_PLUS_WITH_SPACE))
      .ToString();
}

std::string QueryValue(const std::string& url, const std::string& name) {
  const size_t query_start = url.find('?');
  if (query_start == std::string::npos) {
    return "";
  }
  const std::string needle = name + "=";
  size_t cursor = query_start + 1;
  while (cursor < url.size()) {
    const size_t next = url.find('&', cursor);
    const size_t end = next == std::string::npos ? url.size() : next;
    const std::string part = url.substr(cursor, end - cursor);
    if (part.rfind(needle, 0) == 0) {
      return DecodeUriComponent(part.substr(needle.size()));
    }
    if (next == std::string::npos) {
      break;
    }
    cursor = next + 1;
  }
  return "";
}

std::string BridgeRequestId(const std::string& url) {
  const std::string prefix = "sabine://bridge/";
  if (url.rfind(prefix, 0) != 0) {
    return "";
  }
  const size_t start = prefix.size();
  const size_t end = url.find_first_of("?#", start);
  return DecodeUriComponent(
      url.substr(start, end == std::string::npos ? std::string::npos : end - start));
}

std::string UrlOrigin(const std::string& url) {
  const size_t scheme_end = url.find("://");
  if (scheme_end == std::string::npos) {
    return "null";
  }
  const std::string scheme = url.substr(0, scheme_end);
  if (scheme == "file" || scheme == "about" || scheme == "devtools") {
    return scheme + "://";
  }
  const size_t authority_start = scheme_end + 3;
  const size_t authority_end = url.find_first_of("/?#", authority_start);
  const std::string authority = url.substr(
      authority_start,
      authority_end == std::string::npos ? std::string::npos
                                         : authority_end - authority_start);
  return authority.empty() ? "null" : scheme + "://" + authority;
}

std::string BridgeInstallScript(const std::set<std::string>& commands) {
  // See the matching comment in handler.cc: the canonical bridge script is
  // embedded as SABINE_BRIDGE_JS_RAW by host.rs at C++ build time.
  std::string prelude =
      "window.__sabineBridgeCommands=" + JsArray(commands) + ";";
  return prelude + SABINE_BRIDGE_JS_RAW;
}

bool ParseBridgeResponse(const std::string& line,
                         std::string* browser_id,
                         std::string* request_id,
                         bool* ok,
                         std::string* payload) {
  const std::string prefix = "SABINE_BRIDGE_RESPONSE\t";
  if (line.rfind(prefix, 0) != 0) {
    return false;
  }
  std::vector<std::string> parts;
  size_t cursor = prefix.size();
  while (parts.size() < 3) {
    const size_t next = line.find('\t', cursor);
    if (next == std::string::npos) {
      return false;
    }
    parts.push_back(line.substr(cursor, next - cursor));
    cursor = next + 1;
  }
  *browser_id = parts[0];
  *request_id = parts[1];
  *ok = parts[2] == "ok";
  *payload = line.substr(cursor);
  return true;
}

bool ParseBridgeEvent(const std::string& line,
                      std::string* name_json,
                      std::string* payload) {
  const std::string prefix = "SABINE_BRIDGE_EVENT\t";
  if (line.rfind(prefix, 0) != 0) {
    return false;
  }
  const size_t separator = line.find('\t', prefix.size());
  if (separator == std::string::npos) {
    return false;
  }
  *name_json = line.substr(prefix.size(), separator - prefix.size());
  *payload = line.substr(separator + 1);
  return true;
}

bool ParseHostControl(const std::string& line,
                      std::string* command,
                      std::string* value) {
  const std::string prefix = "SABINE_HOST_CONTROL\t";
  if (line.rfind(prefix, 0) != 0) {
    return false;
  }
  const size_t separator = line.find('\t', prefix.size());
  if (separator == std::string::npos) {
    *command = line.substr(prefix.size());
    *value = "{}";
    return !command->empty();
  }
  *command = line.substr(prefix.size(), separator - prefix.size());
  *value = line.substr(separator + 1);
  return !command->empty();
}

void PutU32(std::vector<char>* buffer, size_t offset, uint32_t value) {
  (*buffer)[offset + 0] = static_cast<char>(value & 0xff);
  (*buffer)[offset + 1] = static_cast<char>((value >> 8) & 0xff);
  (*buffer)[offset + 2] = static_cast<char>((value >> 16) & 0xff);
  (*buffer)[offset + 3] = static_cast<char>((value >> 24) & 0xff);
}

void PutI32(std::vector<char>* buffer, size_t offset, int32_t value) {
  PutU32(buffer, offset, static_cast<uint32_t>(value));
}

void PutU64(std::vector<char>* buffer, size_t offset, uint64_t value) {
  for (size_t i = 0; i < 8; ++i) {
    (*buffer)[offset + i] = static_cast<char>((value >> (i * 8)) & 0xff);
  }
}

bool SendAll(intptr_t fd, const char* bytes, size_t len) {
  size_t sent = 0;
  while (sent < len) {
    const int result = send(
#ifdef _WIN32
                            static_cast<SOCKET>(fd),
#else
                            static_cast<int>(fd),
#endif
                            bytes + sent,
                            static_cast<int>(len - sent),
#ifdef _WIN32
                            0
#else
                            MSG_NOSIGNAL
#endif
    );
    if (result <= 0) {
      return false;
    }
    sent += static_cast<size_t>(result);
  }
  return true;
}

#ifndef _WIN32
int CreateMemfd(const char* name) {
#ifdef SYS_memfd_create
  return static_cast<int>(syscall(SYS_memfd_create, name, MFD_CLOEXEC));
#else
  errno = ENOSYS;
  return -1;
#endif
}

bool WriteAllAt(int fd, const char* bytes, size_t len, off_t offset) {
  size_t written = 0;
  while (written < len) {
    const ssize_t result = pwrite(fd, bytes + written, len - written,
                                  offset + static_cast<off_t>(written));
    if (result <= 0) {
      return false;
    }
    written += static_cast<size_t>(result);
  }
  return true;
}
#endif

void PutPaintEntry(std::vector<char>* payload,
                   size_t offset,
                   const PaintRectBytes& rect) {
  PutI32(payload, offset + 0, rect.x);
  PutI32(payload, offset + 4, rect.y);
  PutU32(payload, offset + 8, static_cast<uint32_t>(rect.width));
  PutU32(payload, offset + 12, static_cast<uint32_t>(rect.height));
  PutU64(payload, offset + 16, rect.offset);
  PutU32(payload, offset + 24, rect.len);
}

bool CopyPaintRect(char* destination,
                   const void* buffer,
                   int buffer_width,
                   const PaintRectBytes& rect) {
  const char* source = static_cast<const char*>(buffer);
  const int source_stride = buffer_width * 4;
  const int row_bytes = rect.width * 4;
  for (int row = 0; row < rect.height; ++row) {
    std::memcpy(destination + rect.offset + static_cast<size_t>(row * row_bytes),
                source + (rect.y + row) * source_stride + rect.x * 4,
                row_bytes);
  }
  return true;
}

#ifndef _WIN32
bool WritePaintRect(int fd,
                    const void* buffer,
                    int buffer_width,
                    const PaintRectBytes& rect) {
  const char* source = static_cast<const char*>(buffer);
  const int source_stride = buffer_width * 4;
  const int row_bytes = rect.width * 4;
  for (int row = 0; row < rect.height; ++row) {
    if (!WriteAllAt(fd,
                    source + (rect.y + row) * source_stride + rect.x * 4,
                    row_bytes,
                    static_cast<off_t>(rect.offset + static_cast<uint64_t>(row * row_bytes)))) {
      return false;
    }
  }
  return true;
}
#endif

int KeyCodeForName(const std::string& key) {
  if (key.size() == 1) {
    unsigned char c = key[0];
    if (c >= 'a' && c <= 'z') {
      return c - 'a' + 'A';
    }
    return c;
  }
  if (key.rfind("Key", 0) == 0 && key.size() == 4) {
    return key[3];
  }
  if (key == "Enter") return 13;
  if (key == "Backspace") return 8;
  if (key == "Tab") return 9;
  if (key == "Escape") return 27;
  if (key == " " || key == "Space") return 32;
  if (key == "ArrowLeft") return 37;
  if (key == "ArrowUp") return 38;
  if (key == "ArrowRight") return 39;
  if (key == "ArrowDown") return 40;
  if (key == "Delete") return 46;
  if (key == "Home") return 36;
  if (key == "End") return 35;
  if (key == "PageUp") return 33;
  if (key == "PageDown") return 34;
  if (key.size() >= 2 && key[0] == 'F') {
    const std::string number = key.substr(1);
    if (!number.empty() &&
        std::all_of(number.begin(), number.end(), [](unsigned char c) { return std::isdigit(c); })) {
      const int function_key = std::atoi(number.c_str());
      if (function_key >= 1 && function_key <= 24) {
        return 111 + function_key;
      }
    }
  }
  return 0;
}

std::u16string Utf8ToUtf16(const std::string& value) {
  std::u16string output;
  for (size_t i = 0; i < value.size();) {
    uint32_t cp = static_cast<unsigned char>(value[i++]);
    if ((cp & 0x80) == 0) {
    } else if ((cp & 0xe0) == 0xc0 && i < value.size()) {
      const uint32_t b1 = static_cast<unsigned char>(value[i++]);
      cp = ((cp & 0x1f) << 6) | (b1 & 0x3f);
    } else if ((cp & 0xf0) == 0xe0 && i + 1 < value.size()) {
      const uint32_t b1 = static_cast<unsigned char>(value[i++]);
      const uint32_t b2 = static_cast<unsigned char>(value[i++]);
      cp = ((cp & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f);
    } else if ((cp & 0xf8) == 0xf0 && i + 2 < value.size()) {
      const uint32_t b1 = static_cast<unsigned char>(value[i++]);
      const uint32_t b2 = static_cast<unsigned char>(value[i++]);
      const uint32_t b3 = static_cast<unsigned char>(value[i++]);
      cp = ((cp & 0x07) << 18) | ((b1 & 0x3f) << 12) |
           ((b2 & 0x3f) << 6) | (b3 & 0x3f);
    } else {
      continue;
    }
    if (cp <= 0xffff) {
      output.push_back(static_cast<char16_t>(cp));
    } else {
      cp -= 0x10000;
      output.push_back(static_cast<char16_t>(0xd800 + (cp >> 10)));
      output.push_back(static_cast<char16_t>(0xdc00 + (cp & 0x3ff)));
    }
  }
  return output;
}

cef_mouse_button_type_t MouseButtonFromString(const std::string& value) {
  if (value == "right") return MBT_RIGHT;
  if (value == "middle") return MBT_MIDDLE;
  return MBT_LEFT;
}

uint32_t BatchKind(uint32_t frame_kind) {
  if (frame_kind == kGuestFrame) {
    return kGuestBatch;
  }
  return frame_kind == kPopupFrame ? kPopupBatch : kMainBatch;
}

uint32_t SharedBatchKind(uint32_t frame_kind) {
  if (frame_kind == kGuestFrame) {
    return kGuestSharedBatch;
  }
  return frame_kind == kPopupFrame ? kPopupSharedBatch : kMainSharedBatch;
}

std::string CursorName(cef_cursor_type_t type) {
  switch (type) {
    case CT_HAND:
      return "pointer";
    case CT_IBEAM:
      return "text";
    case CT_CROSS:
      return "crosshair";
    case CT_MOVE:
      return "move";
    case CT_WAIT:
      return "wait";
    case CT_HELP:
      return "help";
    case CT_NOTALLOWED:
    case CT_NODROP:
      return "not-allowed";
    case CT_EASTWESTRESIZE:
    case CT_COLUMNRESIZE:
      return "ew-resize";
    case CT_NORTHSOUTHRESIZE:
    case CT_ROWRESIZE:
      return "ns-resize";
    case CT_NORTHEASTRESIZE:
      return "ne-resize";
    case CT_NORTHWESTRESIZE:
      return "nw-resize";
    case CT_SOUTHEASTRESIZE:
      return "se-resize";
    case CT_SOUTHWESTRESIZE:
      return "sw-resize";
    default:
      return "default";
  }
}

OsrCommandTask::OsrCommandTask(CefRefPtr<SabineOsrHandler> handler,
                               std::string line)
    : handler_(handler), line_(std::move(line)) {}

OsrCommandTask::~OsrCommandTask() = default;

void OsrCommandTask::Execute() {
  handler_->HandleControlLine(line_);
}

OsrResizeTask::OsrResizeTask(CefRefPtr<SabineOsrHandler> handler)
    : handler_(std::move(handler)) {}

OsrResizeTask::~OsrResizeTask() = default;

void OsrResizeTask::Execute() {
  handler_->HandlePendingResize();
}

CloseOnDisconnectTask::CloseOnDisconnectTask(
    CefRefPtr<SabineOsrHandler> handler)
    : handler_(std::move(handler)) {}

CloseOnDisconnectTask::~CloseOnDisconnectTask() = default;

void CloseOnDisconnectTask::Execute() {
  handler_->CloseFromNativeDisconnect();
}

}  // namespace sabine_osr
