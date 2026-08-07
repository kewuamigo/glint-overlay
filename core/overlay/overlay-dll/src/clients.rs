//! Multi-client event sink registry for concurrent named-pipe connections.

#![allow(dead_code)] // ClientRegistry unit-tested; production fan-out/owners also on OverlayEventSink

use glint_overlay_event::OverlayEvent;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// True when no live IPC sessions remain — sole gate for `cleanup_backends`.
pub(crate) fn should_cleanup_backends(remaining_sessions: usize) -> bool {
    remaining_sessions == 0
}

/// Counts accepted sessions (`listen_loop` enter → `serve_connection` leave).
pub(crate) struct SessionCounter {
    count: AtomicUsize,
}

impl SessionCounter {
    pub const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
        }
    }

    pub fn enter(&self) {
        self.count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement and return the remaining session count.
    pub fn leave(&self) -> usize {
        self.count.fetch_sub(1, Ordering::AcqRel) - 1
    }
}

/// Opaque handle for a registered client sink.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClientId(u64);

impl ClientId {
    /// Wrap a raw sink id (same value as [`glint_overlay_core::event_sink::OverlayEventSink::add`]).
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

type SinkFn = Arc<dyn Fn(OverlayEvent) + Send + Sync>;

/// Registry of connected IPC clients that receive overlay event broadcasts.
pub struct ClientRegistry {
    inner: Mutex<Inner>,
}

struct Inner {
    next: u64,
    sinks: Vec<(ClientId, SinkFn)>,
    /// First successful binder wins per layer.
    layers: HashMap<u32, ClientId>,
}

impl ClientRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                next: 1,
                sinks: Vec::new(),
                layers: HashMap::new(),
            }),
        }
    }

    /// Register a sink; returned id is used to unregister later.
    pub fn register(&self, sink: impl Fn(OverlayEvent) + Send + Sync + 'static) -> ClientId {
        let mut inner = self.inner.lock();
        let id = ClientId(inner.next);
        inner.next = inner.next.saturating_add(1);
        inner.sinks.push((id, Arc::new(sink)));
        id
    }

    /// Remove a previously registered sink and clear its layer bindings.
    pub fn unregister(&self, id: ClientId) {
        let mut inner = self.inner.lock();
        inner.sinks.retain(|(sid, _)| *sid != id);
        inner.layers.retain(|_, owner| *owner != id);
    }

    /// True if at least one client sink is registered.
    pub fn has_clients(&self) -> bool {
        !self.inner.lock().sinks.is_empty()
    }

    /// Bind `layer` to `client` if no owner yet (first binder wins).
    pub fn bind_layer(&self, client: ClientId, layer: u32) {
        self.inner.lock().layers.entry(layer).or_insert(client);
    }

    /// Remove all layer bindings owned by `client`.
    pub fn unbind_client_layers(&self, client: ClientId) {
        self.inner.lock().layers.retain(|_, owner| *owner != client);
    }

    /// Owner of `layer`, if bound.
    pub fn owner_of_layer(&self, layer: u32) -> Option<ClientId> {
        self.inner.lock().layers.get(&layer).copied()
    }

    /// Deliver `event` to a single client sink.
    pub fn emit_to(&self, client: ClientId, event: OverlayEvent) {
        let sink = self
            .inner
            .lock()
            .sinks
            .iter()
            .find(|(id, _)| *id == client)
            .map(|(_, s)| s.clone());
        if let Some(sink) = sink {
            sink(event);
        }
    }

    /// Deliver `event` to every registered sink.
    pub fn broadcast(&self, event: OverlayEvent) {
        let sinks: Vec<_> = self
            .inner
            .lock()
            .sinks
            .iter()
            .map(|(_, s)| s.clone())
            .collect();
        for s in sinks {
            s(event.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_overlay_event::{OverlayEvent, WindowEvent};
    use std::sync::{Arc, Mutex};

    #[test]
    fn two_clients_both_receive_broadcast() {
        let reg = ClientRegistry::new();
        let a = Arc::new(Mutex::new(Vec::new()));
        let b = Arc::new(Mutex::new(Vec::new()));
        let id_a = reg.register({
            let a = a.clone();
            move |e| a.lock().unwrap().push(format!("{e:?}"))
        });
        let id_b = reg.register({
            let b = b.clone();
            move |e| b.lock().unwrap().push(format!("{e:?}"))
        });
        reg.broadcast(OverlayEvent::Window {
            id: 1,
            event: WindowEvent::Destroyed,
        });
        assert_eq!(a.lock().unwrap().len(), 1);
        assert_eq!(b.lock().unwrap().len(), 1);
        reg.unregister(id_a);
        reg.broadcast(OverlayEvent::Window {
            id: 1,
            event: WindowEvent::Destroyed,
        });
        assert_eq!(a.lock().unwrap().len(), 1);
        assert_eq!(b.lock().unwrap().len(), 2);
        let _ = id_b;
    }

    #[test]
    fn empty_registry_is_disconnected() {
        let reg = ClientRegistry::new();
        assert!(!reg.has_clients());
    }

    #[test]
    fn cleanup_only_when_last_session_leaves() {
        let sessions = SessionCounter::new();
        sessions.enter(); // A
        sessions.enter(); // B
        assert!(!should_cleanup_backends(sessions.leave())); // A leaves, B remains
        assert!(should_cleanup_backends(sessions.leave())); // B leaves → cleanup
    }

    #[test]
    fn topmost_layer_owner_wins() {
        let reg = ClientRegistry::new();
        let a = reg.register(|_| {});
        let b = reg.register(|_| {});
        reg.bind_layer(a, 0);
        reg.bind_layer(b, 1);
        assert_eq!(reg.owner_of_layer(1), Some(b));
        reg.unbind_client_layers(b);
        assert_eq!(reg.owner_of_layer(1), None);
    }
}
