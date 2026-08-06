#include "osr_handler_accelerated.h"

#include <algorithm>
#include <cstring>
#include <vector>

#include "osr_handler.h"
#include "osr_handler_accel_ipc.h"

#ifndef _WIN32
#include <sys/mman.h>
#include <unistd.h>
#endif

namespace sabine_osr {

bool PreferSharedTexture(CefRefPtr<CefCommandLine> command_line) {
  if (!command_line || command_line->HasSwitch("sabine-software-osr")) {
    return false;
  }
#if defined(__linux__)
  return true;
#else
  // Windows/macOS D3D11/IOSurface → compositor import lands later.
  (void)command_line;
  return false;
#endif
}

void ApplySharedTexture(CefWindowInfo* window_info, bool enabled) {
  if (!window_info) {
    return;
  }
  window_info->shared_texture_enabled = enabled ? 1 : 0;
}

bool UnpackAcceleratedPlaneToBgra(const void* src,
                                  size_t src_size,
                                  uint32_t stride,
                                  int width,
                                  int height,
                                  bool src_is_rgba,
                                  std::vector<uint8_t>* out_bgra) {
  if (!src || !out_bgra || width <= 0 || height <= 0 || stride == 0) {
    return false;
  }
  const size_t row_bytes = static_cast<size_t>(width) * 4;
  if (stride < row_bytes) {
    return false;
  }
  const size_t needed =
      static_cast<size_t>(stride) * static_cast<size_t>(height);
  if (src_size < needed) {
    return false;
  }
  out_bgra->assign(static_cast<size_t>(width) * static_cast<size_t>(height) * 4,
                   0);
  const auto* rows = static_cast<const uint8_t*>(src);
  for (int y = 0; y < height; ++y) {
    const uint8_t* src_row = rows + static_cast<size_t>(y) * stride;
    uint8_t* dst_row = out_bgra->data() + static_cast<size_t>(y) * row_bytes;
    if (!src_is_rgba) {
      std::memcpy(dst_row, src_row, row_bytes);
      continue;
    }
    for (int x = 0; x < width; ++x) {
      const uint8_t* px = src_row + static_cast<size_t>(x) * 4;
      uint8_t* out = dst_row + static_cast<size_t>(x) * 4;
      out[0] = px[2];
      out[1] = px[1];
      out[2] = px[0];
      out[3] = px[3];
    }
  }
  return true;
}

}  // namespace sabine_osr

using namespace sabine_osr;

