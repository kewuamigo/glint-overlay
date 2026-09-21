use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    Graphics::Direct3D12::*,
    System::Threading::{CreateEventW, WaitForSingleObject},
};

/// Previous Present's Signal is still in flight on the game queue.
pub(crate) fn gpu_busy(completed: u64, fence_val: u64) -> bool {
    completed < fence_val
}

/// Steam waits (`WaitForSingleObject` infinite) then draws. Skip blanks the
/// overlay for that Present — FPS-scaled strobe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingAction {
    Draw,
    Wait,
}

pub(crate) fn pending_fence_action(pending: bool) -> PendingAction {
    if pending {
        PendingAction::Wait
    } else {
        PendingAction::Draw
    }
}

#[derive(Debug)]
pub struct RendererFence {
    fence: ID3D12Fence,
    fence_val: u64,
    event: HANDLE,
}

impl RendererFence {
    pub fn new(device: &ID3D12Device) -> anyhow::Result<Self> {
        Ok(Self {
            fence: unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? },
            fence_val: 0,
            event: unsafe { CreateEventW(None, false, false, None)? },
        })
    }

    pub fn register(&mut self, queue: &ID3D12CommandQueue) -> anyhow::Result<()> {
        self.fence_val += 1;
        unsafe {
            queue.Signal(&self.fence, self.fence_val)?;
        }

        Ok(())
    }

    pub fn is_pending(&self) -> bool {
        gpu_busy(unsafe { self.fence.GetCompletedValue() }, self.fence_val)
    }

    pub fn wait_pending(&self) -> anyhow::Result<()> {
        if !self.is_pending() {
            return Ok(());
        }
        match pending_fence_action(true) {
            PendingAction::Draw => Ok(()),
            PendingAction::Wait => self.block_until_signaled(),
        }
    }

    fn block_until_signaled(&self) -> anyhow::Result<()> {
        unsafe {
            self.fence
                .SetEventOnCompletion(self.fence_val, self.event)?;
            let wr = WaitForSingleObject(self.event, u32::MAX);
            if wr != WAIT_OBJECT_0 {
                anyhow::bail!("dx12 overlay fence wait failed: {wr:?}");
            }
        }
        Ok(())
    }

    /// Extra AddRef so a later field `Release` leaves the fence alive.
    pub fn leak(&self) {
        std::mem::forget(self.fence.clone());
    }
}

impl Drop for RendererFence {
    fn drop(&mut self) {
        if self.is_pending() || self.event.is_invalid() {
            return;
        }
        unsafe {
            _ = CloseHandle(self.event);
        }
    }
}

unsafe impl Send for RendererFence {}

#[cfg(test)]
mod tests {
    use super::{PendingAction, gpu_busy, pending_fence_action};

    #[test]
    fn previous_frame_complete_does_not_skip() {
        assert!(!gpu_busy(1, 1));
        assert!(!gpu_busy(2, 1));
    }

    #[test]
    fn previous_frame_pending_is_busy() {
        assert!(gpu_busy(0, 1));
    }

    #[test]
    fn pending_fence_waits_not_skips() {
        assert_eq!(pending_fence_action(false), PendingAction::Draw);
        assert_eq!(pending_fence_action(true), PendingAction::Wait);
    }

    #[test]
    fn one_signal_after_all_layers_does_not_busy_later_layers() {
        let mut fence_val = 1u64;
        let completed = 1u64;
        assert!(!gpu_busy(completed, fence_val));
        assert!(!gpu_busy(completed, fence_val));
        fence_val += 1;
        assert!(gpu_busy(completed, fence_val));
    }

    #[test]
    fn per_layer_signal_would_skip_remaining_layers() {
        let completed = 1u64;
        let after_layer0 = 2u64;
        assert!(gpu_busy(completed, after_layer0));
    }

    #[test]
    fn drop_leaks_only_while_gpu_busy() {
        assert!(gpu_busy(0, 1));
        assert!(!gpu_busy(1, 1));
    }
}
