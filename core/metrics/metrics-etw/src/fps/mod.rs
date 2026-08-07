//! ETW FPS tracking: rolling windows, frame-gen heuristics, and JSON snapshots.

mod frame_gen;
mod snapshot;
mod tracker;

pub use snapshot::{FrameGenKind, HookedNativeSample, MetricsSnapshot};
pub use tracker::{EventCounters, HardwareFpsTracker};
