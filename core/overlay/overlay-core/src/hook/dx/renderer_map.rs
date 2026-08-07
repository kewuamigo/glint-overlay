//! Shared [`IntDashMap`] get-or-insert helper for per-swapchain/device renderers.

use dashmap::Entry;

use crate::types::IntDashMap;

/// Look up `key` in `map`, initialize on first use, then run `f` on the value.
pub fn with_map_entry<K, V, R, E>(
    map: &IntDashMap<K, V>,
    key: K,
    init: impl FnOnce() -> Result<V, E>,
    on_created: Option<impl FnOnce()>,
    f: impl FnOnce(&mut V) -> Result<R, E>,
) -> Result<R, E>
where
    K: Eq + std::hash::Hash + nohash_hasher::IsEnabled,
{
    let mut slot = match map.entry(key) {
        Entry::Occupied(entry) => entry.into_ref(),
        Entry::Vacant(entry) => {
            let value = init()?;
            let inserted = entry.insert(value);
            if let Some(on_created) = on_created {
                on_created();
            }
            inserted
        }
    };

    f(&mut slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn with_map_entry_initializes_once_and_calls_on_created() {
        let map = IntDashMap::default();
        let created = AtomicU32::new(0);

        let v1 = with_map_entry(
            &map,
            1usize,
            || Ok::<u32, ()>(10u32),
            Some(|| {
                created.fetch_add(1, Ordering::SeqCst);
            }),
            |v| {
                *v += 1;
                Ok(*v)
            },
        )
        .unwrap();
        assert_eq!(v1, 11);
        assert_eq!(created.load(Ordering::SeqCst), 1);

        let v2 = with_map_entry(
            &map,
            1usize,
            || Ok::<u32, ()>(0u32),
            Some(|| {
                created.fetch_add(1, Ordering::SeqCst);
            }),
            |v| Ok::<u32, ()>(*v),
        )
        .unwrap();
        assert_eq!(v2, 11);
        assert_eq!(created.load(Ordering::SeqCst), 1);
    }
}
