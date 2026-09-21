//! Steam `VulkanSteamOverlayProcessCapturedFrame` @ `0x1800C6AC0`.
//!
//! 13-arg export. `a2` picks RGB / NV12 / shared. Body overwrites one last-frame
//! slot. Shared stores the handle (`a6`); CPU RGB/NV12 may copy a bounded buffer.
//! Not an overlay blit. Not a present. No `CSharedMemStream`.

use std::ptr;

use parking_lot::Mutex;

/// Steam `a2` format (`0x1800C6AC0` switch / NV12 / else-RGB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturedFormat {
    /// `a2 == 0x1000` — Game Vulkan Shared RGB (`0x00424752`).
    SharedRgb,
    /// `a2 == 0x2000` — Game Vulkan Shared BGR (`0x00524742`).
    SharedBgr,
    /// `a2 == 0x4000` — Game Vulkan Shared NV12.
    SharedNv12,
    /// `a2 == 0x8000` — Game Vulkan Shared P010.
    SharedP010,
    /// `a2 == 512` — Game Vulkan NV12 (CPU; chroma at `a6 + a9*a7`).
    Nv12,
    /// else — Game Vulkan RGB.
    Rgb,
}

impl CapturedFormat {
    fn is_shared(self) -> bool {
        matches!(
            self,
            Self::SharedRgb | Self::SharedBgr | Self::SharedNv12 | Self::SharedP010
        )
    }
}

/// One last ingested game frame. Overwritten on each ingest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastCapturedFrame {
    /// Shared formats: handle only. Do not `OpenShared*` on the overlay device.
    Shared {
        format: CapturedFormat,
        handle: usize,
        width: i32,
        height: i32,
        stride: u32,
    },
    /// CPU RGB / NV12. Bytes empty when src is null or over the bound.
    Cpu {
        format: CapturedFormat,
        width: i32,
        height: i32,
        stride: u32,
        bytes: Vec<u8>,
    },
}

/// Steam C2B10 luma is `a9 * a7`; NV12 chroma is `a9/2` rows after that.
const MAX_CPU_BYTES: usize = 64 * 1024 * 1024;

static LAST: Mutex<Option<LastCapturedFrame>> = Mutex::new(None);

pub fn classify_captured_format(a2: i32) -> CapturedFormat {
    match a2 {
        0x1000 => CapturedFormat::SharedRgb,
        0x2000 => CapturedFormat::SharedBgr,
        0x4000 => CapturedFormat::SharedNv12,
        0x8000 => CapturedFormat::SharedP010,
        512 => CapturedFormat::Nv12,
        _ => CapturedFormat::Rgb,
    }
}

fn cpu_byte_len(format: CapturedFormat, stride: u32, rows: i32) -> Option<usize> {
    let rows = usize::try_from(rows).ok()?;
    let stride = stride as usize;
    let luma = rows.checked_mul(stride)?;
    match format {
        CapturedFormat::Nv12 => luma.checked_add(luma / 2),
        CapturedFormat::Rgb => Some(luma),
        _ => None,
    }
}

fn copy_cpu(format: CapturedFormat, src: i64, stride: u32, rows: i32) -> Vec<u8> {
    let Some(len) = cpu_byte_len(format, stride, rows) else {
        return Vec::new();
    };
    if len == 0 || len > MAX_CPU_BYTES || src == 0 {
        return Vec::new();
    }
    let mut out = vec![0u8; len];
    unsafe {
        ptr::copy_nonoverlapping(src as *const u8, out.as_mut_ptr(), len);
    }
    out
}

/// Ingest into the last-frame slot (overwrite). Returns 1. Never blits or Presents.
pub fn ingest_captured_frame(a2: i32, a6: i64, a7: u32, a8: i32, a9: i32) -> i64 {
    let format = classify_captured_format(a2);
    let frame = if format.is_shared() {
        LastCapturedFrame::Shared {
            format,
            handle: a6 as usize,
            width: a8,
            height: a9,
            stride: a7,
        }
    } else {
        LastCapturedFrame::Cpu {
            format,
            width: a8,
            height: a9,
            stride: a7,
            bytes: copy_cpu(format, a6, a7, a9),
        }
    };
    *LAST.lock() = Some(frame);
    1
}

#[cfg(test)]
pub(crate) fn last_captured_frame() -> Option<LastCapturedFrame> {
    LAST.lock().clone()
}

#[cfg(test)]
mod tests {
    use super::{
        CapturedFormat, LastCapturedFrame, classify_captured_format, ingest_captured_frame,
        last_captured_frame,
    };

    #[test]
    fn a2_0x1000_is_shared_rgb() {
        assert_eq!(classify_captured_format(0x1000), CapturedFormat::SharedRgb);
    }

    #[test]
    fn a2_0x2000_is_shared_bgr() {
        assert_eq!(classify_captured_format(0x2000), CapturedFormat::SharedBgr);
    }

    #[test]
    fn a2_0x4000_is_shared_nv12() {
        assert_eq!(classify_captured_format(0x4000), CapturedFormat::SharedNv12);
    }

    #[test]
    fn a2_0x8000_is_shared_p010() {
        assert_eq!(classify_captured_format(0x8000), CapturedFormat::SharedP010);
    }

    #[test]
    fn a2_512_is_nv12() {
        assert_eq!(classify_captured_format(512), CapturedFormat::Nv12);
    }

    #[test]
    fn a2_other_is_rgb() {
        assert_eq!(classify_captured_format(0), CapturedFormat::Rgb);
        assert_eq!(classify_captured_format(1), CapturedFormat::Rgb);
        assert_eq!(classify_captured_format(256), CapturedFormat::Rgb);
        assert_eq!(classify_captured_format(0x1234), CapturedFormat::Rgb);
    }

    #[test]
    fn ingest_overwrites_last_slot() {
        assert_eq!(ingest_captured_frame(0x1000, 0x11, 4, 2, 2), 1);
        assert_eq!(ingest_captured_frame(0x2000, 0x22, 8, 4, 3), 1);
        assert_eq!(
            last_captured_frame(),
            Some(LastCapturedFrame::Shared {
                format: CapturedFormat::SharedBgr,
                handle: 0x22,
                width: 4,
                height: 3,
                stride: 8,
            })
        );

        let first = [1u8, 2, 3, 4];
        assert_eq!(ingest_captured_frame(0, first.as_ptr() as i64, 4, 1, 1), 1);
        let second = [9u8; 8];
        assert_eq!(ingest_captured_frame(0, second.as_ptr() as i64, 4, 2, 2), 1);
        match last_captured_frame() {
            Some(LastCapturedFrame::Cpu {
                format,
                width,
                height,
                stride,
                bytes,
            }) => {
                assert_eq!(format, CapturedFormat::Rgb);
                assert_eq!((width, height, stride), (2, 2, 4));
                assert_eq!(bytes, second);
            }
            other => panic!("expected cpu rgb overwrite, got {other:?}"),
        }
    }
}
