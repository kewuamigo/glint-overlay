use core::{ffi::c_void, mem};

use anyhow::Context;
use glint_overlay_hook::DetourHook;
use once_cell::sync::{Lazy, OnceCell};
use tracing::{debug, error, trace, warn};
use windows::{
    Win32::{
        Foundation::{HWND, LUID, RECT},
        Graphics::{
            Direct3D9::{
                D3DDEVICE_CREATION_PARAMETERS, D3DDEVTYPE, D3DDISPLAYMODEEX, D3DPRESENT_PARAMETERS,
                IDirect3D9, IDirect3D9Ex, IDirect3DDevice9, IDirect3DDevice9Ex,
                IDirect3DSwapChain9,
            },
            Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
            Gdi::RGNDATA,
        },
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{HRESULT, Interface, s},
};

use crate::{
    backend::{Backends, render::Renderer},
    event_sink::OverlayEventSink,
    hook::dx::renderer_map::with_map_entry,
    renderer::dx9::Dx9Renderer,
    types::IntDashMap,
    util::{after_original_present, find_adapter_by_luid},
};

/// Mapping from [`IDirect3DDevice9`] to [`Dx9Renderer`].
static RENDERERS: Lazy<IntDashMap<usize, Dx9Renderer>> = Lazy::new(IntDashMap::default);

#[inline]
fn with_or_init_renderer<R>(
    device: &IDirect3DDevice9,
    f: impl FnOnce(&mut Dx9Renderer) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    with_map_entry(
        &RENDERERS,
        device.as_raw() as usize,
        || {
            debug!("initializing dx9 renderer");
            Dx9Renderer::new(device)
        },
        None::<fn()>,
        f,
    )
}

#[tracing::instrument]
extern "system" fn hooked_present(
    this: *mut c_void,
    source_rect: *const RECT,
    dest_rect: *const RECT,
    dest_window_override: HWND,
    dirty_region: *const RGNDATA,
) -> HRESULT {
    trace!("IDirect3DDevice9::Present called");

    if OverlayEventSink::connected() {
        if let Some(device) = unsafe { IDirect3DDevice9::from_raw_borrowed(&this) } {
            let mut hwnd = dest_window_override;
            if hwnd.is_invalid() {
                if let Ok(swapchain) = unsafe { device.GetSwapChain(0) } {
                    let mut params = D3DPRESENT_PARAMETERS::default();
                    if unsafe { swapchain.GetPresentParameters(&mut params) }.is_ok() {
                        hwnd = params.hDeviceWindow;
                    }
                }
            }
            if !hwnd.is_invalid() {
                draw_overlay(hwnd, device);
            }
        }
    }

    after_original_present(|| unsafe {
        HOOK.present.wait().original_fn()(
            this,
            source_rect,
            dest_rect,
            dest_window_override,
            dirty_region,
        )
    })
}

#[tracing::instrument]
extern "system" fn hooked_swapchain_present(
    this: *mut c_void,
    source_rect: *const RECT,
    dest_rect: *const RECT,
    dest_window_override: HWND,
    dirty_region: *const RGNDATA,
    dw_flags: u32,
) -> HRESULT {
    trace!("IDirect3DSwapChain9::Present called");

    if let Some(swapchain) = unsafe { IDirect3DSwapChain9::from_raw_borrowed(&this) } {
        if let Ok(device) = unsafe { swapchain.GetDevice() } {
            let mut hwnd = dest_window_override;
            if hwnd.is_invalid() {
                let mut params = D3DPRESENT_PARAMETERS::default();
                if unsafe { swapchain.GetPresentParameters(&mut params) }.is_ok() {
                    hwnd = params.hDeviceWindow;
                }
            }
            if !hwnd.is_invalid() {
                draw_overlay(hwnd, &device);
            }
        }
    }

    after_original_present(|| unsafe {
        HOOK.swapchain_present.wait().original_fn()(
            this,
            source_rect,
            dest_rect,
            dest_window_override,
            dirty_region,
            dw_flags,
        )
    })
}

#[tracing::instrument]
extern "system" fn hooked_release(this: *mut c_void) -> u32 {
    trace!("IDirect3DDevice9::Release called");

    let count = unsafe { HOOK.release.wait().original_fn()(this) };

    // renderer includes refs from IDirect3DVertexBuffer9, IDirect3DStateBlock9 and optionally texture.
    if count == 2 || count == 3 {
        cleanup_renderer(this as _);
    }

    count
}

#[tracing::instrument]
extern "system" fn hooked_present_ex(
    this: *mut c_void,
    source_rect: *const RECT,
    dest_rect: *const RECT,
    dest_window_override: HWND,
    dirty_region: *const RGNDATA,
    dw_flags: u32,
) -> HRESULT {
    trace!("IDirect3DDevice9Ex::PresentEx called");

    if OverlayEventSink::connected() {
        if let Some(device) = unsafe { IDirect3DDevice9::from_raw_borrowed(&this) } {
            let mut hwnd = dest_window_override;
            if hwnd.is_invalid() {
                if let Ok(swapchain) = unsafe { device.GetSwapChain(0) } {
                    let mut params = D3DPRESENT_PARAMETERS::default();
                    if unsafe { swapchain.GetPresentParameters(&mut params) }.is_ok() {
                        hwnd = params.hDeviceWindow;
                    }
                }
            }
            if !hwnd.is_invalid() {
                draw_overlay(hwnd, device);
            }
        }
    }

    after_original_present(|| unsafe {
        HOOK.present_ex.wait().original_fn()(
            this,
            source_rect,
            dest_rect,
            dest_window_override,
            dirty_region,
            dw_flags,
        )
    })
}

fn draw_overlay(hwnd: HWND, device: &IDirect3DDevice9) {
    let res = Backends::with_or_init_backend(
        hwnd.0 as _,
        || {
            let d3d9ex = unsafe { device.GetDirect3D() }
                .ok()?
                .cast::<IDirect3D9Ex>()
                .ok()?;
            let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok()?;

            let mut param = D3DDEVICE_CREATION_PARAMETERS::default();
            unsafe { device.GetCreationParameters(&mut param) }.ok()?;

            let mut luid = LUID::default();
            unsafe { d3d9ex.GetAdapterLUID(param.AdapterOrdinal, &mut luid) }.ok()?;

            find_adapter_by_luid(&factory, luid)
        },
        |backend| {
            if backend.independent_active() {
                return;
            }
            let render = &mut *backend.render.lock();
            match render.renderer {
                Some(Renderer::Dx9) => {}
                Some(_) => {
                    trace!("ignoring dx9 rendering");
                    return;
                }
                None => {
                    debug!("Found dx9 window");
                    render.renderer = Some(Renderer::Dx9);
                    // wait next swap for possible remaining renderer check
                    return;
                }
            };

            let Some(surface) = render.surface.get() else {
                return;
            };

            let position = render.position;
            let screen = render.window_size;
            let interop = &mut render.interop;
            _ = with_or_init_renderer(device, move |renderer| {
                trace!("using dx9 renderer");

                renderer
                    .update_texture(device, surface, &interop.device, interop.cx.get_mut())
                    .context("failed to update dx9 texture")?;

                unsafe { device.BeginScene() }.context("BeginScene failed")?;
                let res = renderer.draw(device, position, screen);
                trace!("dx9 render: {:?}", res);
                unsafe { device.EndScene() }.context("EndScene failed")?;
                Ok(res)
            });
        },
    );

    if let Err(_err) = res {
        error!("Backends::with_or_init_backend failed. err: {:?}", _err);
    }
}

fn cleanup_renderer(device: usize) {
    if RENDERERS.remove(&device).is_none() {
        return;
    }

    debug!("dx9 renderer cleanup");
}

#[tracing::instrument]
extern "system" fn hooked_reset(this: *mut c_void, param: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
    trace!("Reset called");
    cleanup_renderer(this as _);

    unsafe { HOOK.reset.wait().original_fn()(this, param) }
}

#[tracing::instrument]
extern "system" fn hooked_reset_ex(
    this: *mut c_void,
    param: *mut D3DPRESENT_PARAMETERS,
    fullscreen_display_mode: *mut D3DDISPLAYMODEEX,
) -> HRESULT {
    trace!("ResetEx called");
    cleanup_renderer(this as _);

    unsafe { HOOK.reset_ex.wait().original_fn()(this, param, fullscreen_display_mode) }
}

type Create9Fn = unsafe extern "system" fn(u32) -> *mut c_void;
type Create9ExFn = unsafe extern "system" fn(u32, *mut *mut c_void) -> HRESULT;
type CreateDeviceFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    D3DDEVTYPE,
    HWND,
    u32,
    *mut D3DPRESENT_PARAMETERS,
    *mut *mut c_void,
) -> HRESULT;
type CreateDeviceExFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    D3DDEVTYPE,
    HWND,
    u32,
    *mut D3DPRESENT_PARAMETERS,
    *mut D3DDISPLAYMODEEX,
    *mut *mut c_void,
) -> HRESULT;
type PresentFn = unsafe extern "system" fn(
    *mut c_void,
    *const RECT,
    *const RECT,
    HWND,
    *const RGNDATA,
) -> HRESULT;
type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type PresentExFn = unsafe extern "system" fn(
    *mut c_void,
    *const RECT,
    *const RECT,
    HWND,
    *const RGNDATA,
    u32,
) -> HRESULT;
type SwapchainPresentFn = unsafe extern "system" fn(
    *mut c_void,
    *const RECT,
    *const RECT,
    HWND,
    *const RGNDATA,
    u32,
) -> HRESULT;
type ResetFn = unsafe extern "system" fn(*mut c_void, *mut D3DPRESENT_PARAMETERS) -> HRESULT;
type ResetExFn = unsafe extern "system" fn(
    *mut c_void,
    *mut D3DPRESENT_PARAMETERS,
    *mut D3DDISPLAYMODEEX,
) -> HRESULT;

