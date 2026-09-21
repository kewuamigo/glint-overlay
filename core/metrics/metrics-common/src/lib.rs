//! Shared native FPS counter and SHM block used by injected metrics DLLs.

pub mod counter;
pub mod log;
pub mod shm;
pub mod timing;

pub use counter::now_ms;
pub use log::debug_log;
pub use shm::{
    FG_KIND_DLSS, FG_KIND_FSR, FG_KIND_NONE, MetricsBlock, SharedMetrics, frame_gen_active,
    frame_gen_kind_label, read_metrics_for_pid, snapshot_fps,
};
pub use timing::{
    FpsWindow, FpsWindowTick, FrametimeEma, ema, fps_from_window, frametime_ema_step,
};
