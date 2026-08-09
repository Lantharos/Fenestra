#include "osr_accel_d3d11_win.h"

#include <d3d11.h>
#include <d3d11_1.h>
#include <d3d11_4.h>
#include <d3d12.h>
#include <dxgi.h>
#include <dxgi1_2.h>

#include <cstdio>
#include <map>
#include <utility>

#include "include/internal/cef_types.h"

namespace sabine_osr {
namespace {

// Windows GPU interop containment boundary.
//
// CEF owns an accelerated-paint texture only until its callback returns. This
// module opens that texture on the same DXGI adapter, copies it into a Sabine-
// owned D3D12 shareable resource through D3D11, waits for an ordered D3D11
// fence, and retains the copied slot until the compositor acknowledges it.
// Seemingly redundant COM interfaces and handle transitions enforce those
// ownership and ordering rules. Do not reorder or simplify them without
// testing Windows 10/11 on integrated, discrete, and hybrid-GPU systems.

constexpr uint32_t kCefColorTypeBgra8888 = 1;
constexpr DWORD kGpuFenceTimeoutMs = 1000;

struct D3d11Context {
  ID3D11Device* device = nullptr;
  ID3D11Device1* device1 = nullptr;
  ID3D11Device5* device5 = nullptr;
  ID3D11DeviceContext* context = nullptr;
  ID3D11DeviceContext4* context4 = nullptr;
  ID3D11Fence* fence = nullptr;
  HANDLE fence_event = nullptr;
  uint64_t fence_value = 0;
  ID3D12Device* device12 = nullptr;
};

struct OwnedSharedSlot {
  ID3D11Texture2D* texture = nullptr;
  ID3D12Resource* resource12 = nullptr;
  HANDLE shared_handle = nullptr;
  int width = 0;
  int height = 0;
  DXGI_FORMAT format = DXGI_FORMAT_UNKNOWN;
  bool in_use = false;
  uint64_t token = 0;

  void Reset() {
    if (texture) {
      texture->Release();
      texture = nullptr;
    }
    if (resource12) {
      resource12->Release();
      resource12 = nullptr;
    }
    if (shared_handle) {
      CloseHandle(shared_handle);
      shared_handle = nullptr;
    }
    width = 0;
    height = 0;
    format = DXGI_FORMAT_UNKNOWN;
    in_use = false;
    token = 0;
  }
};

D3d11Context g_d3d11;
std::map<std::string, OwnedSharedSlot> g_slots;
std::map<std::string, uint32_t> g_next_slot;
uint64_t g_next_token = 1;
constexpr uint32_t kSlotsPerSurface = 4;

bool EnsureDevice() {
  if (g_d3d11.device && g_d3d11.device1 && g_d3d11.device5 &&
      g_d3d11.context && g_d3d11.context4 && g_d3d11.fence &&
      g_d3d11.fence_event && g_d3d11.device12) {
    return true;
  }
  D3D_FEATURE_LEVEL feature_levels[] = {
      D3D_FEATURE_LEVEL_11_1,
      D3D_FEATURE_LEVEL_11_0,
  };
  D3D_FEATURE_LEVEL chosen = D3D_FEATURE_LEVEL_11_0;
  ID3D11Device* device = nullptr;
  ID3D11DeviceContext* context = nullptr;
  HRESULT hr = D3D11CreateDevice(
      nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, feature_levels,
      static_cast<UINT>(sizeof(feature_levels) / sizeof(feature_levels[0])),
      D3D11_SDK_VERSION, &device, &chosen, &context);
  if (hr == E_INVALIDARG) {
    hr = D3D11CreateDevice(
        nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, &feature_levels[1], 1,
        D3D11_SDK_VERSION, &device, &chosen, &context);
  }
  if (FAILED(hr) || !device || !context) {
    return false;
  }

  ID3D11Device1* device1 = nullptr;
  hr = device->QueryInterface(__uuidof(ID3D11Device1),
                              reinterpret_cast<void**>(&device1));
  if (FAILED(hr) || !device1) {
    device->Release();
    context->Release();
    return false;
  }

  ID3D11Device5* device5 = nullptr;
  hr = device->QueryInterface(__uuidof(ID3D11Device5),
                              reinterpret_cast<void**>(&device5));
  if (FAILED(hr) || !device5) {
    std::fprintf(stderr,
                 "Sabine CEF: D3D11 fence support is unavailable (hr=0x%08lx)\n",
                 static_cast<unsigned long>(hr));
    device1->Release();
    device->Release();
    context->Release();
    return false;
  }

  ID3D11DeviceContext4* context4 = nullptr;
  hr = context->QueryInterface(__uuidof(ID3D11DeviceContext4),
                               reinterpret_cast<void**>(&context4));
  if (FAILED(hr) || !context4) {
    device5->Release();
    device1->Release();
    device->Release();
    context->Release();
    return false;
  }

  IDXGIDevice* dxgi_device = nullptr;
  IDXGIAdapter* adapter = nullptr;
  ID3D12Device* device12 = nullptr;
  hr = device->QueryInterface(__uuidof(IDXGIDevice),
                              reinterpret_cast<void**>(&dxgi_device));
  if (SUCCEEDED(hr) && dxgi_device) {
    hr = dxgi_device->GetAdapter(&adapter);
    dxgi_device->Release();
  }
  if (SUCCEEDED(hr) && adapter) {
    hr = D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0,
                           __uuidof(ID3D12Device),
                           reinterpret_cast<void**>(&device12));
    adapter->Release();
  }
  if (FAILED(hr) || !device12) {
    context4->Release();
    device5->Release();
    device1->Release();
    device->Release();
    context->Release();
    return false;
  }

