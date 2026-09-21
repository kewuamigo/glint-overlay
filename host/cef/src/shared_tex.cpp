#include "shared_tex.h"

#include <dxgi1_2.h>

#include <algorithm>
#include <cstdio>
#include <cwchar>

namespace {
constexpr HRESULT kWaitAbandoned = static_cast<HRESULT>(0x80);

bool DirtyCovers(const SharedDirtyRect* dirty, size_t dirty_n, uint32_t w, uint32_t h) {
  if (dirty_n == 0 || !dirty) return true;
  for (size_t i = 0; i < dirty_n; ++i) {
    if (dirty[i].x <= 0 && dirty[i].y <= 0 && dirty[i].w >= static_cast<int>(w) &&
        dirty[i].h >= static_cast<int>(h)) {
      return true;
    }
  }
  return false;
}
}  // namespace

SharedTexPublisher::~SharedTexPublisher() {
  ResetLayer(0);
  ResetLayer(1);
  if (frame_ready_) {
    CloseHandle(frame_ready_);
    frame_ready_ = nullptr;
  }
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

void SharedTexPublisher::EnsureFrameEvent() {
  if (frame_ready_ || parent_pid_ == 0) return;
  wchar_t name[64];
  swprintf_s(name, L"Local\\GlintCefFrame-%lu", static_cast<unsigned long>(parent_pid_));
  frame_ready_ = CreateEventW(nullptr, FALSE, FALSE, name);
  if (!frame_ready_) {
    std::fprintf(stderr, "shared_tex: CreateEvent frame-ready failed err=%lu\n",
                 static_cast<unsigned long>(GetLastError()));
  }
}

void SharedTexPublisher::ResetLayer(int layer) {
  if (layer < 0 || layer > 1) return;
  if (remote_nt_[layer]) {
    if (parent_pid_ != 0) {
      HANDLE parent = OpenProcess(PROCESS_DUP_HANDLE, FALSE, parent_pid_);
      if (parent) {
        DuplicateHandle(parent, remote_nt_[layer], nullptr, nullptr, 0, FALSE,
                        DUPLICATE_CLOSE_SOURCE);
        CloseHandle(parent);
      }
    }
    remote_nt_[layer] = nullptr;
  }
  if (local_nt_[layer]) {
    CloseHandle(local_nt_[layer]);
    local_nt_[layer] = nullptr;
  }
  if (layer_mutex_[layer]) {
    layer_mutex_[layer]->Release();
    layer_mutex_[layer] = nullptr;
  }
  if (layer_tex_[layer]) {
    layer_tex_[layer]->Release();
    layer_tex_[layer] = nullptr;
  }
  layer_w_[layer] = 0;
  layer_h_[layer] = 0;
  layer_fmt_[layer] = DXGI_FORMAT_UNKNOWN;
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
      layer_fmt_[layer] == format && remote_nt_[layer] && layer_mutex_[layer]) {
    return true;
  }
  if (!EnsureDevice() || parent_pid_ == 0) return false;

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
  desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX | D3D11_RESOURCE_MISC_SHARED_NTHANDLE;

  ID3D11Texture2D* tex = nullptr;
  HRESULT hr = device_->CreateTexture2D(&desc, nullptr, &tex);
  if (FAILED(hr) || !tex) {
    std::fprintf(stderr, "shared_tex: CreateTexture2D layer failed hr=0x%08lx\n",
                 static_cast<unsigned long>(hr));
    return false;
  }

  IDXGIKeyedMutex* mutex = nullptr;
  hr = tex->QueryInterface(__uuidof(IDXGIKeyedMutex), reinterpret_cast<void**>(&mutex));
  if (FAILED(hr) || !mutex) {
    tex->Release();
    return false;
  }

  IDXGIResource1* res1 = nullptr;
  hr = tex->QueryInterface(__uuidof(IDXGIResource1), reinterpret_cast<void**>(&res1));
  if (FAILED(hr) || !res1) {
    mutex->Release();
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
    mutex->Release();
    tex->Release();
    return false;
  }

  HANDLE parent = OpenProcess(PROCESS_DUP_HANDLE, FALSE, parent_pid_);
  if (!parent) {
    std::fprintf(stderr, "shared_tex: OpenProcess(%lu) failed err=%lu\n",
                 static_cast<unsigned long>(parent_pid_),
                 static_cast<unsigned long>(GetLastError()));
    CloseHandle(local);
    mutex->Release();
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
    mutex->Release();
    tex->Release();
    return false;
  }

  layer_tex_[layer] = tex;
  layer_mutex_[layer] = mutex;
  local_nt_[layer] = local;
  remote_nt_[layer] = remote;
  layer_w_[layer] = width;
  layer_h_[layer] = height;
  layer_fmt_[layer] = format;
  return true;
}

