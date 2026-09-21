mod dx10;
mod dx11;
mod dx12;
mod dx9;
mod dxgi;
mod render_gate;
mod renderer_map;

pub use dx12::original_execute_command_lists;

use windows::Win32::Foundation::HWND;

pub(crate) fn draw_overlay_for_hwnd(hwnd: HWND) -> bool {
    dxgi::draw_overlay_for_hwnd(hwnd)
}

fn cached_rtv_get_or_insert<T>(slot: &mut Option<T>, create: impl FnOnce() -> Option<T>) -> Option<&T> {
    if slot.is_none() {
        *slot = create();
    }
    slot.as_ref()
}

fn cached_rtv_invalidate<T>(slot: &mut Option<T>) {
    *slot = None;
}

#[cfg(test)]
mod cached_rtv_tests {
    use super::{cached_rtv_get_or_insert, cached_rtv_invalidate};

    #[test]
    fn cached_rtv_store_get_clear_fake_key() {
        let mut slot = None;
        assert_eq!(cached_rtv_get_or_insert(&mut slot, || Some(0xAAu32)), Some(&0xAA));
        let mut created = false;
        assert_eq!(
            cached_rtv_get_or_insert(&mut slot, || {
                created = true;
                Some(0xBB)
            }),
            Some(&0xAA)
        );
        assert!(!created);
        cached_rtv_invalidate(&mut slot);
        assert!(slot.is_none());
        assert_eq!(cached_rtv_get_or_insert(&mut slot, || Some(0xCC)), Some(&0xCC));
    }
}

#[tracing::instrument]
pub fn hook() {
    dx12::hook();
    dxgi::hook();
    dx9::hook();
}
