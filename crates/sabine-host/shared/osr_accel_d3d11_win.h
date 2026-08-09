#ifndef SABINE_CEF_HOST_OSR_ACCEL_D3D11_WIN_H_
#define SABINE_CEF_HOST_OSR_ACCEL_D3D11_WIN_H_

#include <cstdint>
#include <string>
#include <windows.h>

namespace sabine_osr {

struct AccelD3d11CopiedFrame {
  HANDLE shared_handle = nullptr;
  uint64_t slot_token = 0;
  uint32_t width = 0;
  uint32_t height = 0;
};

/// Copy CEF's pooled shared texture into a Sabine-owned shared texture before
/// `OnAcceleratedPaint` returns. CEF recycles the source handle when the
/// callback returns; the compositor must only ever receive handles to our copy.
bool CopyAcceleratedD3d11Frame(const std::string& slot_key,
                               HANDLE cef_shared_handle,
                               int width,
                               int height,
                               uint32_t cef_format,
                               AccelD3d11CopiedFrame* out);

/// Allow a copied texture slot to be reused after the compositor confirms its
/// D3D12 copy has completed.
void ReleaseAcceleratedD3d11Frame(uint64_t slot_token);

}  // namespace sabine_osr

#endif
