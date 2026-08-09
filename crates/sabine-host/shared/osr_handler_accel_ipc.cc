#include "osr_handler_accel_ipc.h"

#include <cstring>
#include <limits>

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
                              const AccelPaintMeta& meta) {
  if (guest_id.size() > std::numeric_limits<uint16_t>::max()) {
    return {};
  }
  const std::string prefix = GuestPrefix(guest_id);
  const size_t meta_len = prefix.size() + 4 + 8 + 8;
  if (meta_len > std::numeric_limits<uint32_t>::max()) {
    return {};
  }
  std::vector<char> payload(meta_len, 0);
  size_t at = 0;
  std::memcpy(payload.data() + at, prefix.data(), prefix.size());
  at += prefix.size();
  PutU32(&payload, at, meta.format);
  at += 4;
  PutU64(&payload, at, meta.native_handle);
  at += 8;
  PutU64(&payload, at, meta.slot_token);
  return std::string(payload.begin(), payload.end());
}

}  // namespace sabine_osr
