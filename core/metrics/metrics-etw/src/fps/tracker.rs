use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glint_metrics_common::timing::{ema, FrametimeEma, FpsWindow as FpsWindowCore};

use super::frame_gen::{
    FrameGenState, hooked_frame_gen_active, native_without_frame_gen,
    present_history_indicates_frame_gen,
};
use super::snapshot::{FrameGenKind, HookedNativeSample, MetricsSnapshot};

/// Intel-PresentMon FrameType tag values (see PresentMon `Intel_PresentMon.h`).
const FRAME_TYPE_ORIGINAL: u8 = 1;
const FRAME_TYPE_REPEATED: u8 = 2;
const FRAME_TYPE_INTEL_XEFG: u8 = 50;
const FRAME_TYPE_AMD_AFMF: u8 = 100;

/// Driver tags older than this mean the FG SDK/driver stopped emitting.
const TAG_STALE_AFTER: Duration = Duration::from_millis(2_000);

#[derive(Debug, Default)]
pub struct EventCounters {
    pub dxgi: AtomicU64,
    pub dxgk_flip: AtomicU64,
    pub dxgk_present: AtomicU64,
}

impl EventCounters {
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.dxgi.load(Ordering::Relaxed),
            self.dxgk_flip.load(Ordering::Relaxed),
            self.dxgk_present.load(Ordering::Relaxed),
        )
    }
}

struct FrametimeTracker {
    last_at: Option<Instant>,
    inner: FrametimeEma,
}

impl FrametimeTracker {
    fn new() -> Self {
        Self {
            last_at: None,
            inner: FrametimeEma::new(),
        }
    }

    fn on_frame(&mut self) -> f64 {
        let now = Instant::now();
        if let Some(last) = self.last_at {
            let dt = now.duration_since(last).as_secs_f64() * 1000.0;
            self.inner.on_delta_ms(dt);
        }
        self.last_at = Some(now);
        self.inner.value()
    }
}

/// Rolling FPS over a short window (PresentMon-style), with min/max tracking for ETW.
struct FpsWindow {
    window_start: Instant,
    inner: FpsWindowCore,
    min_fps: f64,
    max_fps: f64,
}

impl FpsWindow {
    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            inner: FpsWindowCore::new(),
            min_fps: f64::MAX,
            max_fps: 0.0,
        }
    }

    fn on_frame(&mut self) {
        let elapsed = self.window_start.elapsed().as_millis() as u64;
        let tick = self.inner.on_frame(elapsed);
        if tick.window_reset {
            if tick.fps > 0.0 {
                if tick.fps < self.min_fps {
                    self.min_fps = tick.fps;
                }
                if tick.fps > self.max_fps {
                    self.max_fps = tick.fps;
                }
            }
            self.window_start = Instant::now();
        }
    }

    fn current_fps(&self) -> f64 {
        self.inner.current_fps()
    }

    fn min_fps(&self) -> f64 {
        if self.min_fps == f64::MAX {
            0.0
        } else {
            self.min_fps
        }
    }

    fn max_fps(&self) -> f64 {
        self.max_fps
    }
}

/// Per-frame counters driven by driver/SDK-emitted `Intel-PresentMon`
/// FrameType tags (Steam-overlay / PresentMon `--track_frame_type` style).
struct TaggedFrameCounter {
    native: FpsWindow,
    generated: FpsWindow,
    native_total: u64,
    generated_total: u64,
    last_kind: FrameGenKind,
    last_native_at: Option<Instant>,
    last_generated_at: Option<Instant>,
}

impl TaggedFrameCounter {
    fn new() -> Self {
        Self {
            native: FpsWindow::new(),
            generated: FpsWindow::new(),
            native_total: 0,
            generated_total: 0,
            last_kind: FrameGenKind::None,
            last_native_at: None,
            last_generated_at: None,
        }
    }

    fn on_tag(&mut self, tag: u8) {
        match tag {
            FRAME_TYPE_ORIGINAL => {
                self.native.on_frame();
                self.native_total += 1;
                self.last_native_at = Some(Instant::now());
            }
            FRAME_TYPE_INTEL_XEFG | FRAME_TYPE_AMD_AFMF => {
                self.generated.on_frame();
                self.generated_total += 1;
                self.last_kind = FrameGenKind::from_frame_type_tag(tag);
                self.last_generated_at = Some(Instant::now());
            }
            // Repeated frames are neither app-rendered nor interpolated.
            FRAME_TYPE_REPEATED => {}
            _ => {}
        }
    }

    /// Native tags are flowing recently enough to trust for the split.
    fn is_active(&self) -> bool {
        self.last_native_at
            .is_some_and(|at| at.elapsed() < TAG_STALE_AFTER)
    }

    /// Generated tags seen recently → frame generation currently on.
    fn generation_active(&self) -> bool {
        self.last_generated_at
            .is_some_and(|at| at.elapsed() < TAG_STALE_AFTER)
    }
}