  ID3D11Fence* fence = nullptr;
  hr = device5->CreateFence(0, D3D11_FENCE_FLAG_NONE, __uuidof(ID3D11Fence),
                            reinterpret_cast<void**>(&fence));
  if (FAILED(hr) || !fence) {
    device12->Release();
    context4->Release();
    device5->Release();
    device1->Release();
    device->Release();
    context->Release();
    return false;
  }

  HANDLE fence_event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
  if (!fence_event) {
    fence->Release();
    device12->Release();
    context4->Release();
    device5->Release();
    device1->Release();
    device->Release();
    context->Release();
    return false;
  }

  g_d3d11.device = device;
  g_d3d11.device1 = device1;
  g_d3d11.device5 = device5;
  g_d3d11.context = context;
  g_d3d11.context4 = context4;
  g_d3d11.fence = fence;
  g_d3d11.fence_event = fence_event;
  g_d3d11.device12 = device12;
  return true;
}

bool WaitForGpu() {
  if (!g_d3d11.context || !g_d3d11.context4 || !g_d3d11.fence ||
      !g_d3d11.fence_event) {
    return false;
  }
  const uint64_t fence_value = ++g_d3d11.fence_value;
  HRESULT hr = g_d3d11.context4->Signal(g_d3d11.fence, fence_value);
  if (FAILED(hr)) {
    return false;
  }
  hr = g_d3d11.fence->SetEventOnCompletion(fence_value,
                                           g_d3d11.fence_event);
  if (FAILED(hr)) {
    return false;
  }
  g_d3d11.context->Flush();
  return WaitForSingleObject(g_d3d11.fence_event, kGpuFenceTimeoutMs) ==
         WAIT_OBJECT_0;
}

bool EnsureOwnedSharedSlot(OwnedSharedSlot* slot,
                           int width,
                           int height,
                           DXGI_FORMAT format) {
  if (!slot || width <= 0 || height <= 0 || format == DXGI_FORMAT_UNKNOWN) {
    return false;
  }
  if (slot->texture && slot->shared_handle && slot->width == width &&
      slot->height == height && slot->format == format) {
    return true;
  }
  slot->Reset();

  D3D12_HEAP_PROPERTIES heap{};
  heap.Type = D3D12_HEAP_TYPE_DEFAULT;
  heap.CreationNodeMask = 1;
  heap.VisibleNodeMask = 1;
  D3D12_RESOURCE_DESC desc{};
  desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
  desc.Width = static_cast<UINT64>(width);
  desc.Height = static_cast<UINT>(height);
  desc.DepthOrArraySize = 1;
  desc.MipLevels = 1;
  desc.Format = format == DXGI_FORMAT_B8G8R8A8_UNORM
                    ? DXGI_FORMAT_B8G8R8A8_TYPELESS
                    : format;
  desc.SampleDesc.Count = 1;
  desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
  desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET |
               D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS;

  ID3D12Resource* resource12 = nullptr;
  HRESULT hr = g_d3d11.device12->CreateCommittedResource(
      &heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON,
      nullptr, __uuidof(ID3D12Resource),
      reinterpret_cast<void**>(&resource12));
  if (FAILED(hr) || !resource12) {
    std::fprintf(stderr,
                 "Sabine CEF: failed to create owned D3D12 shared texture (hr=0x%08lx)\n",
                 static_cast<unsigned long>(hr));
    return false;
  }

  HANDLE shared_handle = nullptr;
  hr = g_d3d11.device12->CreateSharedHandle(
      resource12, nullptr, GENERIC_ALL, nullptr, &shared_handle);
  if (FAILED(hr) || !shared_handle) {
    std::fprintf(stderr,
                 "Sabine CEF: failed to export owned D3D11 shared handle (hr=0x%08lx)\n",
                 static_cast<unsigned long>(hr));
    resource12->Release();
    return false;
  }

  ID3D11Texture2D* texture = nullptr;
  // Windows quirk: the D3D12 resource is the exported owner, while this D3D11
  // view exists solely so CopyResource can consume CEF's D3D11 texture.
  hr = g_d3d11.device1->OpenSharedResource1(
      shared_handle, __uuidof(ID3D11Texture2D),
      reinterpret_cast<void**>(&texture));
  if (FAILED(hr) || !texture) {
    std::fprintf(stderr,
                 "Sabine CEF: failed to open owned D3D12 texture in D3D11 (hr=0x%08lx)\n",
                 static_cast<unsigned long>(hr));
    CloseHandle(shared_handle);
    resource12->Release();
    return false;
  }

  slot->texture = texture;
  slot->resource12 = resource12;
  slot->shared_handle = shared_handle;
  slot->width = width;
  slot->height = height;
  slot->format = format;
  return true;
}

ID3D11Texture2D* OpenCefSharedTexture(HANDLE cef_shared_handle) {
  if (!cef_shared_handle || !g_d3d11.device1) {
    return nullptr;
  }
  ID3D11Texture2D* texture = nullptr;
  // The returned COM reference must not outlive this paint callback. Sabine
  // exports its own copy instead of retaining CEF's pooled texture or handle.
  const HRESULT hr = g_d3d11.device1->OpenSharedResource1(
      cef_shared_handle, __uuidof(ID3D11Texture2D),
      reinterpret_cast<void**>(&texture));
  if (FAILED(hr)) {
    std::fprintf(stderr,
                 "Sabine CEF: OpenSharedResource1 failed (hr=0x%08lx)\n",
                 static_cast<unsigned long>(hr));
    return nullptr;
  }
  return texture;
}

bool CopyOpenedTexture(ID3D11Texture2D* source,
                       const std::string& slot_key,
                       AccelD3d11CopiedFrame* out) {
  if (!source || !out) {
    return false;
  }

  D3D11_TEXTURE2D_DESC source_desc{};
  source->GetDesc(&source_desc);
  if (source_desc.Width == 0 || source_desc.Height == 0) {
    return false;
  }

  OwnedSharedSlot& slot = g_slots[slot_key];
  if (slot.in_use) {
    return false;
  }
  if (!EnsureOwnedSharedSlot(&slot, static_cast<int>(source_desc.Width),
                             static_cast<int>(source_desc.Height),
                             source_desc.Format)) {
    return false;
  }

  g_d3d11.context->CopyResource(slot.texture, source);
  if (!WaitForGpu()) {
    return false;
  }

  out->shared_handle = slot.shared_handle;
  slot.in_use = true;
  slot.token = g_next_token++;
  if (slot.token == 0) {
    slot.token = g_next_token++;
  }
  out->slot_token = slot.token;
  out->width = source_desc.Width;
  out->height = source_desc.Height;
  return out->shared_handle != nullptr && out->slot_token != 0;
}

}  // namespace

