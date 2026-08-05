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

bool MullionOsrHandler::ConnectSocket() {
#ifdef _WIN32
  WSADATA data{};
  if (WSAStartup(MAKEWORD(2, 2), &data) != 0) {
    return false;
  }
  const size_t separator = endpoint_.rfind(':');
  if (separator == std::string::npos) {
    return false;
  }
  const std::string host = endpoint_.substr(0, separator);
  const std::string port = endpoint_.substr(separator + 1);
  addrinfo hints{};
  hints.ai_family = AF_INET;
  hints.ai_socktype = SOCK_STREAM;
  addrinfo* addresses = nullptr;
  if (getaddrinfo(host.c_str(), port.c_str(), &hints, &addresses) != 0) {
    return false;
  }
  SOCKET connection = INVALID_SOCKET;
  for (addrinfo* address = addresses; address; address = address->ai_next) {
    connection = socket(address->ai_family, address->ai_socktype,
                        address->ai_protocol);
    if (connection != INVALID_SOCKET &&
        connect(connection, address->ai_addr,
                static_cast<int>(address->ai_addrlen)) == 0) {
      break;
    }
    if (connection != INVALID_SOCKET) {
      closesocket(connection);
      connection = INVALID_SOCKET;
    }
  }
  freeaddrinfo(addresses);
  if (connection == INVALID_SOCKET) {
    return false;
  }
  socket_fd_ = static_cast<intptr_t>(connection);
  const std::string authentication = authentication_token_ + "\n";
  if (!SendAll(socket_fd_, authentication.data(), authentication.size())) {
    closesocket(connection);
    socket_fd_ = -1;
    return false;
  }
  return true;
#else
  socket_fd_ = socket(AF_UNIX, SOCK_STREAM, 0);
  if (socket_fd_ < 0) {
    return false;
  }
  sockaddr_un addr{};
  addr.sun_family = AF_UNIX;
  if (endpoint_.size() >= sizeof(addr.sun_path)) {
    return false;
  }
  std::strncpy(addr.sun_path, endpoint_.c_str(), sizeof(addr.sun_path) - 1);
  if (connect(socket_fd_, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
    close(socket_fd_);
    socket_fd_ = -1;
    return false;
  }
  const std::string authentication = authentication_token_ + "\n";
  if (!SendAll(socket_fd_, authentication.data(), authentication.size())) {
    close(socket_fd_);
    socket_fd_ = -1;
    return false;
  }
  return true;
#endif
}

void MullionOsrHandler::StartCommandReader() {
  if (socket_fd_ < 0) {
    return;
  }
  const intptr_t fd = socket_fd_;
  CefRefPtr<MullionOsrHandler> self(this);
  std::thread([self, fd] {
    std::string pending;
    char buffer[2048];
    while (true) {
      const int n = recv(
#ifdef _WIN32
          static_cast<SOCKET>(fd),
#else
          static_cast<int>(fd),
#endif
          buffer, sizeof(buffer), 0);
      if (n <= 0) {
        // Native host exited or crashed — close this browser only so sibling
        // OSR windows sharing the process singleton keep running.
        CefPostTask(TID_UI, new CloseOnDisconnectTask(self));
        break;
      }
      pending.append(buffer, static_cast<size_t>(n));
      size_t newline = 0;
      while ((newline = pending.find('\n')) != std::string::npos) {
        std::string line = pending.substr(0, newline);
        pending.erase(0, newline + 1);
        CefPostTask(TID_UI, new OsrCommandTask(self, line));
      }
    }
  }).detach();
}

bool MullionOsrHandler::SendMessage(uint32_t kind,
                                 uint32_t width,
                                 uint32_t height,
                                 int32_t x,
                                 int32_t y,
                                 const void* payload,
                                 uint32_t payload_len) {
  if (socket_fd_ < 0) {
    return false;
  }
  std::lock_guard<std::mutex> lock(socket_mutex_);
  std::vector<char> header(28, 0);
  header[0] = 'M';
  header[1] = 'L';
  header[2] = 'O';
  header[3] = 'N';
  PutU32(&header, 4, kind);
  PutU32(&header, 8, width);
  PutU32(&header, 12, height);
  PutI32(&header, 16, x);
  PutI32(&header, 20, y);
  PutU32(&header, 24, payload_len);
  return SendAll(socket_fd_, header.data(), header.size()) &&
         (payload_len == 0 ||
          SendAll(socket_fd_, static_cast<const char*>(payload), payload_len));
}

bool MullionOsrHandler::SendMessageWithFd(uint32_t kind,
                                           uint32_t width,
                                           uint32_t height,
                                           int32_t x,
                                           int32_t y,
                                           const void* payload,
                                           uint32_t payload_len,
                                           int fd) {
#ifdef _WIN32
  return false;
#else
  if (socket_fd_ < 0 || fd < 0) {
    return false;
  }
  std::lock_guard<std::mutex> lock(socket_mutex_);
  std::vector<char> header(28, 0);
  header[0] = 'M';
  header[1] = 'L';
  header[2] = 'O';
  header[3] = 'N';
  PutU32(&header, 4, kind);
  PutU32(&header, 8, width);
  PutU32(&header, 12, height);
  PutI32(&header, 16, x);
  PutI32(&header, 20, y);
  PutU32(&header, 24, payload_len);

  iovec iov{};
  iov.iov_base = header.data();
  iov.iov_len = header.size();
  alignas(cmsghdr) char control[CMSG_SPACE(sizeof(int))] = {};
  msghdr message{};
  message.msg_iov = &iov;
  message.msg_iovlen = 1;
  message.msg_control = control;
  message.msg_controllen = sizeof(control);
  cmsghdr* cmsg = CMSG_FIRSTHDR(&message);
  cmsg->cmsg_level = SOL_SOCKET;
  cmsg->cmsg_type = SCM_RIGHTS;
  cmsg->cmsg_len = CMSG_LEN(sizeof(int));
  std::memcpy(CMSG_DATA(cmsg), &fd, sizeof(int));

  const ssize_t sent = sendmsg(socket_fd_, &message, MSG_NOSIGNAL);
  return sent == static_cast<ssize_t>(header.size()) &&
         (payload_len == 0 ||
          SendAll(socket_fd_, static_cast<const char*>(payload), payload_len));
#endif
}

bool MullionOsrHandler::SendPaintBatch(uint32_t kind,
                                        const std::string& guest_id,
                                        int32_t origin_x,
                                        int32_t origin_y,
                                        const void* buffer,
                                        int buffer_width,
                                        int buffer_height,
                                        const RectList& dirty_rects) {
  if (buffer_width <= 0 || buffer_height <= 0 || !buffer) {
    return false;
  }

  std::vector<CefRect> source_rects;
  if (dirty_rects.empty()) {
    source_rects.push_back(CefRect(0, 0, buffer_width, buffer_height));
  } else {
    source_rects.assign(dirty_rects.begin(), dirty_rects.end());
  }

  std::vector<PaintRectBytes> rects;
  uint64_t total_bytes = 0;
  for (const auto& rect : source_rects) {
    const int left = std::max(0, rect.x);
    const int top = std::max(0, rect.y);
    const int right = std::min(buffer_width, rect.x + rect.width);
    const int bottom = std::min(buffer_height, rect.y + rect.height);
    const int width = right - left;
    const int height = bottom - top;
    if (width <= 0 || height <= 0) {
      continue;
    }
    const uint64_t len = static_cast<uint64_t>(width) * height * 4;
    if (len > std::numeric_limits<uint32_t>::max()) {
      return false;
    }
    rects.push_back(PaintRectBytes{
        left,
        top,
        width,
        height,
        total_bytes,
        static_cast<uint32_t>(len),
    });
    total_bytes += len;
  }
  if (rects.empty()) {
    return true;
  }

  const std::string prefix =
      kind == kGuestFrame ? GuestPayloadPrefix(guest_id) : std::string();
  const size_t metadata_len = prefix.size() + 4 + rects.size() * kBatchEntryLen;
  if (metadata_len > std::numeric_limits<uint32_t>::max()) {
    return false;
  }
  std::vector<char> metadata(metadata_len, 0);
  std::memcpy(metadata.data(), prefix.data(), prefix.size());
  PutU32(&metadata, prefix.size(), static_cast<uint32_t>(rects.size()));
  for (size_t i = 0; i < rects.size(); ++i) {
    PutPaintEntry(&metadata, prefix.size() + 4 + i * kBatchEntryLen, rects[i]);
  }

  const bool use_shared = total_bytes >= kSharedPaintThreshold;
#ifndef _WIN32
  if (use_shared) {
    const int fd = CreateMemfd("mullion-osr-paint");
    if (fd >= 0) {
      bool ok = ftruncate(fd, static_cast<off_t>(total_bytes)) == 0;
      for (const auto& rect : rects) {
        ok = ok && WritePaintRect(fd, buffer, buffer_width, rect);
      }
      ok = ok && lseek(fd, 0, SEEK_SET) >= 0;
      if (ok) {
        ok = SendMessageWithFd(SharedBatchKind(kind),
                               static_cast<uint32_t>(buffer_width),
                               static_cast<uint32_t>(buffer_height),
                               origin_x,
                               origin_y,
                               metadata.data(),
                               static_cast<uint32_t>(metadata.size()),
                               fd);
      }
      close(fd);
      if (ok) {
        return true;
      }
    }
  }
#else
  (void)use_shared;
#endif

  if (metadata_len + total_bytes > std::numeric_limits<uint32_t>::max()) {
    return false;
  }
  std::vector<char> payload(metadata_len + static_cast<size_t>(total_bytes), 0);
  std::memcpy(payload.data(), metadata.data(), metadata.size());
  char* data = payload.data() + metadata_len;
  for (const auto& rect : rects) {
    CopyPaintRect(data, buffer, buffer_width, rect);
  }
  return SendMessage(BatchKind(kind),
                     static_cast<uint32_t>(buffer_width),
                     static_cast<uint32_t>(buffer_height),
                     origin_x,
                     origin_y,
                     payload.data(),
                     static_cast<uint32_t>(payload.size()));
}

