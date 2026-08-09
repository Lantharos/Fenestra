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
#endif

namespace sabine_osr {

bool PreferSharedTexture(CefRefPtr<CefCommandLine> command_line) {
#if defined(OS_WIN)
  (void)command_line;
  return true;
#else
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

#if defined(OS_WIN)
HANDLE OpenParentForHandleDuplication() {
  CefRefPtr<CefCommandLine> command_line = CefCommandLine::GetGlobalCommandLine();
  const int parent_pid = SwitchInt(command_line, "sabine-parent-pid", 0);
  if (parent_pid <= 0) {
    return nullptr;
  }
  return OpenProcess(PROCESS_DUP_HANDLE, FALSE, static_cast<DWORD>(parent_pid));
}

uint64_t DuplicateHandleToParent(HANDLE shared) {
  if (!shared) {
    return 0;
  }
  HANDLE parent = OpenParentForHandleDuplication();
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

void CloseHandleInParent(uint64_t remote_value) {
  if (remote_value == 0) {
    return;
  }
  HANDLE parent = OpenParentForHandleDuplication();
  if (!parent) {
    return;
  }
  HANDLE local = nullptr;
  DuplicateHandle(parent, reinterpret_cast<HANDLE>(remote_value),
                  GetCurrentProcess(), &local, 0, FALSE,
                  DUPLICATE_SAME_ACCESS | DUPLICATE_CLOSE_SOURCE);
  if (local) {
    CloseHandle(local);
  }
  CloseHandle(parent);
}
#endif

}  // namespace sabine_osr

using namespace sabine_osr;

void SabineOsrHandler::OnAcceleratedPaint(
    CefRefPtr<CefBrowser> browser,
    PaintElementType type,
    const RectList& dirtyRects,
    const CefAcceleratedPaintInfo& info) {
#if !defined(OS_WIN)
  (void)browser;
  (void)type;
  (void)dirtyRects;
  (void)info;
  return;
#else
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
      ReleaseAcceleratedD3d11Frame(copied.slot_token);
      EmitBridgeEvent("\"osr.accel_handle_failed\"", "{}");
      return false;
    }

    AccelPaintMeta meta;
    meta.format = static_cast<uint32_t>(info.format);
    meta.native_handle = remote_handle;
    meta.slot_token = copied.slot_token;

    const std::string payload = BuildAccelPayload(guest_id, meta);
    const bool sent = !payload.empty() &&
                      SendMessage(accel_kind, static_cast<uint32_t>(frame_w),
                                  static_cast<uint32_t>(frame_h), x, y,
                                  payload.data(),
                                  static_cast<uint32_t>(payload.size()));
    if (!sent) {
      CloseHandleInParent(remote_handle);
      ReleaseAcceleratedD3d11Frame(copied.slot_token);
    }
    return sent;
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
#endif
}