bool CopyAcceleratedD3d11Frame(const std::string& slot_key,
                               HANDLE cef_shared_handle,
                               int width,
                               int height,
                               uint32_t cef_format,
                               AccelD3d11CopiedFrame* out) {
  if (!cef_shared_handle || width <= 0 || height <= 0 || !out ||
      cef_format != kCefColorTypeBgra8888) {
    return false;
  }
  if (!EnsureDevice()) {
    return false;
  }

  ID3D11Texture2D* source = OpenCefSharedTexture(cef_shared_handle);
  if (!source) {
    return false;
  }
  const uint32_t first_slot = g_next_slot[slot_key]++ % kSlotsPerSurface;
  bool copied = false;
  for (uint32_t offset = 0; offset < kSlotsPerSurface; ++offset) {
    const uint32_t slot_index = (first_slot + offset) % kSlotsPerSurface;
    if (CopyOpenedTexture(source,
                          slot_key + "#" + std::to_string(slot_index), out)) {
      copied = true;
      break;
    }
  }
  source->Release();
  return copied;
}

void ReleaseAcceleratedD3d11Frame(uint64_t slot_token) {
  if (slot_token == 0) {
    return;
  }
  for (auto& [key, slot] : g_slots) {
    (void)key;
    if (slot.in_use && slot.token == slot_token) {
      slot.in_use = false;
      slot.token = 0;
      return;
    }
  }
}

}  // namespace sabine_osr
