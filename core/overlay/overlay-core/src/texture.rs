use core::num::NonZeroU32;

use glint_overlay_common::request::UpdateSharedHandle;

#[derive(Debug)]
pub enum OverlayTextureState<T> {
    None,
    Handle(NonZeroU32),
    /// Last opened shared tex (Steam cached `hTexture`). Same handle keeps this.
    Created(NonZeroU32, T),
    /// New handle pending; keep drawing the old `hTexture` until reopen works.
    CreatedPending(NonZeroU32, T, NonZeroU32),
}

impl<T> OverlayTextureState<T> {
    pub const fn new() -> Self {
        Self::None
    }

    pub fn update(&mut self, shared: UpdateSharedHandle) {
        match shared.handle {
            Some(handle) => match self {
                Self::Created(current, _) if *current == handle => {}
                Self::CreatedPending(current, _, pending)
                    if *current == handle || *pending == handle => {}
                Self::Created(current, _) => {
                    let current = *current;
                    if let Self::Created(_, tex) = std::mem::replace(self, Self::None) {
                        *self = Self::CreatedPending(current, tex, handle);
                    }
                }
                Self::CreatedPending(current, _, _) => {
                    let current = *current;
                    if let Self::CreatedPending(_, tex, _) = std::mem::replace(self, Self::None) {
                        *self = Self::CreatedPending(current, tex, handle);
                    }
                }
                _ => *self = Self::Handle(handle),
            },
            None => *self = Self::None,
        }
    }

    pub fn get_or_create(
        &mut self,
        f: impl FnOnce(NonZeroU32) -> anyhow::Result<Option<T>>,
    ) -> anyhow::Result<Option<&mut T>> {
        match self {
            Self::None => Ok(None),
            Self::Created(_, created) => Ok(Some(created)),
            Self::Handle(handle) => {
                let handle = *handle;
                if let Some(created) = f(handle)? {
                    *self = Self::Created(handle, created);
                    let Self::Created(_, created) = self else {
                        unreachable!();
                    };
                    Ok(Some(created))
                } else {
                    *self = Self::None;
                    Ok(None)
                }
            }
            Self::CreatedPending(_, _, pending) => {
                let pending = *pending;
                if let Some(created) = f(pending)? {
                    *self = Self::Created(pending, created);
                    let Self::Created(_, created) = self else {
                        unreachable!();
                    };
                    Ok(Some(created))
                } else {
                    let Self::CreatedPending(_, created, _) = self else {
                        unreachable!();
                    };
                    Ok(Some(created))
                }
            }
        }
    }
}

impl<T> Default for OverlayTextureState<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::OverlayTextureState;
    use glint_overlay_common::request::UpdateSharedHandle;
    use std::num::NonZeroU32;

    fn nz(v: u32) -> NonZeroU32 {
        NonZeroU32::new(v).unwrap()
    }

    fn handle(v: u32) -> UpdateSharedHandle {
        UpdateSharedHandle {
            handle: Some(nz(v)),
        }
    }

    #[test]
    fn same_handle_does_not_reopen_htexture() {
        let mut s = OverlayTextureState::None;
        s.update(handle(7));
        s.get_or_create(|_| Ok(Some(1))).unwrap();
        s.update(handle(7));
        let mut creates = 0;
        let got = s
            .get_or_create(|_| {
                creates += 1;
                Ok(Some(99))
            })
            .unwrap();
        assert_eq!(got, Some(&mut 1));
        assert_eq!(creates, 0);
    }

    #[test]
    fn new_handle_reopens() {
        let mut s = OverlayTextureState::None;
        s.update(handle(7));
        s.get_or_create(|_| Ok(Some(1))).unwrap();
        s.update(handle(8));
        let mut creates = 0;
        let got = s
            .get_or_create(|_| {
                creates += 1;
                Ok(Some(2))
            })
            .unwrap();
        assert_eq!(got, Some(&mut 2));
        assert_eq!(creates, 1);
    }

    #[test]
    fn new_handle_keeps_old_htexture_if_reopen_fails() {
        let mut s = OverlayTextureState::None;
        s.update(handle(7));
        s.get_or_create(|_| Ok(Some(1))).unwrap();
        s.update(handle(8));
        let got = s.get_or_create(|_| Ok(None)).unwrap();
        assert_eq!(got, Some(&mut 1));
    }

    #[test]
    fn none_handle_clears_layer() {
        let mut s = OverlayTextureState::None;
        s.update(handle(7));
        s.get_or_create(|_| Ok(Some(1))).unwrap();
        s.update(UpdateSharedHandle { handle: None });
        assert!(s.get_or_create(|_| Ok(Some(99))).unwrap().is_none());
    }
}