struct Hook {
    create9: OnceCell<DetourHook<Create9Fn>>,
    create9ex: OnceCell<DetourHook<Create9ExFn>>,
    create_device: OnceCell<DetourHook<CreateDeviceFn>>,
    create_device_ex: OnceCell<DetourHook<CreateDeviceExFn>>,
    present: OnceCell<DetourHook<PresentFn>>,
    release: OnceCell<DetourHook<ReleaseFn>>,
    present_ex: OnceCell<DetourHook<PresentExFn>>,
    swapchain_present: OnceCell<DetourHook<SwapchainPresentFn>>,
    reset: OnceCell<DetourHook<ResetFn>>,
    reset_ex: OnceCell<DetourHook<ResetExFn>>,
}

static HOOK: Hook = Hook {
    create9: OnceCell::new(),
    create9ex: OnceCell::new(),
    create_device: OnceCell::new(),
    create_device_ex: OnceCell::new(),
    present: OnceCell::new(),
    release: OnceCell::new(),
    present_ex: OnceCell::new(),
    swapchain_present: OnceCell::new(),
    reset: OnceCell::new(),
    reset_ex: OnceCell::new(),
};

fn attach<F: Copy + std::fmt::Debug>(
    cell: &OnceCell<DetourHook<F>>,
    target: F,
    detour: F,
    label: &str,
) {
    if cell.get().is_some() {
        return;
    }
    match unsafe { DetourHook::attach(target, detour) } {
        Ok(h) => {
            let _ = cell.set(h);
            debug!("hooked {label}");
        }
        Err(err) => warn!("Failed hooking {label}: {err:?}"),
    }
}

