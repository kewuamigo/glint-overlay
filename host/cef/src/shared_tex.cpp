#include "shared_tex.h"

#include <dxgi1_2.h>

#include <algorithm>
#include <cstdio>

SharedTexPublisher::~SharedTexPublisher() {
  ResetPublish();
  ResetLayer(0);
  ResetLayer(1);
  if (ctx_) {
    ctx_->Release();
    ctx_ = nullptr;
  }
  if (device1_) {
    device1_->Release();
    device1_ = nullptr;
  }
  if (device_) {
    device_->Release();
    device_ = nullptr;
  }
}

void SharedTexPublisher::Configure(DWORD parent_pid, LONG luid_low, LONG luid_high) {
  parent_pid_ = parent_pid;
  luid_low_ = luid_low;
  luid_high_ = luid_high;
}

void SharedTexPublisher::ResetLayer(int layer) {
  if (layer < 0 || layer > 1) return;
  if (layer_tex_[layer]) {
    layer_tex_[layer]->Release();
    layer_tex_[layer] = nullptr;
  }
  layer_w_[layer] = 0;
  layer_h_[layer] = 0;
  layer_fmt_[layer] = DXGI_FORMAT_UNKNOWN;
}

void SharedTexPublisher::ResetPublish() {
  // remote_nt_ is a handle value valid only in the Electron (parent) process.
  // Never CloseHandle(remote_nt_) here — that closes a wrong object in *this*
  // process and corrupts CEF's handle table (black frames). Close it in the
  // parent via DUPLICATE_CLOSE_SOURCE; Electron owns the duplicate.
  if (remote_nt_) {
    if (parent_pid_ != 0) {
      HANDLE parent = OpenProcess(PROCESS_DUP_HANDLE, FALSE, parent_pid_);
      if (parent) {
        DuplicateHandle(parent, remote_nt_, nullptr, nullptr, 0, FALSE,
                        DUPLICATE_CLOSE_SOURCE);
        CloseHandle(parent);
      }
    }
    remote_nt_ = nullptr;
  }
  if (local_nt_) {
    CloseHandle(local_nt_);
    local_nt_ = nullptr;
  }
  if (shared_rtv_) {
    shared_rtv_->Release();
    shared_rtv_ = nullptr;
  }
  if (shared_tex_) {
    shared_tex_->Release();
    shared_tex_ = nullptr;
  }
  pub_w_ = 0;
  pub_h_ = 0;
  pub_fmt_ = DXGI_FORMAT_UNKNOWN;
}

