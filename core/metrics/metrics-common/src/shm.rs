use std::ptr::NonNull;
use std::sync::OnceLock;

use parking_lot::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Threading::GetCurrentProcessId;

use crate::counter::{FpsWindow, FrametimeTracker};

const MAGIC: u32 = 0x474F564D; // GOVM
const VERSION: u32 = 4;

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
                crate::log::debug_log("glint-metrics", &format!("shared metrics unavailable: {err}"));
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
            b.fg_kind = fg_kind;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shm_name_for_pid_format() {
        assert_eq!(shm_name_for_pid(4242), "Local\\GlintMetrics-4242");
    }
}
