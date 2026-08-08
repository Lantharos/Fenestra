#include "osr_handler_accelerated.h"

#include <algorithm>
#include <cstring>
#include <vector>

#include "osr_handler.h"
#include "osr_handler_accel_ipc.h"
#include "osr_handler_util.h"

#if defined(OS_WIN)
#include "osr_accel_d3d11_win.h"
#include <windows.h>
#elif defined(OS_MAC)
#include <IOSurface/IOSurface.h>
#elif defined(OS_LINUX)
#include <sys/mman.h>
#include <unistd.h>
#endif

namespace sabine_osr {

bool PreferSharedTexture(CefRefPtr<CefCommandLine> command_line) {
  if (!command_line || command_line->HasSwitch("sabine-software-osr")) {
    return false;
  }
#if defined(OS_LINUX)
  // Linux shared-texture OSR (DMA-BUF) still fails SkSurface init on many
  // drivers — especially NVIDIA. Opt in with --sabine-shared-texture.
  return command_line->HasSwitch("sabine-shared-texture");
#else
  // Windows (D3D11) and macOS (IOSurface) use accelerated OSR by default.
  return true;
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

#if defined(OS_WIN)
uint64_t DuplicateHandleToParent(HANDLE shared) {
  if (!shared) {
    return 0;
  }
  CefRefPtr<CefCommandLine> command_line = CefCommandLine::GetGlobalCommandLine();
  const int parent_pid = SwitchInt(command_line, "sabine-parent-pid", 0);
  if (parent_pid <= 0) {
    return 0;
  }
  HANDLE parent =
      OpenProcess(PROCESS_DUP_HANDLE, FALSE, static_cast<DWORD>(parent_pid));
  if (!parent) {
    return 0;
  }
  HANDLE remote = nullptr;
  const BOOL ok =
      DuplicateHandle(GetCurrentProcess(), shared, parent, &remote, 0, FALSE,
                      DUPLICATE_SAME_ACCESS);
  CloseHandle(parent);
  if (!ok || !remote) {
    return 0;
  }
  return reinterpret_cast<uint64_t>(remote);
}
#endif

}  // namespace sabine_osr

using namespace sabine_osr;

void SabineOsrHandler::OnAcceleratedPaint(
    CefRefPtr<CefBrowser> browser,
    PaintElementType type,
    const RectList& dirtyRects,
    const CefAcceleratedPaintInfo& info) {
  if (!browser) {
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

#if defined(OS_LINUX)
  if (info.plane_count <= 0) {
    return;
  }
  const auto& plane = info.planes[0];
  if (plane.fd < 0 || plane.size == 0 || plane.stride == 0) {
    return;
  }

  AccelPaintMeta meta;
  meta.format = static_cast<uint32_t>(info.format);
  meta.modifier = info.modifier;
  meta.stride = plane.stride;
  meta.offset = plane.offset;
  meta.size = plane.size;

  auto send_fd = [&](uint32_t kind, const std::string& guest_id, int32_t x,
                     int32_t y) -> bool {
    const std::string payload = BuildAccelPayload(
        guest_id, meta, dirtyRects, static_cast<uint32_t>(frame_w),
        static_cast<uint32_t>(frame_h));
    return !payload.empty() &&
           SendMessageWithFd(kind, static_cast<uint32_t>(frame_w),
                             static_cast<uint32_t>(frame_h), x, y, payload.data(),
                             static_cast<uint32_t>(payload.size()), plane.fd);
  };

  auto mmap_bgra = [&](std::vector<uint8_t>* out) -> bool {
    void* mapped = mmap(nullptr, plane.size, PROT_READ, MAP_SHARED, plane.fd, 0);
    if (mapped == MAP_FAILED) {
      return false;
    }
    const bool ok = UnpackAcceleratedPlaneToBgra(
        mapped, plane.size, plane.stride, frame_w, frame_h, src_is_rgba, out);
    munmap(mapped, plane.size);
    return ok;
  };

  if (GuestView* guest = GuestForBrowser(browser)) {
    if (type == PET_POPUP) {
      const std::string popup_id = guest->id + "/popup";
      if (send_fd(kGuestAccel, popup_id, guest->bounds.x + guest_popup_rect_.x,
                  guest->bounds.y + guest_popup_rect_.y)) {
        return;
      }
      std::vector<uint8_t> bgra;
      if (mmap_bgra(&bgra)) {
        SendPaintBatch(kGuestFrame, popup_id,
                       guest->bounds.x + guest_popup_rect_.x,
                       guest->bounds.y + guest_popup_rect_.y, bgra.data(),
                       frame_w, frame_h, dirtyRects);
      }
      return;
    }
    if (send_fd(kGuestAccel, guest->id, guest->bounds.x, guest->bounds.y)) {
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
    if (send_fd(kind, std::string(), x, y)) {
      return;
    }
  }

  std::vector<uint8_t> bgra;
  if (!mmap_bgra(&bgra)) {
    EmitBridgeEvent("\"osr.accel_map_failed\"", "{}");
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

#elif defined(OS_WIN)
  auto send_copied_accel = [&](const std::string& slot_key,
                               uint32_t accel_kind,
                               const std::string& guest_id,
                               int32_t x, int32_t y) -> bool {
    AccelD3d11CopiedFrame copied{};
    if (!CopyAcceleratedD3d11Frame(
            slot_key, info.shared_texture_handle, frame_w, frame_h,
            static_cast<uint32_t>(info.format), &copied)) {
      std::vector<uint8_t> bgra;
      if (!ReadAcceleratedD3d11FrameToBgra(
              info.shared_texture_handle, frame_w, frame_h,
              static_cast<uint32_t>(info.format), &bgra)) {
        EmitBridgeEvent("\"osr.accel_copy_failed\"", "{}");
        return false;
      }
      const uint32_t frame_kind = guest_id.empty()
                                      ? (type == PET_POPUP ? kPopupFrame
                                                           : kMainFrame)
                                      : kGuestFrame;
      SendPaintBatch(frame_kind, guest_id, x, y, bgra.data(), frame_w, frame_h,
                     dirtyRects);
      return true;
    }

    const uint64_t remote_handle =
        DuplicateHandleToParent(copied.shared_handle);
    if (remote_handle == 0) {
      EmitBridgeEvent("\"osr.accel_handle_failed\"", "{}");
      return false;
    }

    AccelPaintMeta meta;
    meta.format = static_cast<uint32_t>(info.format);
    meta.native_handle = remote_handle;
    meta.stride = static_cast<uint32_t>(frame_w) * 4;
    meta.size =
        static_cast<uint64_t>(meta.stride) * static_cast<uint64_t>(frame_h);

    const std::string payload = BuildAccelPayload(
        guest_id, meta, dirtyRects, static_cast<uint32_t>(frame_w),
        static_cast<uint32_t>(frame_h));
    return !payload.empty() &&
           SendMessage(accel_kind, static_cast<uint32_t>(frame_w),
                       static_cast<uint32_t>(frame_h), x, y, payload.data(),
                       static_cast<uint32_t>(payload.size()));
  };

  if (GuestView* guest = GuestForBrowser(browser)) {
    if (type == PET_POPUP) {
      send_copied_accel(guest->id + "/popup", kGuestAccel, guest->id + "/popup",
                        guest->bounds.x + guest_popup_rect_.x,
                        guest->bounds.y + guest_popup_rect_.y);
      return;
    }
    if (send_copied_accel(guest->id, kGuestAccel, guest->id, guest->bounds.x,
                          guest->bounds.y) &&
        !guest->painted) {
      guest->painted = true;
      if (guest->id == kSabinePopupGuestId) {
        EmitBridgeEvent("\"popup.open\"", "{}");
      }
    }
    return;
  }
  if (view_hidden_ || !browser_ || !browser_->IsSame(browser)) {
    return;
  }
  const uint32_t kind = type == PET_POPUP ? kPopupAccel : kMainAccel;
  const int32_t x = type == PET_POPUP ? popup_rect_.x : 0;
  const int32_t y = type == PET_POPUP ? popup_rect_.y : 0;
  send_copied_accel(type == PET_POPUP ? "popup" : "main", kind, std::string(),
                    x, y);

#elif defined(OS_MAC)
  auto* surface =
      static_cast<IOSurfaceRef>(info.shared_texture_io_surface);
  if (!surface) {
    return;
  }

  AccelPaintMeta meta;
  meta.format = static_cast<uint32_t>(info.format);
  meta.native_handle = static_cast<uint64_t>(IOSurfaceGetID(surface));
  meta.stride = static_cast<uint32_t>(IOSurfaceGetBytesPerRow(surface));
  meta.size =
      static_cast<uint64_t>(meta.stride) * static_cast<uint64_t>(frame_h);

  auto send_surface = [&](uint32_t kind, const std::string& guest_id, int32_t x,
                          int32_t y) -> bool {
    const std::string payload = BuildAccelPayload(
        guest_id, meta, dirtyRects, static_cast<uint32_t>(frame_w),
        static_cast<uint32_t>(frame_h));
    return !payload.empty() &&
           SendMessage(kind, static_cast<uint32_t>(frame_w),
                       static_cast<uint32_t>(frame_h), x, y, payload.data(),
                       static_cast<uint32_t>(payload.size()));
  };

  if (GuestView* guest = GuestForBrowser(browser)) {
    if (type == PET_POPUP) {
      if (send_surface(kGuestAccel, guest->id + "/popup",
                       guest->bounds.x + guest_popup_rect_.x,
                       guest->bounds.y + guest_popup_rect_.y)) {
        return;
      }
    } else if (send_surface(kGuestAccel, guest->id, guest->bounds.x,
                            guest->bounds.y)) {
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
    if (send_surface(kind, std::string(), x, y)) {
      return;
    }
  }

  // IPC failed — bridge via locked BGRA into the existing paint path.
  if (IOSurfaceLock(surface, kIOSurfaceLockReadOnly, nullptr) !=
      kIOReturnSuccess) {
    EmitBridgeEvent("\"osr.accel_map_failed\"", "{}");
    return;
  }
  const void* base = IOSurfaceGetBaseAddress(surface);
  const size_t stride = IOSurfaceGetBytesPerRow(surface);
  const size_t plane_size = stride * static_cast<size_t>(frame_h);
  std::vector<uint8_t> bgra;
  const bool ok = UnpackAcceleratedPlaneToBgra(
      base, plane_size, static_cast<uint32_t>(stride), frame_w, frame_h,
      src_is_rgba, &bgra);
  IOSurfaceUnlock(surface, kIOSurfaceLockReadOnly, nullptr);
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
#else
  (void)type;
  (void)dirtyRects;
  (void)info;
  (void)src_is_rgba;
  (void)width;
  (void)height;
  (void)frame_w;
  (void)frame_h;
#endif
}
