//! Mantle present-site hooks (Steam `sub_1800B4120` / D10).
//!
//! Load-gate: `GetModuleHandleA("mantle32.dll")` then `"mantle64.dll"`.
//! Never `LoadLibrary`. Missing DLL is success — overlay still works.
//!
//! Queue-present: mailbox overlay for that HWND if a backend exists, then
//! always original. Create/destroy/object/image call original only.
//! No Mantle GPU compositor. No `grCmd*` recording.

use std::mem;

use glint_overlay_hook::DetourHook;
use once_cell::sync::OnceCell;
use tracing::{info, warn};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1},
        System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
    },
    core::{PCSTR, s},
};

use crate::{
    backend::Backends,
    compositor,
    event_sink::OverlayEventSink,
    util::{after_original_present, get_client_size},
};

type GrResult = u32;

type CreateDeviceFn =
    unsafe extern "system" fn(usize, *const core::ffi::c_void, *mut usize) -> GrResult;
type DestroyDeviceFn = unsafe extern "system" fn(usize) -> GrResult;
type DestroyObjectFn = unsafe extern "system" fn(usize) -> GrResult;
type CreatePresentableImageFn =
    unsafe extern "system" fn(usize, *const core::ffi::c_void, *mut usize, *mut usize) -> GrResult;
type QueuePresentFn = unsafe extern "system" fn(usize, *const core::ffi::c_void) -> GrResult;

struct Hooks {
    create_device: Option<DetourHook<CreateDeviceFn>>,
    destroy_device: Option<DetourHook<DestroyDeviceFn>>,
    destroy_object: Option<DetourHook<DestroyObjectFn>>,
    create_image: Option<DetourHook<CreatePresentableImageFn>>,
    queue_present: Option<DetourHook<QueuePresentFn>>,
}

static HOOKS: OnceCell<Hooks> = OnceCell::new();

/// Steam `sub_1800AB510` @ `0x1800ABC95`: mantle32 then mantle64. No LoadLibrary.
fn mantle_module() -> Option<HMODULE> {
    unsafe { GetModuleHandleA(s!("mantle32.dll")) }
        .ok()
        .or_else(|| unsafe { GetModuleHandleA(s!("mantle64.dll")) }.ok())
}

