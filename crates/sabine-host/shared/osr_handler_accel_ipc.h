#ifndef SABINE_CEF_HOST_OSR_HANDLER_ACCEL_IPC_H_
#define SABINE_CEF_HOST_OSR_HANDLER_ACCEL_IPC_H_

#include <cstdint>
#include <string>

#include "include/cef_render_handler.h"

constexpr uint32_t kMainAccel = 24;
constexpr uint32_t kPopupAccel = 25;
constexpr uint32_t kGuestAccel = 26;

namespace sabine_osr {

struct AccelPaintMeta {
  uint32_t format = 0;
  int32_t visible_x = 0;
  int32_t visible_y = 0;
  uint32_t visible_width = 0;
  uint32_t visible_height = 0;
  /// Duplicated NT HANDLE value in the compositor process.
  uint64_t native_handle = 0;
  /// Identifies the producer slot released after the compositor is done sampling.
  uint64_t slot_token = 0;
};

std::string BuildAccelPayload(const std::string& guest_id,
                              const AccelPaintMeta& meta);

}  // namespace sabine_osr

#endif
