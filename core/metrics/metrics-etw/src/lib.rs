//! ETW consumer for Microsoft-Windows-DxgKrnl flip/present events.

mod cleanup;
mod consumer;
mod fps;

pub use consumer::EtwMetricsConsumer;
pub use fps::{
    EventCounters, FrameGenKind, HardwareFpsTracker, HookedNativeSample, MetricsSnapshot,
};
