use core::{ptr, slice};

use ash::vk::{self, Handle};
use glint_overlay_core::{
    MailboxSample, after_original_present,
    backend::{Backends, WindowBackend, render::Renderer},
    event_sink::OverlayEventSink,
    mailbox_sample, with_keyed_mutex_sampled,
};
use parking_lot::Mutex;
use tracing::{debug, error, trace};
use windows::Win32::{
    Foundation::LUID,
    Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory4},
};

use crate::{
    device::{
        DISPATCH_TABLE, DispatchTable, get_queue_data,
        swapchain::{SwapchainData, with_swapchain_data},
    },
    instance::physical_device::{get_physical_device_luid, get_physical_device_memory_properties},
    renderer::VulkanRenderer,
};

/// Layer `vkQueuePresentKHR` implementation
pub(super) extern "system" fn present(
    queue: vk::Queue,
    info: *const vk::PresentInfoKHR,
) -> vk::Result {
    trace!("vkQueuePresentKHR called");

    let Some(queue_data) = get_queue_data(queue) else {
        error!("vkQueuePresentKHR: missing queue data");
        return vk::Result::ERROR_UNKNOWN;
    };
    let Some(mut table) = DISPATCH_TABLE.get_mut(&queue_data.device.as_raw()) else {
        error!("vkQueuePresentKHR: missing dispatch table");
        return vk::Result::ERROR_UNKNOWN;
    };
    let Some(queue_present) = table.queue_present else {
        error!("vkQueuePresentKHR: missing queue_present");
        return vk::Result::ERROR_UNKNOWN;
    };

    if OverlayEventSink::connected() {
        let info = unsafe { &*info };
        let wait_semaphores = unsafe {
            slice::from_raw_parts(info.p_wait_semaphores, info.wait_semaphore_count as _)
        };
        let swapchains =
            unsafe { slice::from_raw_parts(info.p_swapchains, info.swapchain_count as _) };
        let indices =
            unsafe { slice::from_raw_parts(info.p_image_indices, info.swapchain_count as _) };

        for i in 0..info.swapchain_count as usize {
            blit_mailbox(
                &mut table,
                queue,
                queue_data.family_index,
                swapchains[i],
                indices[i],
                wait_semaphores,
            );
        }

        if !table.semaphore_buf.is_empty() {
            let present_info = vk::PresentInfoKHR::default()
                .swapchains(swapchains)
                .image_indices(indices)
                .wait_semaphores(&table.semaphore_buf);
            let res = after_original_present(|| unsafe { queue_present(queue, &present_info) });
            table.semaphore_buf.clear();
            clear_acquired_if_presented(info, res);
            return res;
        }
    }

    let res = after_original_present(|| unsafe { queue_present(queue, info) });
    clear_acquired_if_presented(info, res);
    res
}

#[derive(Clone, Copy)]
struct AcquiredMailbox {
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    index: u32,
}

/// Acquired, not-yet-presented swapchain image (from `vkAcquireNextImage*`).
static ACQUIRED: Mutex<Option<AcquiredMailbox>> = Mutex::new(None);

fn acquire_ok(res: vk::Result) -> bool {
    res == vk::Result::SUCCESS || res == vk::Result::SUBOPTIMAL_KHR
}

fn note_acquired(device: vk::Device, swapchain: vk::SwapchainKHR, index: u32) {
    *ACQUIRED.lock() = Some(AcquiredMailbox {
        device,
        swapchain,
        index,
    });
}

fn clear_if_presented(swapchain: vk::SwapchainKHR, index: u32) {
    let mut slot = ACQUIRED.lock();
    if matches!(*slot, Some(mb) if mb.swapchain == swapchain && mb.index == index) {
        *slot = None;
    }
}

fn clear_acquired_if_presented(info: *const vk::PresentInfoKHR, res: vk::Result) {
    if !acquire_ok(res) || info.is_null() {
        return;
    }
    let info = unsafe { &*info };
    if info.p_swapchains.is_null() || info.p_image_indices.is_null() {
        return;
    }
    let swapchains = unsafe { slice::from_raw_parts(info.p_swapchains, info.swapchain_count as _) };
    let indices = unsafe { slice::from_raw_parts(info.p_image_indices, info.swapchain_count as _) };
    for i in 0..info.swapchain_count as usize {
        clear_if_presented(swapchains[i], indices[i]);
    }
}

/// Layer `vkAcquireNextImageKHR` — store the live image the caller will present.
pub(super) extern "system" fn acquire_next_image(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    timeout: u64,
    semaphore: vk::Semaphore,
    fence: vk::Fence,
    index: *mut u32,
) -> vk::Result {
    let Some(table) = DISPATCH_TABLE.get(&device.as_raw()) else {
        return vk::Result::ERROR_UNKNOWN;
    };
    let res = unsafe {
        (table.swapchain_fn.acquire_next_image_khr)(
            device, swapchain, timeout, semaphore, fence, index,
        )
    };
    if acquire_ok(res) && !index.is_null() {
        note_acquired(device, swapchain, unsafe { *index });
    }
    res
}