bool SharedTexPublisher::CacheLayer(int layer, HANDLE cef_handle, uint32_t width,
                                    uint32_t height, const SharedDirtyRect* dirty,
                                    size_t dirty_n) {
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
  const uint32_t tw = std::min(width, src_desc.Width ? src_desc.Width : width);
  const uint32_t th = std::min(height, src_desc.Height ? src_desc.Height : height);

  if (!EnsureLayerTexture(layer, tw, th, src_desc.Format)) {
    src->Release();
    return false;
  }

  const HRESULT acquired = layer_mutex_[layer]->AcquireSync(0, 0);
  if (acquired != S_OK && acquired != kWaitAbandoned) {
    src->Release();
    return remote_nt_[layer] != nullptr;
  }

  if (DirtyCovers(dirty, dirty_n, tw, th) ||
      (tw == src_desc.Width && th == src_desc.Height && dirty_n == 0)) {
    if (tw == src_desc.Width && th == src_desc.Height) {
      ctx_->CopyResource(layer_tex_[layer], src);
    } else {
      D3D11_BOX box{};
      box.right = tw;
      box.bottom = th;
      box.back = 1;
      ctx_->CopySubresourceRegion(layer_tex_[layer], 0, 0, 0, 0, src, 0, &box);
    }
  } else {
    for (size_t i = 0; i < dirty_n; ++i) {
      const int x = std::max(0, dirty[i].x);
      const int y = std::max(0, dirty[i].y);
      if (x >= static_cast<int>(tw) || y >= static_cast<int>(th) || dirty[i].w <= 0 ||
          dirty[i].h <= 0) {
        continue;
      }
      const UINT cw =
          static_cast<UINT>(std::min(dirty[i].w, static_cast<int>(tw) - x));
      const UINT ch =
          static_cast<UINT>(std::min(dirty[i].h, static_cast<int>(th) - y));
      if (cw == 0 || ch == 0) continue;
      if (x + static_cast<int>(cw) > static_cast<int>(src_desc.Width) ||
          y + static_cast<int>(ch) > static_cast<int>(src_desc.Height)) {
        continue;
      }
      D3D11_BOX box{};
      box.left = static_cast<UINT>(x);
      box.top = static_cast<UINT>(y);
      box.front = 0;
      box.right = static_cast<UINT>(x) + cw;
      box.bottom = static_cast<UINT>(y) + ch;
      box.back = 1;
      ctx_->CopySubresourceRegion(layer_tex_[layer], 0, static_cast<UINT>(x),
                                  static_cast<UINT>(y), 0, src, 0, &box);
    }
  }
  ctx_->Flush();
  layer_mutex_[layer]->ReleaseSync(0);
  src->Release();
  return true;
}

bool SharedTexPublisher::PublishLayer(int layer, uint64_t* out_remote_handle) {
  if (layer < 0 || layer > 1 || !out_remote_handle || !remote_nt_[layer]) return false;
  *out_remote_handle = static_cast<uint64_t>(reinterpret_cast<uintptr_t>(remote_nt_[layer]));
  EnsureFrameEvent();
  if (frame_ready_) {
    SetEvent(frame_ready_);
  }
  return true;
}
