use serde::Serialize;

/// Which frame-generation technology produced the generated frames.
///
/// Mirrors PresentMon's `FrameType` tags (XeFG/AFMF come from driver-emitted
/// `Intel-PresentMon` ETW events) plus module-scan detection for DLSS/FSR
/// injected SDKs (Steam-overlay style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameGenKind {
    #[default]
    None,
    /// NVIDIA DLSS Frame Generation (Streamline proxy swapchain).
    Dlss,
    /// AMD FSR 3 Frame Interpolation (in-game SDK).
    Fsr,
    /// Intel XeSS Frame Generation (driver-tagged, FrameType = 50).
    XeFg,
    /// AMD Fluid Motion Frames (driver-level, FrameType = 100).
    Afmf,
    /// Frame generation detected by rate heuristics only.
    Unknown,
}

impl FrameGenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Dlss => "dlss",
            Self::Fsr => "fsr",
            Self::XeFg => "xefg",
            Self::Afmf => "afmf",
            Self::Unknown => "unknown",
        }
    }

    /// Map the `fg_kind` field written by the injected metrics DLL (module scan).
    pub fn from_shm(value: u32) -> Self {
        match value {
            1 => Self::Dlss,
            2 => Self::Fsr,
            3 => Self::XeFg,
            _ => Self::None,
        }
    }

    /// Map an `Intel-PresentMon` `FrameType` tag byte to a kind.
    pub fn from_frame_type_tag(tag: u8) -> Self {
        match tag {
            50 => Self::XeFg,
            100 => Self::Afmf,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HookedNativeSample {
    /// Present-hook FPS; 0.0 when the hook is not counting (only module scan alive).
    pub fps: f64,
    pub frame_time_ms: f64,
    /// Frame-generation kind detected via module scan in the injected DLL (SHM `fg_kind`).
    pub fg_kind: u32,
    /// `fg_kind` was refreshed recently — the module scan thread is alive.
    pub fg_kind_fresh: bool,
    /// Game-engine frame rate from the Streamline `slGetNewFrameToken` hook;
    /// 0.0 when Streamline is absent or the hook is not counting. Under
    /// DLSS-FG this is the true native rate (the Present hook sees display rate).
    pub game_fps: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct MetricsSnapshot {
    pub native_fps: f64,
    pub generated_fps: f64,
    pub frame_gen_active: bool,
    pub frame_gen_ratio: f64,
    /// "none" | "dlss" | "fsr" | "xefg" | "afmf" | "unknown"
    pub frame_gen_kind: &'static str,
    /// How the native/generated split was derived:
    /// "driver" (per-frame ETW tags) | "hook" (Present hook) | "etw" (heuristic).
    pub frame_split_source: &'static str,
    /// Cumulative driver-tagged frame counts (0 when driver tags unavailable).
    pub native_frames_total: u64,
    pub generated_frames_total: u64,
    pub native_min: f64,
    pub native_max: f64,
    pub generated_min: f64,
    pub generated_max: f64,
    pub native_frame_time_ms: f64,
    pub display_frame_time_ms: f64,
    pub dxgi_events: u64,
    pub dxgk_flip_events: u64,
    pub dxgk_present_events: u64,
}

impl Default for MetricsSnapshot {
    fn default() -> Self {
        Self {
            native_fps: 0.0,
            generated_fps: 0.0,
            frame_gen_active: false,
            frame_gen_ratio: 0.0,
            frame_gen_kind: FrameGenKind::None.as_str(),
            frame_split_source: "etw",
            native_frames_total: 0,
            generated_frames_total: 0,
            native_min: 0.0,
            native_max: 0.0,
            generated_min: 0.0,
            generated_max: 0.0,
            native_frame_time_ms: 0.0,
            display_frame_time_ms: 0.0,
            dxgi_events: 0,
            dxgk_flip_events: 0,
            dxgk_present_events: 0,
        }
    }
}
