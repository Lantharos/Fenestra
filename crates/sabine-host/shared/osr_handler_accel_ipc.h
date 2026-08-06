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
  uint64_t modifier = 0;
  uint32_t stride = 0;
  uint64_t offset = 0;
  uint64_t size = 0;
  /// Windows: duplicated HANDLE value in the compositor process.
  /// macOS: IOSurfaceID. Linux: unused (0); plane travels via SCM_RIGHTS.
  uint64_t native_handle = 0;
};

std::string BuildAccelPayload(const std::string& guest_id,
                              const AccelPaintMeta& meta,
                              const CefRenderHandler::RectList& dirty_rects,
                              uint32_t width,
                              uint32_t height);

}  // namespace sabine_osr

#endif
