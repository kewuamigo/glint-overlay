use windows::Win32::Graphics::Direct3D12::{ID3D12CommandList, ID3D12CommandQueue};

#[tracing::instrument]
pub unsafe fn original_execute_command_lists(
    queue: &ID3D12CommandQueue,
    command_lists: &[Option<ID3D12CommandList>],
) {
    unsafe { queue.ExecuteCommandLists(command_lists) }
}