/// `GR_WSI_WIN_PRESENT_INFO.hWindowDest` is the first field (`*(HWND *)a2`).
fn hwnd_from_present_info(info: *const core::ffi::c_void) -> HWND {
    if info.is_null() {
        return HWND(core::ptr::null_mut());
    }
    HWND(unsafe { *(info as *const *mut core::ffi::c_void) })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlitPlan {
    /// Null HWND or overlay disconnected — do not wait.
    SkipNoWait,
    /// Known DXGI swapchain — `dxgi::draw_overlay` (mailbox blit).
    DrawKnownSwapchain,
    /// Init backend + client size + plane; no Mantle GPU compositor.
    InitNoGpu,
}

fn blit_plan(hwnd_null: bool, connected: bool, swapchain_known: bool) -> BlitPlan {
    if hwnd_null || !connected {
        return BlitPlan::SkipNoWait;
    }
    if swapchain_known {
        BlitPlan::DrawKnownSwapchain
    } else {
        BlitPlan::InitNoGpu
    }
}

fn default_adapter() -> Option<IDXGIAdapter> {
    let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok()?;
    unsafe { factory.EnumAdapters(0) }.ok()
}

/// Steam `sub_1800B8E60`: lookup renderer, blit `sub_1800BFCB0`, always original.
/// DXGI mailbox if a swapchain is already known for this HWND. No Mantle compositor.
fn blit_mailbox(hwnd: HWND) {
    if blit_plan(hwnd.0.is_null(), OverlayEventSink::connected(), false) == BlitPlan::SkipNoWait {
        return;
    }
    let id = hwnd.0 as u32;
    if let Ok(size) = get_client_size(hwnd) {
        let _ = Backends::with_backend(id, |backend| {
            backend.render.lock().window_size = size;
        });
    }
    if super::dx::draw_overlay_for_hwnd(hwnd) {
        return;
    }
    let _ = Backends::with_or_init_backend(id, default_adapter, |backend| {
        if let Ok(size) = get_client_size(hwnd) {
            backend.render.lock().window_size = size;
        }
        compositor::ensure_overlay_plane();
    });
}

fn attach<F: Copy + std::fmt::Debug>(
    module: HMODULE,
    name: PCSTR,
    detour: F,
    label: &str,
) -> Option<DetourHook<F>> {
    let proc = unsafe { GetProcAddress(module, name) }?;
    let func: F = unsafe { mem::transmute_copy(&proc) };
    match unsafe { DetourHook::attach(func, detour) } {
        Ok(h) => {
            info!("hooked {label}");
            Some(h)
        }
        Err(err) => {
            warn!("Failed hooking {label}: {err:?}");
            None
        }
    }
}

unsafe extern "system" fn hooked_gr_create_device(
    gpu: usize,
    create_info: *const core::ffi::c_void,
    device: *mut usize,
) -> GrResult {
    let orig = HOOKS.wait().create_device.as_ref().unwrap().original_fn();
    unsafe { orig(gpu, create_info, device) }
}

unsafe extern "system" fn hooked_gr_destroy_device(device: usize) -> GrResult {
    let orig = HOOKS.wait().destroy_device.as_ref().unwrap().original_fn();
    unsafe { orig(device) }
}

unsafe extern "system" fn hooked_gr_destroy_object(object: usize) -> GrResult {
    let orig = HOOKS.wait().destroy_object.as_ref().unwrap().original_fn();
    unsafe { orig(object) }
}

unsafe extern "system" fn hooked_gr_wsi_win_create_presentable_image(
    device: usize,
    create_info: *const core::ffi::c_void,
    image: *mut usize,
    mem: *mut usize,
) -> GrResult {
    let orig = HOOKS.wait().create_image.as_ref().unwrap().original_fn();
    unsafe { orig(device, create_info, image, mem) }
}

unsafe extern "system" fn hooked_gr_wsi_win_queue_present(
    queue: usize,
    present_info: *const core::ffi::c_void,
) -> GrResult {
    blit_mailbox(hwnd_from_present_info(present_info));
    after_original_present(|| {
        let orig = HOOKS.wait().queue_present.as_ref().unwrap().original_fn();
        unsafe { orig(queue, present_info) }
    })
}

#[tracing::instrument]
pub(super) fn hook() {
    super::late::attach_when_loaded(mantle_module(), &HOOKS, |module| {
        info!("Game is using mantle[32|64].dll... hooking.");
        Hooks {
            create_device: attach(
                module,
                s!("grCreateDevice"),
                hooked_gr_create_device,
                "grCreateDevice",
            ),
            destroy_device: attach(
                module,
                s!("grDestroyDevice"),
                hooked_gr_destroy_device,
                "grDestroyDevice",
            ),
            destroy_object: attach(
                module,
                s!("grDestroyObject"),
                hooked_gr_destroy_object,
                "grDestroyObject",
            ),
            create_image: attach(
                module,
                s!("grWsiWinCreatePresentableImage"),
                hooked_gr_wsi_win_create_presentable_image,
                "grWsiWinCreatePresentableImage",
            ),
            queue_present: attach(
                module,
                s!("grWsiWinQueuePresent"),
                hooked_gr_wsi_win_queue_present,
                "grWsiWinQueuePresent",
            ),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_without_mantle_is_ok_and_requires_no_detours() {
        assert!(
            mantle_module().is_none(),
            "CI must not LoadLibrary mantle; skip = success"
        );
        hook();
        assert!(
            HOOKS.get().is_none(),
            "miss must not latch empty (late LoadLibrary re-calls hook)"
        );
    }

    #[test]
    fn present_info_hwnd_is_first_field() {
        let hwnd = 0x1234usize as *mut core::ffi::c_void;
        let info = hwnd;
        assert_eq!(
            hwnd_from_present_info((&info as *const *mut core::ffi::c_void).cast()).0,
            hwnd
        );
        assert!(hwnd_from_present_info(core::ptr::null()).0.is_null());
    }

    #[test]
    fn null_or_disconnected_skips_without_wait() {
        assert_eq!(blit_plan(true, true, true), BlitPlan::SkipNoWait);
        assert_eq!(blit_plan(false, false, true), BlitPlan::SkipNoWait);
        assert_eq!(blit_plan(true, false, false), BlitPlan::SkipNoWait);
    }

    #[test]
    fn blit_is_not_empty_closure() {
        assert_eq!(blit_plan(false, true, true), BlitPlan::DrawKnownSwapchain);
        assert_eq!(blit_plan(false, true, false), BlitPlan::InitNoGpu);
        assert_ne!(BlitPlan::DrawKnownSwapchain, BlitPlan::InitNoGpu);
        assert_ne!(BlitPlan::DrawKnownSwapchain, BlitPlan::SkipNoWait);
    }
}
