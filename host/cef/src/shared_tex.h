#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#include <d3d11_1.h>
#include <dxgi.h>

#include <cstdint>

// Owns a cross-process NT-shared D3D11 texture. Copies CEF OSR pool textures
// into layer caches, composites chrome+content, and exposes a handle duplicated
// into the Electron process.
class SharedTexPublisher {
 public:
  SharedTexPublisher() = default;
  ~SharedTexPublisher();

  SharedTexPublisher(const SharedTexPublisher&) = delete;
  SharedTexPublisher& operator=(const SharedTexPublisher&) = delete;

  void Configure(DWORD parent_pid, LONG luid_low, LONG luid_high);

  /** Cache one OSR layer (0=chrome full frame, 1=content hole). */
  bool CacheLayer(int layer, HANDLE cef_handle, uint32_t width, uint32_t height);

  /**
   * Composite into a fullscreen publish texture: clear transparent, blit chrome
   * into the inner window region, then optional content at (content_x, content_y).
   */
  bool PublishComposite(int win_x, int win_y, int win_w, int win_h, int content_x,
                        int content_y, bool include_content, uint64_t* out_remote_handle);

  bool has_chrome() const { return layer_tex_[0] != nullptr; }
  uint32_t chrome_w() const { return layer_w_[0]; }
  uint32_t chrome_h() const { return layer_h_[0]; }

 private:
  bool EnsureDevice();
  bool EnsureLayerTexture(int layer, uint32_t width, uint32_t height, DXGI_FORMAT format);
  bool EnsurePublishTexture(uint32_t width, uint32_t height, DXGI_FORMAT format);
  void ResetPublish();
  void ResetLayer(int layer);

  DWORD parent_pid_ = 0;
  LONG luid_low_ = 0;
  LONG luid_high_ = 0;

  ID3D11Device* device_ = nullptr;
  ID3D11Device1* device1_ = nullptr;
  ID3D11DeviceContext* ctx_ = nullptr;

  ID3D11Texture2D* layer_tex_[2] = {nullptr, nullptr};
  uint32_t layer_w_[2] = {0, 0};
  uint32_t layer_h_[2] = {0, 0};
  DXGI_FORMAT layer_fmt_[2] = {DXGI_FORMAT_UNKNOWN, DXGI_FORMAT_UNKNOWN};

  ID3D11Texture2D* shared_tex_ = nullptr;
  ID3D11RenderTargetView* shared_rtv_ = nullptr;
  HANDLE local_nt_ = nullptr;
  HANDLE remote_nt_ = nullptr;
  uint32_t pub_w_ = 0;
  uint32_t pub_h_ = 0;
  DXGI_FORMAT pub_fmt_ = DXGI_FORMAT_UNKNOWN;
};