pub struct HardwareFpsTracker {
    dxgi: FpsWindow,
    flip: FpsWindow,
    present_hist: FpsWindow,
    display_frametime: FrametimeTracker,
    native_min: f64,
    native_max: f64,
    counters: Arc<EventCounters>,
    frame_gen: FrameGenState,
    /// Per-flip driver tags (AMD AFMF path — classifies displayed frames).
    tagged_flips: TaggedFrameCounter,
    /// Per-present SDK tags (Intel XeFG path — classifies presented frames).
    tagged_presents: TaggedFrameCounter,
    smoothed_native: f64,
    smoothed_display: f64,
    native_frame_time_ms: f64,
    display_frame_time_ms: f64,
}

impl HardwareFpsTracker {
    const NATIVE_EMA_ALPHA: f64 = 0.45;
    const DISPLAY_EMA_ALPHA: f64 = 0.45;

    pub fn new(counters: Arc<EventCounters>) -> Self {
        Self {
            dxgi: FpsWindow::new(),
            flip: FpsWindow::new(),
            present_hist: FpsWindow::new(),
            display_frametime: FrametimeTracker::new(),
            native_min: f64::MAX,
            native_max: 0.0,
            counters,
            frame_gen: FrameGenState::new(),
            tagged_flips: TaggedFrameCounter::new(),
            tagged_presents: TaggedFrameCounter::new(),
            smoothed_native: 0.0,
            smoothed_display: 0.0,
            native_frame_time_ms: 0.0,
            display_frame_time_ms: 0.0,
        }
    }

    pub fn on_dxgi_present(&mut self) {
        self.dxgi.on_frame();
    }

    pub fn on_flip(&mut self) {
        self.flip.on_frame();
        self.display_frame_time_ms = self.display_frametime.on_frame();
    }

    pub fn on_present_history(&mut self) {
        self.present_hist.on_frame();
    }

    /// Intel-PresentMon `FlipFrameType` tag (driver-emitted, per displayed flip).
    pub fn on_tagged_flip(&mut self, tag: u8) {
        self.tagged_flips.on_tag(tag);
    }

    /// Intel-PresentMon `PresentFrameType` tag (SDK-emitted, per present).
    pub fn on_tagged_present(&mut self, tag: u8) {
        self.tagged_presents.on_tag(tag);
    }

    fn track_native_bounds(&mut self, native: f64) {
        if native <= 0.0 {
            return;
        }
        if native < self.native_min {
            self.native_min = native;
        }
        if native > self.native_max {
            self.native_max = native;
        }
    }

    fn smooth_native(&mut self, raw: f64) -> f64 {
        self.smoothed_native = ema(self.smoothed_native, raw, Self::NATIVE_EMA_ALPHA);
        self.smoothed_native
    }

    fn smooth_display(&mut self, raw: f64) -> f64 {
        self.smoothed_display = ema(self.smoothed_display, raw, Self::DISPLAY_EMA_ALPHA);
        self.smoothed_display
    }

    /// Driver-tagged frame counters when they are fresh (preferred split source).
    fn active_tagged(&self) -> Option<&TaggedFrameCounter> {
        // Flip tags classify what actually reached the display; prefer them.
        if self.tagged_flips.is_active() {
            Some(&self.tagged_flips)
        } else if self.tagged_presents.is_active() {
            Some(&self.tagged_presents)
        } else {
            None
        }
    }

    fn tagged_totals(&self) -> (u64, u64) {
        (
            self.tagged_flips.native_total + self.tagged_presents.native_total,
            self.tagged_flips.generated_total + self.tagged_presents.generated_total,
        )
    }

    fn finish_snapshot(
        &mut self,
        raw_native: f64,
        raw_display: f64,
        frame_gen_active: bool,
        hooked_frame_time_ms: Option<f64>,
        frame_gen_kind: FrameGenKind,
        frame_split_source: &'static str,
    ) -> MetricsSnapshot {
        let native = self.smooth_native(raw_native);
        let display = self.smooth_display(raw_display);
        self.track_native_bounds(native);

        if let Some(ms) = hooked_frame_time_ms.filter(|v| *v > 0.0) {
            self.native_frame_time_ms = ms;
        } else if native > 0.0 {
            self.native_frame_time_ms = 1000.0 / native;
        }

        let ratio = self
            .frame_gen
            .compute_ratio(display, native, frame_gen_active);
        let (dxgi_events, dxgk_flip_events, dxgk_present_events) = self.counters.snapshot();
        let (native_frames_total, generated_frames_total) = self.tagged_totals();

        MetricsSnapshot {
            native_fps: native,
            generated_fps: display,
            frame_gen_active,
            frame_gen_ratio: ratio,
            frame_gen_kind: frame_gen_kind.as_str(),
            frame_split_source,
            native_frames_total,
            generated_frames_total,
            native_min: if self.native_min == f64::MAX {
                0.0
            } else {
                self.native_min
            },
            native_max: self.native_max,
            generated_min: self.flip.min_fps(),
            generated_max: self.flip.max_fps(),
            native_frame_time_ms: self.native_frame_time_ms,
            display_frame_time_ms: self.display_frame_time_ms,
            dxgi_events,
            dxgk_flip_events,
            dxgk_present_events,
        }
    }

