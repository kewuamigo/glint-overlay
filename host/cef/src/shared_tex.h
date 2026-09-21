#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <d3d11_1.h>
#include <dxgi.h>

#include <cstdint>

struct SharedDirtyRect {
  int x = 0;
  int y = 0;
  int w = 0;
  int h = 0;
};

// One persistent NT-shared mailbox per layer (Steam cached hTexture).
// CEF paints patch dirty rects onto it; the game always samples this handle.
class SharedTexPublisher {
 public:
  SharedTexPublisher() = default;
  ~SharedTexPublisher();

  SharedTexPublisher(const SharedTexPublisher&) = delete;
  SharedTexPublisher& operator=(const SharedTexPublisher&) = delete;

  void Configure(DWORD parent_pid, LONG luid_low, LONG luid_high);

  bool CacheLayer(int layer, HANDLE cef_handle, uint32_t width, uint32_t height,
                  const SharedDirtyRect* dirty, size_t dirty_n);

  bool PublishLayer(int layer, uint64_t* out_remote_handle);

  uint32_t layer_w(int layer) const {
    return (layer >= 0 && layer <= 1) ? layer_w_[layer] : 0;
  }
  uint32_t layer_h(int layer) const {
    return (layer >= 0 && layer <= 1) ? layer_h_[layer] : 0;
  }

 private:
  bool EnsureDevice();
  bool EnsureLayerTexture(int layer, uint32_t width, uint32_t height, DXGI_FORMAT format);
  void ResetLayer(int layer);
  void EnsureFrameEvent();

  DWORD parent_pid_ = 0;
  HANDLE frame_ready_ = nullptr;
  LONG luid_low_ = 0;
  LONG luid_high_ = 0;

  ID3D11Device* device_ = nullptr;
  ID3D11Device1* device1_ = nullptr;
  ID3D11DeviceContext* ctx_ = nullptr;

  ID3D11Texture2D* layer_tex_[2] = {};
  IDXGIKeyedMutex* layer_mutex_[2] = {};
  HANDLE local_nt_[2] = {};
  HANDLE remote_nt_[2] = {};
  uint32_t layer_w_[2] = {0, 0};
  uint32_t layer_h_[2] = {0, 0};
  DXGI_FORMAT layer_fmt_[2] = {DXGI_FORMAT_UNKNOWN, DXGI_FORMAT_UNKNOWN};
};
