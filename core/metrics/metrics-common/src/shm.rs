use std::ptr::NonNull;
use std::sync::OnceLock;

use parking_lot::Mutex;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::core::PCWSTR;

use crate::counter::{FpsWindow, FrametimeTracker};

const MAGIC: u32 = 0x474F564D; // GOVM
const VERSION: u32 = 5;

pub const FG_KIND_NONE: u32 = 0;
pub const FG_KIND_DLSS: u32 = 1;
pub const FG_KIND_FSR: u32 = 2;

pub fn shm_name_for_pid(pid: u32) -> String {
    format!("Local\\GlintMetrics-{pid}")
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MetricsBlock {
    pub magic: u32,
    pub version: u32,
    pub pid: u32,
    pub native_fps: f32,
    pub native_frame_count: u64,
    pub updated_at_ms: u64,
    pub native_frame_time_ms: f32,
    /// Frame-generation SDK detected in-process via module scan:
    /// 0 = none, 1 = DLSS-FG (Streamline), 2 = FSR3 FG, 3 = XeSS FG.
    pub fg_kind: u32,
    /// When `fg_kind` was last refreshed (QPC ms). Separate from `updated_at_ms`
    /// so module detection stays valid even if the Present hook is not counting.
    pub fg_updated_at_ms: u64,
    /// Game-engine frame rate measured at the FG SDK boundary
    /// (Streamline `slGetNewFrameToken` — one call per simulated frame).
    /// This is the true native rate when frame generation is presenting.
    pub game_frame_fps: f32,
    pub game_frame_count: u64,
    pub game_frame_updated_at_ms: u64,
    /// Steam FG latch: 0 off, 1 on. Kind lives in `fg_kind`.
    pub fg_active: u32,
}

pub struct SharedMetrics {
    view: NonNull<MetricsBlock>,
    window: Mutex<FpsWindow>,
    frametime: Mutex<FrametimeTracker>,
    game_window: Mutex<FpsWindow>,
}

unsafe impl Send for SharedMetrics {}
unsafe impl Sync for SharedMetrics {}

static INSTANCE: OnceLock<SharedMetrics> = OnceLock::new();

impl SharedMetrics {
    pub fn create_or_open() {
        let _ = INSTANCE.get_or_init(|| {
            let pid = unsafe { GetCurrentProcessId() };
            Self::open(&shm_name_for_pid(pid), true).unwrap_or_else(|err| {
                eprintln!("[glint_metrics] shared metrics unavailable: {err}");
                crate::log::debug_log(
                    "glint-metrics",
                    &format!("shared metrics unavailable: {err}"),
                );
                Self::open(&format!("Local\\GlintMetrics-fallback-{pid}"), true)
                    .unwrap_or_else(|_| Self::noop())
            })
        });
    }

    fn noop() -> Self {
        let layout = std::alloc::Layout::new::<MetricsBlock>();
        let ptr = unsafe { std::alloc::alloc(layout) as *mut MetricsBlock };
        let view = NonNull::new(ptr).expect("alloc metrics block");
        unsafe {
            (*view.as_ptr()).magic = MAGIC;
            (*view.as_ptr()).pid = GetCurrentProcessId();
        }
        Self {
            view,
            window: Mutex::new(FpsWindow::new()),
            frametime: Mutex::new(FrametimeTracker::new()),
            game_window: Mutex::new(FpsWindow::new()),
        }
    }

    pub fn instance() -> Option<&'static SharedMetrics> {
        INSTANCE.get()
    }

    pub fn close() {
        // mapped view lives for process lifetime
    }

    fn open(name: &str, create: bool) -> windows::core::Result<Self> {
        let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe {
            CreateFileMappingW(
                HANDLE::default(),
                None,
                PAGE_READWRITE,
                0,
                std::mem::size_of::<MetricsBlock>() as u32,
                PCWSTR(name_wide.as_ptr()),
            )?
        };

        let view = unsafe {
            MapViewOfFile(
                handle,
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
                std::mem::size_of::<MetricsBlock>(),
            )
        };
        if view.Value.is_null() {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_POINTER,
            ));
        }

        let view = NonNull::new(view.Value.cast()).expect("non-null view");
        let shm = Self {
            view,
            window: Mutex::new(FpsWindow::new()),
            frametime: Mutex::new(FrametimeTracker::new()),
            game_window: Mutex::new(FpsWindow::new()),
        };

        if create {
            shm.write_block(|b| {
                b.magic = MAGIC;
                b.version = VERSION;
                b.pid = unsafe { GetCurrentProcessId() };
            });
        }

        let _ = handle;
        Ok(shm)
    }

    pub fn set_fg_kind(&self, fg_kind: u32) {
        self.write_block(|b| {
            if b.fg_active == 0 {
                b.fg_kind = fg_kind;
            }
            b.fg_updated_at_ms = crate::counter::now_ms();
        });
    }

    /// Steam kind dword from `slDLSSGSetOptions` / `ffxConfigure*` (`dword_180197870`).
    pub fn set_fg_active(&self, kind: u32) {
        self.write_block(|b| {
            b.fg_active = if kind == 0 { 0 } else { 1 };
            b.fg_kind = kind;
            b.fg_updated_at_ms = crate::counter::now_ms();
        });
    }

    /// One game-engine frame observed at the FG SDK boundary (e.g. Streamline
    /// `slGetNewFrameToken`). This is the real simulated frame rate under
    /// DLSS-FG, where the Present hook only sees display-rate presents.
    pub fn tick_game_frame(&self) {
        let mut window = self.game_window.lock();
        window.increment();
        let fps = window.fps();
        self.write_block(|b| {
            b.game_frame_fps = fps as f32;
            b.game_frame_count += 1;
            b.game_frame_updated_at_ms = crate::counter::now_ms();
        });
    }

    pub fn tick_native(&self) {
        let mut window = self.window.lock();
        window.increment();
        let fps = window.fps();
        let frame_time_ms = self.frametime.lock().on_frame();
        self.write_block(|b| {
            b.native_fps = fps as f32;
            b.native_frame_count += 1;
            b.native_frame_time_ms = frame_time_ms;
            b.updated_at_ms = crate::counter::now_ms();
            b.version = VERSION;
        });
    }

    fn write_block(&self, f: impl FnOnce(&mut MetricsBlock)) {
        unsafe {
            let block = self.view.as_ptr();
            if (*block).magic != MAGIC {
                (*block).magic = MAGIC;
                (*block).version = 1;
            }
            f(&mut *block);
        }
    }
}

