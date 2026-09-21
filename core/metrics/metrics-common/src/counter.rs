use std::sync::atomic::{AtomicU64, Ordering};

use crate::timing::{FPS_WINDOW_MS, FpsWindow as FpsWindowState, FrametimeEma};

pub static NATIVE_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

pub struct FpsWindow {
    inner: FpsWindowState,
    window_start_ms: u64,
}

pub struct FrametimeTracker {
    inner: FrametimeEma,
    last_ms: u64,
}

impl FrametimeTracker {
    pub fn new() -> Self {
        Self {
            inner: FrametimeEma::new(),
            last_ms: 0,
        }
    }

    pub fn on_frame(&mut self) -> f32 {
        let now = now_ms();
        if self.last_ms > 0 {
            let dt = now.saturating_sub(self.last_ms);
            self.inner.on_delta_ms(dt as f64);
        }
        self.last_ms = now;
        self.inner.value() as f32
    }
}

impl FpsWindow {
    pub const WINDOW_MS: u64 = FPS_WINDOW_MS;

    pub fn new() -> Self {
        Self {
            inner: FpsWindowState::new(),
            window_start_ms: now_ms(),
        }
    }

    pub fn increment(&mut self) {
        let now = now_ms();
        let elapsed = now.saturating_sub(self.window_start_ms);
        let tick = self.inner.on_frame(elapsed);
        if tick.window_reset {
            self.window_start_ms = now;
        }
    }

    pub fn fps(&self) -> f64 {
        self.inner.current_fps()
    }
}

pub fn now_ms() -> u64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    unsafe {
        let mut counter = 0i64;
        let mut freq = 0i64;
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut counter);
        ((counter as u128) * 1000 / freq.max(1) as u128) as u64
    }
}

pub fn load_native_count() -> u64 {
    NATIVE_FRAME_COUNT.load(Ordering::Relaxed)
}
