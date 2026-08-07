use glint_metrics_common::timing::ema;

const MIN_PRESENT_FOR_FRAMEGEN: f64 = 24.0;
const MIN_PRESENT_TO_DXGI_RATIO: f64 = 0.30;
const FRAMEGEN_DISPLAY_MARGIN: f64 = 1.08;
const DXGI_DISPLAY_TOLERANCE: f64 = 0.10;
const FG_ON_WINDOWS: u32 = 2;
const FG_OFF_WINDOWS: u32 = 5;
const RATIO_EMA_ALPHA: f64 = 0.25;
const RATIO_NATIVE_DOWN_ALPHA: f64 = 0.12;
const RATIO_NATIVE_UP_ALPHA: f64 = 0.35;

/// Frame-generation detection latch and ratio smoothing state.
#[derive(Debug, Default)]
pub(crate) struct FrameGenState {
    latched: bool,
    fg_on_streak: u32,
    fg_off_streak: u32,
    last_fg_native: f64,
    native_for_ratio: f64,
    smoothed_ratio: f64,
}

impl FrameGenState {
    pub fn new() -> Self {
        Self {
            smoothed_ratio: 1.0,
            ..Default::default()
        }
    }

    pub fn is_latched(&self) -> bool {
        self.latched
    }

    pub fn update_latch(&mut self, indicators: bool) {
        if indicators {
            self.fg_on_streak += 1;
            self.fg_off_streak = 0;
            if self.fg_on_streak >= FG_ON_WINDOWS {
                self.latched = true;
            }
        } else {
            self.fg_off_streak += 1;
            self.fg_on_streak = 0;
            if self.fg_off_streak >= FG_OFF_WINDOWS {
                self.latched = false;
                self.last_fg_native = 0.0;
            }
        }
    }

    pub fn compute_ratio(&mut self, display: f64, native: f64, frame_gen_active: bool) -> f64 {
        if !frame_gen_active || display <= 0.0 || native <= 0.0 {
            self.smoothed_ratio = 1.0;
            self.native_for_ratio = 0.0;
            return 1.0;
        }

        update_native_for_ratio(&mut self.native_for_ratio, native);
        let denom = self.native_for_ratio.max(1.0);
        let raw = display / denom;
        self.smoothed_ratio = if self.smoothed_ratio <= 1.01 {
            raw
        } else {
            ema(self.smoothed_ratio, raw, RATIO_EMA_ALPHA)
        };
        self.smoothed_ratio.max(1.0)
    }

    pub fn native_with_frame_gen(&mut self, display: f64, present: f64) -> f64 {
        let plausible = present >= MIN_PRESENT_FOR_FRAMEGEN
            && present < display / FRAMEGEN_DISPLAY_MARGIN;

        if plausible {
            self.last_fg_native = present;
            return present;
        }

        if self.last_fg_native > 0.0 {
            return self.last_fg_native;
        }

        if display > 0.0 {
            display / FRAMEGEN_DISPLAY_MARGIN
        } else {
            0.0
        }
    }
}

pub(crate) fn present_history_indicates_frame_gen(display: f64, dxgi: f64, present: f64) -> bool {
    if dxgi <= 0.0 || display <= 0.0 || present <= 0.0 {
        return false;
    }

    let dxgi_tracks_display = (display - dxgi).abs() <= display * DXGI_DISPLAY_TOLERANCE;
    let present_below_display = present < display / FRAMEGEN_DISPLAY_MARGIN;
    let present_is_plausible = present >= MIN_PRESENT_FOR_FRAMEGEN;
    let present_fraction_of_dxgi = present / dxgi;

    dxgi_tracks_display
        && present_below_display
        && present_is_plausible
        && present_fraction_of_dxgi >= MIN_PRESENT_TO_DXGI_RATIO
}

pub(crate) fn native_without_frame_gen(display: f64, dxgi: f64) -> f64 {
    let native = if dxgi > 0.0 { dxgi } else { display };

    if display > 0.0 && native > 0.0 {
        if (display - native).abs() / display <= DXGI_DISPLAY_TOLERANCE {
            return display;
        }
    }

    native
}

pub(crate) fn hooked_frame_gen_active(raw_display: f64, hooked_fps: f64) -> bool {
    raw_display > 0.0 && hooked_fps > 0.0 && raw_display > hooked_fps * FRAMEGEN_DISPLAY_MARGIN
}

fn update_native_for_ratio(native_for_ratio: &mut f64, native: f64) {
    if native <= 0.0 {
        return;
    }
    if *native_for_ratio <= 0.0 {
        *native_for_ratio = native;
        return;
    }
    let alpha = if native >= *native_for_ratio {
        RATIO_NATIVE_UP_ALPHA
    } else {
        RATIO_NATIVE_DOWN_ALPHA
    };
    *native_for_ratio = ema(*native_for_ratio, native, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_gen_detects_present_history_pattern() {
        assert!(present_history_indicates_frame_gen(120.0, 118.0, 60.0));
        assert!(!present_history_indicates_frame_gen(120.0, 50.0, 60.0));
    }

    #[test]
    fn native_without_frame_gen_prefers_aligned_dxgi() {
        assert!((native_without_frame_gen(120.0, 119.0) - 120.0).abs() < 0.01);
        assert!((native_without_frame_gen(120.0, 50.0) - 50.0).abs() < 0.01);
    }
}