void SabineOsrHandler::OnAcceleratedPaint(
    CefRefPtr<CefBrowser> browser,
    PaintElementType type,
    const RectList& dirtyRects,
    const CefAcceleratedPaintInfo& info) {
#if !defined(__linux__)
  (void)browser;
  (void)type;
  (void)dirtyRects;
  (void)info;
  return;
#else
  if (!browser || info.plane_count <= 0) {
    return;
  }
  const auto& plane = info.planes[0];
  if (plane.fd < 0 || plane.size == 0 || plane.stride == 0) {
    return;
  }

  const bool src_is_rgba = info.format == CEF_COLOR_TYPE_RGBA_8888;
  const bool src_is_bgra = info.format == CEF_COLOR_TYPE_BGRA_8888;
  if (!src_is_rgba && !src_is_bgra) {
    EmitBridgeEvent("\"osr.accel_unsupported\"", "{}");
    return;
  }

  const int width = type == PET_POPUP ? popup_rect_.width : width_;
  const int height = type == PET_POPUP ? popup_rect_.height : height_;
  const int frame_w =
      info.extra.coded_size.width > 0 ? info.extra.coded_size.width : width;
  const int frame_h =
      info.extra.coded_size.height > 0 ? info.extra.coded_size.height : height;
  if (frame_w <= 0 || frame_h <= 0) {
    return;
  }

  AccelPaintMeta meta;
  meta.format = static_cast<uint32_t>(info.format);
  meta.modifier = info.modifier;
  meta.stride = plane.stride;
  meta.offset = plane.offset;
  meta.size = plane.size;

  if (GuestView* guest = GuestForBrowser(browser)) {
    if (type == PET_POPUP) {
      const std::string popup_id = guest->id + "/popup";
      const std::string payload = BuildAccelPayload(
          popup_id, meta, dirtyRects, static_cast<uint32_t>(frame_w),
          static_cast<uint32_t>(frame_h));
      if (!payload.empty() &&
          SendMessageWithFd(
              kGuestAccel, static_cast<uint32_t>(frame_w),
              static_cast<uint32_t>(frame_h),
              guest->bounds.x + guest_popup_rect_.x,
              guest->bounds.y + guest_popup_rect_.y, payload.data(),
              static_cast<uint32_t>(payload.size()), plane.fd)) {
        return;
      }
      // Host-side mmap bridge if FD IPC fails.
      void* mapped =
          mmap(nullptr, plane.size, PROT_READ, MAP_SHARED, plane.fd, 0);
      if (mapped == MAP_FAILED) {
        return;
      }
      std::vector<uint8_t> bgra;
      const bool ok = UnpackAcceleratedPlaneToBgra(
          mapped, plane.size, plane.stride, frame_w, frame_h, src_is_rgba,
          &bgra);
      munmap(mapped, plane.size);
      if (ok) {
        SendPaintBatch(kGuestFrame, popup_id,
                       guest->bounds.x + guest_popup_rect_.x,
                       guest->bounds.y + guest_popup_rect_.y, bgra.data(),
                       frame_w, frame_h, dirtyRects);
      }
      return;
    }
    const std::string payload = BuildAccelPayload(
        guest->id, meta, dirtyRects, static_cast<uint32_t>(frame_w),
        static_cast<uint32_t>(frame_h));
    if (!payload.empty() &&
        SendMessageWithFd(kGuestAccel, static_cast<uint32_t>(frame_w),
                          static_cast<uint32_t>(frame_h), guest->bounds.x,
                          guest->bounds.y, payload.data(),
                          static_cast<uint32_t>(payload.size()), plane.fd)) {
      if (!guest->painted) {
        guest->painted = true;
        if (guest->id == kSabinePopupGuestId) {
          EmitBridgeEvent("\"popup.open\"", "{}");
        }
      }
      return;
    }
  } else if (!view_hidden_ && browser_ && browser_->IsSame(browser)) {
    const uint32_t kind = type == PET_POPUP ? kPopupAccel : kMainAccel;
    const int32_t x = type == PET_POPUP ? popup_rect_.x : 0;
    const int32_t y = type == PET_POPUP ? popup_rect_.y : 0;
    const std::string payload = BuildAccelPayload(
        std::string(), meta, dirtyRects, static_cast<uint32_t>(frame_w),
        static_cast<uint32_t>(frame_h));
    if (!payload.empty() &&
        SendMessageWithFd(kind, static_cast<uint32_t>(frame_w),
                          static_cast<uint32_t>(frame_h), x, y, payload.data(),
                          static_cast<uint32_t>(payload.size()), plane.fd)) {
      return;
    }
  }

  // mmap → existing dirty-rect paint path when FD IPC fails.
  void* mapped = mmap(nullptr, plane.size, PROT_READ, MAP_SHARED, plane.fd, 0);
  if (mapped == MAP_FAILED) {
    EmitBridgeEvent("\"osr.accel_map_failed\"", "{}");
    return;
  }
  std::vector<uint8_t> bgra;
  const bool ok = UnpackAcceleratedPlaneToBgra(
      mapped, plane.size, plane.stride, frame_w, frame_h, src_is_rgba, &bgra);
  munmap(mapped, plane.size);
  if (!ok) {
    EmitBridgeEvent("\"osr.accel_unpack_failed\"", "{}");
    return;
  }

  if (GuestView* guest = GuestForBrowser(browser)) {
    if (type == PET_POPUP) {
      SendPaintBatch(kGuestFrame, guest->id + "/popup",
                     guest->bounds.x + guest_popup_rect_.x,
                     guest->bounds.y + guest_popup_rect_.y, bgra.data(), frame_w,
                     frame_h, dirtyRects);
      return;
    }
    if (!guest->painted) {
      guest->painted = true;
      if (guest->id == kSabinePopupGuestId) {
        EmitBridgeEvent("\"popup.open\"", "{}");
      }
    }
    if (guest->visible) {
      SendGuestPaint(*guest, bgra.data(), frame_w, frame_h, dirtyRects);
    }
    return;
  }
  if (view_hidden_ || !browser_ || !browser_->IsSame(browser)) {
    return;
  }
  const uint32_t kind = type == PET_POPUP ? kPopupFrame : kMainFrame;
  const int32_t x = type == PET_POPUP ? popup_rect_.x : 0;
  const int32_t y = type == PET_POPUP ? popup_rect_.y : 0;
  SendPaintBatch(kind, std::string(), x, y, bgra.data(), frame_w, frame_h,
                  dirtyRects);
#endif
}