fn wrap_created(hr: HRESULT, out: *mut *mut c_void, wrap: impl FnOnce(*mut c_void)) {
    if !hr.is_ok() || out.is_null() {
        return;
    }
    let raw = unsafe { *out };
    if !raw.is_null() {
        wrap(raw);
    }
}

fn create_then_wrap(
    original: impl FnOnce() -> HRESULT,
    out: *mut *mut c_void,
    wrap: impl FnOnce(*mut c_void),
) -> HRESULT {
    let hr = original();
    wrap_created(hr, out, wrap);
    hr
}

fn wrap_d3d9(raw: *mut c_void) {
    let Some(d3d9) = (unsafe { IDirect3D9::from_raw_borrowed(&raw) }) else {
        return;
    };
    attach(
        &HOOK.create_device,
        Interface::vtable(d3d9).CreateDevice,
        hooked_create_device as _,
        "IDirect3D9::CreateDevice",
    );
    let Ok(d3d9ex) = d3d9.cast::<IDirect3D9Ex>() else {
        return;
    };
    attach(
        &HOOK.create_device_ex,
        Interface::vtable(&d3d9ex).CreateDeviceEx,
        hooked_create_device_ex as _,
        "IDirect3D9Ex::CreateDeviceEx",
    );
}

fn wrap_device(raw: *mut c_void) {
    let Some(device) = (unsafe { IDirect3DDevice9::from_raw_borrowed(&raw) }) else {
        return;
    };
    let vt = Interface::vtable(device);
    attach(
        &HOOK.present,
        vt.Present,
        hooked_present as _,
        "IDirect3DDevice9::Present",
    );
    attach(
        &HOOK.release,
        vt.base__.Release,
        hooked_release as _,
        "IDirect3DDevice9::Release",
    );
    attach(
        &HOOK.reset,
        vt.Reset,
        hooked_reset as _,
        "IDirect3DDevice9::Reset",
    );
    if let Ok(swapchain) = unsafe { device.GetSwapChain(0) } {
        attach(
            &HOOK.swapchain_present,
            Interface::vtable(&swapchain).Present,
            hooked_swapchain_present as _,
            "IDirect3DSwapChain9::Present",
        );
    }
    let Ok(ex) = device.cast::<IDirect3DDevice9Ex>() else {
        return;
    };
    let vt = Interface::vtable(&ex);
    attach(
        &HOOK.present_ex,
        vt.PresentEx,
        hooked_present_ex as _,
        "IDirect3DDevice9Ex::PresentEx",
    );
    attach(
        &HOOK.reset_ex,
        vt.ResetEx,
        hooked_reset_ex as _,
        "IDirect3DDevice9Ex::ResetEx",
    );
}

