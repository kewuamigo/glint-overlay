//! Standard multi-arch overlay DLL filenames under a directory.

use std::path::{Path, PathBuf};

use crate::OverlayDll;

pub const OVERLAY_DLL_X64: &str = "glint_overlay-x64.dll";
pub const OVERLAY_DLL_X86: &str = "glint_overlay-x86.dll";
pub const OVERLAY_DLL_ARM64: &str = "glint_overlay-aarch64.dll";

/// Built metrics-native DLL filename (under `target/{profile}/`).
pub const METRICS_DLL_NAME: &str = "glint_metrics_native.dll";

/// Owned paths to the overlay DLL set under `dir`.
#[derive(Debug, Clone)]
pub struct OverlayDllPaths {
    pub x64: PathBuf,
    pub x86: PathBuf,
    pub arm64: PathBuf,
}

/// Resolve standard overlay DLL filenames under `dir`.
pub fn overlay_dll_paths(dir: &Path) -> OverlayDllPaths {
    OverlayDllPaths {
        x64: dir.join(OVERLAY_DLL_X64),
        x86: dir.join(OVERLAY_DLL_X86),
        arm64: dir.join(OVERLAY_DLL_ARM64),
    }
}

/// Borrowed [`OverlayDll`] for inject APIs (paths must outlive the call).
pub fn overlay_dll_ref(paths: &OverlayDllPaths) -> OverlayDll<'_> {
    OverlayDll {
        x64: Some(&paths.x64),
        x86: Some(&paths.x86),
        arm64: Some(&paths.arm64),
    }
}

/// Primary marker file used when searching for a built overlay DLL directory.
pub fn overlay_dll_marker(dir: &Path) -> PathBuf {
    dir.join(OVERLAY_DLL_X64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_dll_paths_joins_standard_filenames() {
        let dir = Path::new(r"C:\build\overlay");
        let paths = overlay_dll_paths(dir);
        assert_eq!(paths.x64, dir.join(OVERLAY_DLL_X64));
        assert_eq!(paths.x86, dir.join(OVERLAY_DLL_X86));
        assert_eq!(paths.arm64, dir.join(OVERLAY_DLL_ARM64));
    }
}
