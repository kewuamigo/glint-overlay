//! Shared native FPS counter and SHM block used by injected metrics DLLs.

pub mod counter;
pub mod log;
pub mod shm;
pub mod timing;

pub use counter::now_ms;
pub use log::debug_log;
pub use shm::{read_metrics_for_pid, MetricsBlock, SharedMetrics};
pub use timing::{ema, fps_from_window, frametime_ema_step, FrametimeEma, FpsWindow, FpsWindowTick};