extern "system" fn hooked_direct3d_create9(sdk: u32) -> *mut c_void {
    let raw = unsafe { HOOK.create9.wait().original_fn()(sdk) };
    if !raw.is_null() {
        wrap_d3d9(raw);
    }
    raw
}

extern "system" fn hooked_direct3d_create9_ex(sdk: u32, out: *mut *mut c_void) -> HRESULT {
    create_then_wrap(
        || unsafe { HOOK.create9ex.wait().original_fn()(sdk, out) },
        out,
        wrap_d3d9,
    )
}

extern "system" fn hooked_create_device(
    this: *mut c_void,
    adapter: u32,
    device_type: D3DDEVTYPE,
    focus: HWND,
    behavior: u32,
    params: *mut D3DPRESENT_PARAMETERS,
    out: *mut *mut c_void,
) -> HRESULT {
    create_then_wrap(
        || unsafe {
            HOOK.create_device.wait().original_fn()(
                this,
                adapter,
                device_type,
                focus,
                behavior,
                params,
                out,
            )
        },
        out,
        wrap_device,
    )
}

extern "system" fn hooked_create_device_ex(
    this: *mut c_void,
    adapter: u32,
    device_type: D3DDEVTYPE,
    focus: HWND,
    behavior: u32,
    params: *mut D3DPRESENT_PARAMETERS,
    fullscreen: *mut D3DDISPLAYMODEEX,
    out: *mut *mut c_void,
) -> HRESULT {
    create_then_wrap(
        || unsafe {
            HOOK.create_device_ex.wait().original_fn()(
                this,
                adapter,
                device_type,
                focus,
                behavior,
                params,
                fullscreen,
                out,
            )
        },
        out,
        wrap_device,
    )
}

/// GetModuleHandle only. No `LoadLibrary`, no dummy device. Present attaches from the game device.
pub fn hook() {
    let Ok(module) = (unsafe { GetModuleHandleA(s!("d3d9.dll")) }) else {
        return;
    };
    if let Some(proc) = unsafe { GetProcAddress(module, s!("Direct3DCreate9")) } {
        let func: Create9Fn = unsafe { mem::transmute_copy(&proc) };
        attach(
            &HOOK.create9,
            func,
            hooked_direct3d_create9 as _,
            "Direct3DCreate9",
        );
    }
    if let Some(proc) = unsafe { GetProcAddress(module, s!("Direct3DCreate9Ex")) } {
        let func: Create9ExFn = unsafe { mem::transmute_copy(&proc) };
        attach(
            &HOOK.create9ex,
            func,
            hooked_direct3d_create9_ex as _,
            "Direct3DCreate9Ex",
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn create_then_wrap_always_invokes_original() {
        use super::create_then_wrap;
        use core::ffi::c_void;
        use windows::core::HRESULT;

        let mut original_calls = 0;
        let mut wrap_calls = 0;
        let hr = create_then_wrap(
            || {
                original_calls += 1;
                HRESULT(-1)
            },
            core::ptr::null_mut::<*mut c_void>(),
            |_| wrap_calls += 1,
        );
        assert_eq!(original_calls, 1);
        assert_eq!(wrap_calls, 0);
        assert!(!hr.is_ok());
    }

    #[test]
    fn create_then_wrap_success_null_device_invokes_original_without_device() {
        use super::create_then_wrap;
        use core::ffi::c_void;
        use windows::core::HRESULT;

        let mut original_calls = 0;
        let mut wrap_calls = 0;
        let mut out: *mut c_void = core::ptr::null_mut();
        let hr = create_then_wrap(
            || {
                original_calls += 1;
                HRESULT(0)
            },
            &mut out,
            |_| wrap_calls += 1,
        );
        assert_eq!(original_calls, 1);
        assert_eq!(wrap_calls, 0);
        assert!(hr.is_ok());
    }
}
