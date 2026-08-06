#include "osr_handler_accel_ipc.h"

#include <algorithm>
#include <cstring>
#include <limits>
#include <vector>

#include "osr_handler_util.h"

namespace sabine_osr {

namespace {

std::string GuestPrefix(const std::string& guest_id) {
  if (guest_id.empty()) {
    return {};
  }
  std::string out(2 + guest_id.size(), '\0');
  const uint16_t len = static_cast<uint16_t>(guest_id.size());
  out[0] = static_cast<char>(len & 0xff);
  out[1] = static_cast<char>((len >> 8) & 0xff);
  std::memcpy(out.data() + 2, guest_id.data(), guest_id.size());
  return out;
}

}  // namespace

std::string BuildAccelPayload(const std::string& guest_id,
                              const AccelPaintMeta& meta,
                              const CefRenderHandler::RectList& dirty_rects,
                              uint32_t width,
                              uint32_t height) {
  std::vector<CefRect> rects;
  if (dirty_rects.empty()) {
    rects.push_back(
        CefRect(0, 0, static_cast<int>(width), static_cast<int>(height)));
  } else {
    rects.assign(dirty_rects.begin(), dirty_rects.end());
  }

  const std::string prefix = GuestPrefix(guest_id);
  const size_t meta_len =
      prefix.size() + 4 + 8 + 4 + 8 + 8 + 8 + 4 + rects.size() * 16;
  if (meta_len > std::numeric_limits<uint32_t>::max()) {
    return {};
  }
  std::vector<char> payload(meta_len, 0);
  size_t at = 0;
  std::memcpy(payload.data() + at, prefix.data(), prefix.size());
  at += prefix.size();
  PutU32(&payload, at, meta.format);
  at += 4;
  PutU64(&payload, at, meta.modifier);
  at += 8;
  PutU32(&payload, at, meta.stride);
  at += 4;
  PutU64(&payload, at, meta.offset);
  at += 8;
  PutU64(&payload, at, meta.size);
  at += 8;
  PutU64(&payload, at, meta.native_handle);
  at += 8;
  PutU32(&payload, at, static_cast<uint32_t>(rects.size()));
  at += 4;
  for (const auto& rect : rects) {
    PutI32(&payload, at, rect.x);
    at += 4;
    PutI32(&payload, at, rect.y);
    at += 4;
    PutU32(&payload, at, static_cast<uint32_t>(std::max(0, rect.width)));
    at += 4;
    PutU32(&payload, at, static_cast<uint32_t>(std::max(0, rect.height)));
    at += 4;
  }
  return std::string(payload.begin(), payload.end());
}

}  // namespace sabine_osr