/// Read metrics block from shared memory for a target process (host side).
pub fn read_metrics_for_pid(pid: u32) -> Option<MetricsBlock> {
    read_metrics_named(&shm_name_for_pid(pid))
}

fn read_metrics_named(name: &str) -> Option<MetricsBlock> {
    let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateFileMappingW(
            HANDLE::default(),
            None,
            PAGE_READWRITE,
            0,
            std::mem::size_of::<MetricsBlock>() as u32,
            PCWSTR(name_wide.as_ptr()),
        )
    }
    .ok()?;

    let view = unsafe {
        MapViewOfFile(
            handle,
            FILE_MAP_READ,
            0,
            0,
            std::mem::size_of::<MetricsBlock>(),
        )
    };
    if view.Value.is_null() {
        return None;
    }

    let block = unsafe { *view.Value.cast::<MetricsBlock>() };
    let _ = unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view.Value }) };
    if block.magic == MAGIC {
        Some(block)
    } else {
        None
    }
}

/// Steam kind dword is on when `fg_active != 0` (v5+) or when `fg_kind` is DLSS/FSR.
pub fn frame_gen_active(block: &MetricsBlock) -> bool {
    if block.version >= 5 {
        block.fg_active != 0
    } else {
        matches!(block.fg_kind, FG_KIND_DLSS | FG_KIND_FSR)
    }
}

pub fn frame_gen_kind_label(fg_kind: u32) -> &'static str {
    match fg_kind {
        FG_KIND_DLSS => "dlss",
        FG_KIND_FSR => "fsr",
        3 => "xefg",
        _ => "none",
    }
}

/// Present FPS always; game FPS only when FG is on (single FPS when off).
pub fn snapshot_fps(block: &MetricsBlock) -> (f64, Option<f64>) {
    let present = f64::from(block.native_fps);
    if !frame_gen_active(block) {
        return (present, None);
    }
    let game = f64::from(block.game_frame_fps);
    let game = if game > 0.0 && game.is_finite() {
        Some(game)
    } else {
        None
    };
    (present, game)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(
        version: u32,
        native_fps: f32,
        game_frame_fps: f32,
        fg_kind: u32,
        fg_active: u32,
    ) -> MetricsBlock {
        MetricsBlock {
            magic: MAGIC,
            version,
            pid: 1,
            native_fps,
            native_frame_count: 0,
            updated_at_ms: 0,
            native_frame_time_ms: 0.0,
            fg_kind,
            fg_updated_at_ms: 0,
            game_frame_fps,
            game_frame_count: 0,
            game_frame_updated_at_ms: 0,
            fg_active,
        }
    }

    #[test]
    fn shm_name_for_pid_format() {
        assert_eq!(shm_name_for_pid(4242), "Local\\GlintMetrics-4242");
    }

    #[test]
    fn v4_fields_stay_readable_after_v5_append() {
        let b = block(5, 120.0, 60.0, FG_KIND_DLSS, 1);
        assert_eq!(b.native_fps, 120.0);
        assert_eq!(b.game_frame_fps, 60.0);
        assert_eq!(b.fg_kind, FG_KIND_DLSS);
        assert_eq!(b.fg_active, 1);
        assert_eq!(std::mem::offset_of!(MetricsBlock, native_fps), 12);
        assert_eq!(std::mem::offset_of!(MetricsBlock, fg_kind), 36);
        assert_eq!(std::mem::offset_of!(MetricsBlock, game_frame_fps), 48);
        assert_eq!(std::mem::offset_of!(MetricsBlock, fg_active), 72);
    }

    #[test]
    fn frame_gen_on_uses_kind_dword_not_rate() {
        let b = block(5, 60.0, 60.0, FG_KIND_DLSS, 1);
        assert!(frame_gen_active(&b));
        assert_eq!(snapshot_fps(&b), (60.0, Some(60.0)));
    }

    #[test]
    fn frame_gen_off_is_single_present_fps() {
        let b = block(5, 144.0, 72.0, FG_KIND_NONE, 0);
        assert!(!frame_gen_active(&b));
        assert_eq!(snapshot_fps(&b), (144.0, None));
    }

    #[test]
    fn frame_gen_off_ignores_rate_ratio() {
        let b = block(5, 60.0, 120.0, FG_KIND_NONE, 0);
        assert!(!frame_gen_active(&b));
        assert_eq!(snapshot_fps(&b), (60.0, None));
    }

    #[test]
    fn fsr_on_splits_present_and_game() {
        let b = block(5, 120.0, 60.0, FG_KIND_FSR, 1);
        assert!(frame_gen_active(&b));
        assert_eq!(frame_gen_kind_label(b.fg_kind), "fsr");
        assert_eq!(snapshot_fps(&b), (120.0, Some(60.0)));
    }
}