    pub fn snapshot(&mut self, hooked: Option<HookedNativeSample>) -> MetricsSnapshot {
        let raw_display = self.flip.current_fps();

        // 1) Per-frame driver/SDK tags (Steam-style exact split; XeFG + AFMF).
        if let Some(tagged) = self.active_tagged() {
            let native = tagged.native.current_fps();
            let generated_rate = tagged.generated.current_fps();
            let frame_gen_active = tagged.generation_active() && generated_rate > 0.0;
            let kind = if frame_gen_active {
                tagged.last_kind
            } else {
                FrameGenKind::None
            };
            // Displayed rate: DxgKrnl flips when available, otherwise the sum
            // of tagged native + generated frames.
            let display = if raw_display > 0.0 {
                raw_display
            } else {
                native + generated_rate
            };
            let hooked_ft = hooked
                .filter(|h| h.frame_time_ms > 0.0)
                .map(|h| h.frame_time_ms);
            return self.finish_snapshot(
                native,
                display,
                frame_gen_active,
                hooked_ft,
                kind,
                "driver",
            );
        }

        // Module-scan label from the injected DLL (DLSS / FSR / XeFG), if fresh.
        let detected_kind = hooked
            .filter(|h| h.fg_kind_fresh)
            .map(|h| FrameGenKind::from_shm(h.fg_kind))
            .unwrap_or(FrameGenKind::None);

        // 2a) Streamline frame-token hook: exact game-engine rate. Under
        // DLSS-FG the Present hook sees Streamline's display-rate presents,
        // so `game_fps` is the only correct in-process native signal.
        if let Some(hook) = hooked.filter(|h| h.game_fps > 0.0 && h.game_fps.is_finite()) {
            let native = hook.game_fps;
            let display = if raw_display > 0.0 { raw_display } else { hook.fps };
            let frame_gen_active = hooked_frame_gen_active(display, native);
            let kind = if frame_gen_active {
                if detected_kind == FrameGenKind::None {
                    FrameGenKind::Dlss // slGetNewFrameToken only exists under Streamline
                } else {
                    detected_kind
                }
            } else {
                FrameGenKind::None
            };
            // Frame time from the native rate, not the display-rate Present hook.
            return self.finish_snapshot(native, display, frame_gen_active, None, kind, "hook");
        }

        // 2b) Present hook only. If the hook rate is meaningfully below the
        // display rate, the hook sits on the game side of the FG swapchain
        // and is the native rate. If an FG module is loaded but hook ≈ display,
        // the hook is counting generated presents too — fall back to kernel
        // PresentHistory for the native rate, keeping the module label.
        if let Some(hook) = hooked.filter(|h| h.fps > 0.0 && h.fps.is_finite()) {
            let frame_gen_by_rate = hooked_frame_gen_active(raw_display, hook.fps);

            if frame_gen_by_rate {
                let kind = if detected_kind == FrameGenKind::None {
                    FrameGenKind::Unknown
                } else {
                    detected_kind
                };
                return self.finish_snapshot(
                    hook.fps,
                    raw_display,
                    true,
                    Some(hook.frame_time_ms),
                    kind,
                    "hook",
                );
            }

            if detected_kind != FrameGenKind::None {
                let display = if raw_display > 0.0 { raw_display } else { hook.fps };
                let present = self.present_hist.current_fps();
                let present_is_native = present >= 24.0 && display > 0.0
                    && hooked_frame_gen_active(display, present);
                if present_is_native {
                    return self.finish_snapshot(
                        present,
                        display,
                        true,
                        None,
                        detected_kind,
                        "hook",
                    );
                }
            }

            // FG inactive (or not splittable) — hook rate is the native rate.
            return self.finish_snapshot(
                hook.fps,
                raw_display,
                false,
                Some(hook.frame_time_ms),
                FrameGenKind::None,
                "hook",
            );
        }

        // 3) ETW-only heuristics (PresentHistory vs DXGI vs flip rates).
        let dxgi = self.dxgi.current_fps();
        let present = self.present_hist.current_fps();
        let fg_indicators = present_history_indicates_frame_gen(raw_display, dxgi, present);
        self.frame_gen.update_latch(fg_indicators);

        let raw_native = if self.frame_gen.is_latched() {
            self.frame_gen.native_with_frame_gen(raw_display, present)
        } else {
            native_without_frame_gen(raw_display, dxgi)
        };

        let kind = if self.frame_gen.is_latched() {
            // Even without hook FPS, a fresh module scan still labels the tech.
            if detected_kind == FrameGenKind::None {
                FrameGenKind::Unknown
            } else {
                detected_kind
            }
        } else {
            FrameGenKind::None
        };

        self.finish_snapshot(
            raw_native,
            raw_display,
            self.frame_gen.is_latched(),
            None,
            kind,
            "etw",
        )
    }
}