bool SharedTexPublisher::EnsureDevice() {
  if (device1_ && ctx_) return true;

  IDXGIAdapter* adapter = nullptr;
  if (luid_low_ != 0 || luid_high_ != 0) {
    IDXGIFactory1* factory = nullptr;
    if (SUCCEEDED(CreateDXGIFactory1(__uuidof(IDXGIFactory1),
                                     reinterpret_cast<void**>(&factory))) &&
        factory) {
      for (UINT i = 0;; ++i) {
        IDXGIAdapter* cand = nullptr;
        if (factory->EnumAdapters(i, &cand) == DXGI_ERROR_NOT_FOUND) break;
        DXGI_ADAPTER_DESC desc{};
        if (SUCCEEDED(cand->GetDesc(&desc)) &&
            desc.AdapterLuid.LowPart == static_cast<DWORD>(luid_low_) &&
            desc.AdapterLuid.HighPart == luid_high_) {
          adapter = cand;
          break;
        }
        cand->Release();
      }
      factory->Release();
    }
  }

  D3D_FEATURE_LEVEL level = D3D_FEATURE_LEVEL_11_0;
  ID3D11Device* device = nullptr;
  ID3D11DeviceContext* ctx = nullptr;
  const HRESULT hr = D3D11CreateDevice(
      adapter, adapter ? D3D_DRIVER_TYPE_UNKNOWN : D3D_DRIVER_TYPE_HARDWARE, nullptr,
      D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
      &level, 1, D3D11_SDK_VERSION, &device, nullptr, &ctx);
  if (adapter) adapter->Release();
  if (FAILED(hr) || !device || !ctx) {
    std::fprintf(stderr, "shared_tex: D3D11CreateDevice failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    return false;
  }

  ID3D11Device1* device1 = nullptr;
  if (FAILED(device->QueryInterface(__uuidof(ID3D11Device1),
                                    reinterpret_cast<void**>(&device1))) ||
      !device1) {
    std::fprintf(stderr, "shared_tex: ID3D11Device1 QI failed\n");
    ctx->Release();
    device->Release();
    return false;
  }

  device_ = device;
  device1_ = device1;
  ctx_ = ctx;
  return true;
}

bool SharedTexPublisher::EnsureLayerTexture(int layer, uint32_t width, uint32_t height,
                                            DXGI_FORMAT format) {
  if (layer < 0 || layer > 1) return false;
  if (layer_tex_[layer] && layer_w_[layer] == width && layer_h_[layer] == height &&
      layer_fmt_[layer] == format) {
    return true;
  }
  if (!EnsureDevice()) return false;

  ResetLayer(layer);

  D3D11_TEXTURE2D_DESC desc{};
  desc.Width = width;
  desc.Height = height;
  desc.MipLevels = 1;
  desc.ArraySize = 1;
  desc.Format = format;
  desc.SampleDesc.Count = 1;
  desc.Usage = D3D11_USAGE_DEFAULT;
  desc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;

  ID3D11Texture2D* tex = nullptr;
  const HRESULT hr = device_->CreateTexture2D(&desc, nullptr, &tex);
  if (FAILED(hr) || !tex) {
    std::fprintf(stderr, "shared_tex: CreateTexture2D layer failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    return false;
  }
  layer_tex_[layer] = tex;
  layer_w_[layer] = width;
  layer_h_[layer] = height;
  layer_fmt_[layer] = format;
  return true;
}

bool SharedTexPublisher::EnsurePublishTexture(uint32_t width, uint32_t height,
                                              DXGI_FORMAT format) {
  if (shared_tex_ && pub_w_ == width && pub_h_ == height && pub_fmt_ == format &&
      remote_nt_) {
    return true;
  }
  if (!EnsureDevice() || parent_pid_ == 0) return false;

  ResetPublish();

  D3D11_TEXTURE2D_DESC desc{};
  desc.Width = width;
  desc.Height = height;
  desc.MipLevels = 1;
  desc.ArraySize = 1;
  desc.Format = format;
  desc.SampleDesc.Count = 1;
  desc.Usage = D3D11_USAGE_DEFAULT;
  desc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
  desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE;

  ID3D11Texture2D* tex = nullptr;
  HRESULT hr = device_->CreateTexture2D(&desc, nullptr, &tex);
  if (FAILED(hr) || !tex) {
    std::fprintf(stderr, "shared_tex: CreateTexture2D publish failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    return false;
  }

  ID3D11RenderTargetView* rtv = nullptr;
  D3D11_RENDER_TARGET_VIEW_DESC rtv_desc{};
  rtv_desc.Format = format;
  rtv_desc.ViewDimension = D3D11_RTV_DIMENSION_TEXTURE2D;
  hr = device_->CreateRenderTargetView(tex, &rtv_desc, &rtv);
  if (FAILED(hr) || !rtv) {
    std::fprintf(stderr, "shared_tex: CreateRenderTargetView failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    tex->Release();
    return false;
  }

  IDXGIResource1* res1 = nullptr;
  hr = tex->QueryInterface(__uuidof(IDXGIResource1), reinterpret_cast<void**>(&res1));
  if (FAILED(hr) || !res1) {
    rtv->Release();
    tex->Release();
    return false;
  }

  HANDLE local = nullptr;
  hr = res1->CreateSharedHandle(
      nullptr, DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE, nullptr, &local);
  res1->Release();
  if (FAILED(hr) || !local) {
    std::fprintf(stderr, "shared_tex: CreateSharedHandle failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    rtv->Release();
    tex->Release();
    return false;
  }

  HANDLE parent = OpenProcess(PROCESS_DUP_HANDLE, FALSE, parent_pid_);
  if (!parent) {
    std::fprintf(stderr, "shared_tex: OpenProcess(%lu) failed err=%lu\n",
                 static_cast<unsigned long>(parent_pid_),
                 static_cast<unsigned long>(GetLastError()));
    CloseHandle(local);
    rtv->Release();
    tex->Release();
    return false;
  }

  HANDLE remote = nullptr;
  const BOOL ok = DuplicateHandle(GetCurrentProcess(), local, parent, &remote, 0, FALSE,
                                  DUPLICATE_SAME_ACCESS);
  CloseHandle(parent);
  if (!ok || !remote) {
    std::fprintf(stderr, "shared_tex: DuplicateHandle failed err=%lu\n",
                 static_cast<unsigned long>(GetLastError()));
    CloseHandle(local);
    rtv->Release();
    tex->Release();
    return false;
  }

  shared_tex_ = tex;
  shared_rtv_ = rtv;
  local_nt_ = local;
  remote_nt_ = remote;
  pub_w_ = width;
  pub_h_ = height;
  pub_fmt_ = format;
  return true;
}

bool SharedTexPublisher::CacheLayer(int layer, HANDLE cef_handle, uint32_t width,
                                    uint32_t height) {
  if (layer < 0 || layer > 1 || !cef_handle || width == 0 || height == 0) return false;
  if (!EnsureDevice()) return false;

  ID3D11Texture2D* src = nullptr;
  HRESULT hr = device1_->OpenSharedResource1(cef_handle, __uuidof(ID3D11Texture2D),
                                             reinterpret_cast<void**>(&src));
  if (FAILED(hr) || !src) {
    std::fprintf(stderr, "shared_tex: OpenSharedResource1 failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    return false;
  }

  D3D11_TEXTURE2D_DESC src_desc{};
  src->GetDesc(&src_desc);
  // CEF coded_size textures are often padded — crop to the view size we asked for.
  const uint32_t tw = std::min(width, src_desc.Width ? src_desc.Width : width);
  const uint32_t th = std::min(height, src_desc.Height ? src_desc.Height : height);

  if (!EnsureLayerTexture(layer, tw, th, src_desc.Format)) {
    src->Release();
    return false;
  }

  if (tw == src_desc.Width && th == src_desc.Height) {
    ctx_->CopyResource(layer_tex_[layer], src);
  } else {
    D3D11_BOX box{};
    box.left = 0;
    box.top = 0;
    box.front = 0;
    box.right = tw;
    box.bottom = th;
    box.back = 1;
    ctx_->CopySubresourceRegion(layer_tex_[layer], 0, 0, 0, 0, src, 0, &box);
  }
  src->Release();
  return true;
}

bool SharedTexPublisher::PublishComposite(int win_x, int win_y, int win_w, int win_h,
                                          int content_x, int content_y,
                                          bool include_content,
                                          uint64_t* out_remote_handle) {
  if (!out_remote_handle || !layer_tex_[0]) return false;
  if (!EnsureDevice()) return false;

  const uint32_t tw = layer_w_[0];
  const uint32_t th = layer_h_[0];
  if (tw == 0 || th == 0) return false;

  if (!EnsurePublishTexture(tw, th, DXGI_FORMAT_B8G8R8A8_UNORM) || !shared_rtv_) return false;

  const float clear[4] = {0.f, 0.f, 0.f, 0.f};
  ctx_->ClearRenderTargetView(shared_rtv_, clear);

  const int wx = std::max(0, win_x);
  const int wy = std::max(0, win_y);
  if (win_w > 0 && win_h > 0 && wx < static_cast<int>(tw) && wy < static_cast<int>(th) &&
      wx < static_cast<int>(layer_w_[0]) && wy < static_cast<int>(layer_h_[0])) {
    const UINT copy_w = static_cast<UINT>(
        std::min(static_cast<uint32_t>(win_w),
                 std::min(tw - static_cast<uint32_t>(wx),
                          layer_w_[0] - static_cast<uint32_t>(wx))));
    const UINT copy_h = static_cast<UINT>(
        std::min(static_cast<uint32_t>(win_h),
                 std::min(th - static_cast<uint32_t>(wy),
                          layer_h_[0] - static_cast<uint32_t>(wy))));
    if (copy_w > 0 && copy_h > 0) {
      D3D11_BOX src_box{};
      src_box.left = static_cast<UINT>(wx);
      src_box.top = static_cast<UINT>(wy);
      src_box.front = 0;
      src_box.right = static_cast<UINT>(wx) + copy_w;
      src_box.bottom = static_cast<UINT>(wy) + copy_h;
      src_box.back = 1;
      ctx_->CopySubresourceRegion(shared_tex_, 0, static_cast<UINT>(wx),
                                  static_cast<UINT>(wy), 0, layer_tex_[0], 0, &src_box);
    }
  }

  if (include_content && layer_tex_[1] && layer_w_[1] > 0 && layer_h_[1] > 0) {
    const int ox = std::max(0, content_x);
    const int oy = std::max(0, content_y);
    if (ox < static_cast<int>(tw) && oy < static_cast<int>(th)) {
      const UINT copy_w =
          std::min(layer_w_[1], tw - static_cast<uint32_t>(ox));
      const UINT copy_h =
          std::min(layer_h_[1], th - static_cast<uint32_t>(oy));
      if (copy_w > 0 && copy_h > 0) {
        D3D11_BOX src_box{};
        src_box.left = 0;
        src_box.top = 0;
        src_box.front = 0;
        src_box.right = copy_w;
        src_box.bottom = copy_h;
        src_box.back = 1;
        ctx_->CopySubresourceRegion(shared_tex_, 0, static_cast<UINT>(ox),
                                    static_cast<UINT>(oy), 0, layer_tex_[1], 0,
                                    &src_box);
      }
    }
  }

  ctx_->Flush();
  *out_remote_handle = static_cast<uint64_t>(reinterpret_cast<uintptr_t>(remote_nt_));
  return true;
}