/// Layer `vkAcquireNextImage2KHR` — same store as `vkAcquireNextImageKHR`.
pub(super) extern "system" fn acquire_next_image2(
    device: vk::Device,
    info: *const vk::AcquireNextImageInfoKHR<'_>,
    index: *mut u32,
) -> vk::Result {
    let Some(table) = DISPATCH_TABLE.get(&device.as_raw()) else {
        return vk::Result::ERROR_UNKNOWN;
    };
    let res = unsafe { (table.swapchain_fn.acquire_next_image2_khr)(device, info, index) };
    if acquire_ok(res) && !info.is_null() && !index.is_null() {
        note_acquired(device, unsafe { (*info).swapchain }, unsafe { *index });
    }
    res
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportBlit {
    SkippedNoAcquire,
    Entered { swapchain: u64, index: u32 },
}

#[cfg(test)]
static LAST_EXPORT_BLIT: Mutex<Option<ExportBlit>> = Mutex::new(None);

/// Steam a4 fn-ptr gate. Null → no draw. Non-null → blit the acquired image.
/// Caller presents — this MUST NOT call `vkQueuePresentKHR`.
pub fn steam_overlay_present(draw_gate: bool) {
    if !draw_gate {
        return;
    }
    let Some(mb) = *ACQUIRED.lock() else {
        #[cfg(test)]
        {
            *LAST_EXPORT_BLIT.lock() = Some(ExportBlit::SkippedNoAcquire);
        }
        return;
    };
    #[cfg(test)]
    {
        *LAST_EXPORT_BLIT.lock() = Some(ExportBlit::Entered {
            swapchain: mb.swapchain.as_raw(),
            index: mb.index,
        });
    }
    let Some(mut table) = DISPATCH_TABLE.get_mut(&mb.device.as_raw()) else {
        return;
    };
    let Some(&queue) = table.queues.first() else {
        return;
    };
    let family_index = get_queue_data(queue).map(|q| q.family_index).unwrap_or(0);
    blit_mailbox(&mut table, queue, family_index, mb.swapchain, mb.index, &[]);
    if !table.semaphore_buf.is_empty() {
        unsafe {
            _ = table.device.queue_wait_idle(queue);
        }
        table.semaphore_buf.clear();
    }
    after_original_present(|| {});
}

fn blit_mailbox(
    table: &mut DispatchTable,
    queue: vk::Queue,
    family_index: u32,
    swapchain: vk::SwapchainKHR,
    index: u32,
    wait_semaphores: &[vk::Semaphore],
) {
    _ = with_swapchain_data(swapchain, |data| {
        let physical_device = table.physical_device;
        if let Err(err) = Backends::with_or_init_backend(
            data.hwnd,
            || {
                let mut luid = LUID::default();
                unsafe {
                    ptr::copy_nonoverlapping::<[u8; 8]>(
                        &get_physical_device_luid(physical_device)?,
                        &mut luid as *mut _ as _,
                        1,
                    );
                }
                let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory4>() }.ok()?;

                unsafe { factory.EnumAdapterByLuid(luid).ok() }
            },
            |backend| {
                let semaphore = draw_overlay(
                    table,
                    swapchain,
                    index,
                    data,
                    queue,
                    family_index,
                    backend,
                    wait_semaphores,
                );

                if let Some(semaphore) = semaphore {
                    table.semaphore_buf.push(semaphore);
                }
            },
        ) {
            error!("Backends::with_or_init_backend failed. err: {err:?}");
        }
    });
}

