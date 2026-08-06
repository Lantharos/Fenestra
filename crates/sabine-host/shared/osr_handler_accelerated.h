#ifndef SABINE_CEF_HOST_OSR_HANDLER_ACCELERATED_H_
#define SABINE_CEF_HOST_OSR_HANDLER_ACCELERATED_H_

#include <cstddef>
#include <cstdint>
#include <vector>

#include "include/cef_command_line.h"
#include "include/internal/cef_types.h"

namespace sabine_osr {

bool PreferSharedTexture(CefRefPtr<CefCommandLine> command_line);
void ApplySharedTexture(CefWindowInfo* window_info, bool enabled);

// Copy a strided BGRA/RGBA plane into a tightly packed BGRA buffer.
bool UnpackAcceleratedPlaneToBgra(const void* src,
                                  size_t src_size,
                                  uint32_t stride,
                                  int width,
                                  int height,
                                  bool src_is_rgba,
                                  std::vector<uint8_t>* out_bgra);

}  // namespace sabine_osr

#endif
