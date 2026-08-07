//! Shared FPS / frametime math used by injected DLLs and ETW metrics.

pub const EMA_ALPHA: f64 = 0.18;
pub const MAX_DELTA_MS: f64 = 500.0;
pub const FPS_WINDOW_MS: u64 = 500;

/// Exponential moving average step.
#[inline]
pub fn ema(current: f64, sample: f64, alpha: f64) -> f64 {
    if sample <= 0.0 {
        return current;
    }
    if current <= 0.0 {
        sample
    } else {
        current * (1.0 - alpha) + sample * alpha
    }
}

/// Update frametime EMA from a frame-to-frame delta in milliseconds.
#[inline]
pub fn frametime_ema_step(current_ema: f64, dt_ms: f64) -> f64 {
    if dt_ms <= 0.0 || dt_ms >= MAX_DELTA_MS {
        return current_ema;
    }
    ema(current_ema, dt_ms, EMA_ALPHA)
}

/// Compute FPS from frames counted inside a rolling window.
#[inline]
pub fn fps_from_window(frame_count: u64, elapsed_ms: u64) -> f64 {
    if frame_count == 0 || elapsed_ms == 0 {
        0.0
    } else {
        frame_count as f64 / (elapsed_ms as f64 / 1000.0)
    }
}

/// Rolling exponential moving average of per-frame deltas.
#[derive(Debug, Default, Clone, Copy)]
pub struct FrametimeEma {
    ema_ms: f64,
}

impl FrametimeEma {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_delta_ms(&mut self, dt_ms: f64) -> f64 {
        self.ema_ms = frametime_ema_step(self.ema_ms, dt_ms);
        self.ema_ms
    }

    pub fn value(self) -> f64 {
        self.ema_ms
    }
}

/// Result of advancing a fixed-duration FPS counting window.
#[derive(Debug, Clone, Copy)]
pub struct FpsWindowTick {
    pub fps: f64,
    pub window_reset: bool,
}

/// Count frames over [`FPS_WINDOW_MS`] and return the computed FPS when the window elapses.
#[derive(Debug, Clone, Copy)]
pub struct FpsWindow {
    frame_count: u64,
    elapsed_ms: u64,
    current_fps: f64,
}

impl FpsWindow {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            elapsed_ms: 0,
            current_fps: 0.0,
        }
    }

    pub fn on_frame(&mut self, elapsed_ms: u64) -> FpsWindowTick {
        self.frame_count += 1;
        if elapsed_ms >= FPS_WINDOW_MS {
            let fps = fps_from_window(self.frame_count, elapsed_ms);
            self.current_fps = fps;
            self.frame_count = 0;
            self.elapsed_ms = 0;
            FpsWindowTick {
                fps,
                window_reset: true,
            }
        } else {
            self.elapsed_ms = elapsed_ms;
            FpsWindowTick {
                fps: self.current_fps,
                window_reset: false,
            }
        }
    }

    pub fn current_fps(self) -> f64 {
        self.current_fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ema_starts_at_first_sample() {
        assert_eq!(ema(0.0, 16.0, EMA_ALPHA), 16.0);
    }

    #[test]
    fn fps_from_window_computes() {
        assert!((fps_from_window(60, 1000) - 60.0).abs() < f64::EPSILON);
    }
}