/// Draw the overlay, create a semaphore chained to the provided wait semaphores, and return it.
#[allow(clippy::too_many_arguments)]
#[inline]
fn draw_overlay(
    table: &DispatchTable,
    swapchain: vk::SwapchainKHR,
    index: u32,
    data: &SwapchainData,
    queue: vk::Queue,
    queue_family_index: u32,
    backend: &WindowBackend,
    wait_semaphores: &[vk::Semaphore],
) -> Option<vk::Semaphore> {
    if backend.independent_active() {
        return None;
    }
    let render = &mut *backend.render.lock();
    match render.renderer {
        Some(Renderer::Vulkan) => {}
        Some(_) => {
            trace!("ignoring vulkan rendering");
            return None;
        }
        None => {
            debug!("Found vulkan window");
            render.renderer = Some(Renderer::Vulkan);
            // wait next swap for possible dxgi swapchain check
            return None;
        }
    };

    let mut renderer_slot = data.renderer.lock();
    let renderer = match renderer_slot.as_mut() {
        Some(renderer) => renderer,
        None => {
            debug!("initializing vulkan renderer");

            let mut image_count = 0;
            let mut images = Vec::<vk::Image>::new();
            let images_ok = unsafe {
                _ = (table.swapchain_fn.get_swapchain_images_khr)(
                    table.device.handle(),
                    swapchain,
                    &mut image_count,
                    0 as _,
                );
                images.resize(image_count as _, vk::Image::null());

                (table.swapchain_fn.get_swapchain_images_khr)(
                    table.device.handle(),
                    swapchain,
                    &mut image_count,
                    images.as_mut_ptr(),
                )
                .result()
            };
            if let Err(err) = images_ok {
                error!("failed to get swapchain images. err: {err:?}");
                return None;
            }

            match VulkanRenderer::new(
                table.device.clone(),
                queue_family_index,
                data.image_size,
                data.format,
                &images,
            ) {
                Ok(created) => renderer_slot.insert(created),
                Err(err) => {
                    error!("renderer creation failed. err: {err:?}");
                    return None;
                }
            }
        }
    };

    if render.surface.invalidate_update() {
        let Some(props) = get_physical_device_memory_properties(table.physical_device) else {
            error!("failed to get physical device memory properties");
            return None;
        };

        if let Err(err) = renderer.update_texture(
            render.surface.get().map(|surface| surface.texture()),
            &props,
        ) {
            error!("failed to update vulkan texture. err: {err:?}");
            return None;
        }
    }
    let surface = render.surface.get()?;

    let res = with_keyed_mutex_sampled(surface.mutex(), |lock_held| {
        match mailbox_sample(lock_held, false) {
            MailboxSample::Live => renderer.draw(
                queue,
                wait_semaphores,
                index,
                render.position,
                surface.size(),
                render.window_size,
            ),
            MailboxSample::Skip | MailboxSample::Cache => Ok(None),
        }
    });
    trace!("vulkan render: {:?}", res);
    match res {
        Ok(Some(inner)) => inner.ok().flatten(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ACQUIRED, ExportBlit, LAST_EXPORT_BLIT, acquire_next_image, acquire_next_image2,
        clear_if_presented, note_acquired, present, steam_overlay_present,
    };
    use ash::vk::{self, Handle};
    use parking_lot::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_export() {
        *ACQUIRED.lock() = None;
        *LAST_EXPORT_BLIT.lock() = None;
    }

    fn test_swapchain(raw: u64) -> vk::SwapchainKHR {
        vk::SwapchainKHR::from_raw(raw)
    }

    fn test_device(raw: u64) -> vk::Device {
        vk::Device::from_raw(raw)
    }

    #[test]
    fn queue_present_missing_queue_does_not_panic() {
        let info = vk::PresentInfoKHR::default();
        let result = present(vk::Queue::null(), &info);
        assert_eq!(result, vk::Result::ERROR_UNKNOWN);
    }

    #[test]
    fn steam_export_null_gate_skips_blit() {
        let _g = TEST_LOCK.lock();
        reset_export();
        note_acquired(test_device(1), test_swapchain(0x11), 2);
        steam_overlay_present(false);
        assert_eq!(*LAST_EXPORT_BLIT.lock(), None);
    }

    #[test]
    fn steam_export_non_null_skips_when_nothing_acquired() {
        let _g = TEST_LOCK.lock();
        reset_export();
        steam_overlay_present(true);
        assert_eq!(*LAST_EXPORT_BLIT.lock(), Some(ExportBlit::SkippedNoAcquire));
    }

    #[test]
    fn steam_export_non_null_enters_blit_with_acquired_mailbox() {
        let _g = TEST_LOCK.lock();
        reset_export();
        note_acquired(test_device(1), test_swapchain(0x11), 2);
        steam_overlay_present(true);
        assert_eq!(
            *LAST_EXPORT_BLIT.lock(),
            Some(ExportBlit::Entered {
                swapchain: 0x11,
                index: 2,
            })
        );
    }

    #[test]
    fn present_success_clears_acquired_so_later_export_skips() {
        let _g = TEST_LOCK.lock();
        reset_export();
        note_acquired(test_device(1), test_swapchain(0x11), 2);
        clear_if_presented(test_swapchain(0x11), 2);
        steam_overlay_present(true);
        assert_eq!(*LAST_EXPORT_BLIT.lock(), Some(ExportBlit::SkippedNoAcquire));
    }

    #[test]
    fn acquire_hooks_missing_device_do_not_panic() {
        let mut index = 7u32;
        let res = acquire_next_image(
            vk::Device::null(),
            vk::SwapchainKHR::null(),
            0,
            vk::Semaphore::null(),
            vk::Fence::null(),
            &mut index,
        );
        assert_eq!(res, vk::Result::ERROR_UNKNOWN);
        assert_eq!(index, 7);

        let info = vk::AcquireNextImageInfoKHR::default();
        let res = acquire_next_image2(vk::Device::null(), &info, &mut index);
        assert_eq!(res, vk::Result::ERROR_UNKNOWN);
    }
}
