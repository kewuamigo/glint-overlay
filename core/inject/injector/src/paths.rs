//! Shared helpers for locating the repo root and built DLL artifacts.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use glint_overlay_client::paths::overlay_dll_marker;

/// Marker file used to identify the monorepo root (no Electron overlay build needed).
pub const PROJECT_HOST_MARKER: &str = "pnpm-workspace.yaml";

/// Packaged install root: directory containing `host/` and `native/` next to the exe.
pub fn install_root() -> Option<PathBuf> {
    let parent = exe_parent()?;
    if parent.join("host").is_dir() && parent.join("native").is_dir() {
        Some(parent)
    } else {
        None
    }
}

/// Prefer packaged layout; fall back to monorepo root.
pub fn runtime_root() -> Option<PathBuf> {
    install_root().or_else(project_root)
}

/// Walk upward from cwd and the executable directory to find the repo root.
pub fn project_root() -> Option<PathBuf> {
    for start in search_roots() {
        if let Some(root) = walk_up_to_root(start) {
            return Some(root);
        }
    }
    None
}

/// Returns true when `dir` looks like the gameoverlay workspace root.
pub fn is_project_root(dir: &Path) -> bool {
    dir.join(PROJECT_HOST_MARKER).is_file() && dir.join("Cargo.toml").is_file()
}

/// Search `target/{release,debug}/`, repo root, then the executable directory.
pub fn find_built_dll(file_name: &str) -> Option<PathBuf> {
    find_beside_exe(file_name)
        .or_else(|| {
            install_root().and_then(|root| {
                let path = root.join("native").join(file_name);
                path.exists().then_some(path)
            })
        })
        .or_else(|| find_in_target_profiles(file_name))
        .or_else(|| {
            project_root().and_then(|root| {
                ["target/release", "target/debug"]
                    .map(PathBuf::from)
                    .map(|dir| root.join(dir).join(file_name))
                    .into_iter()
                    .find(|path| path.exists())
            })
        })
}

/// Search a directory that contains the overlay multi-arch DLL set.
pub fn find_overlay_dll_dir() -> PathBuf {
    if let Some(root) = install_root() {
        let native = root.join("native");
        if overlay_dll_marker(&native).exists() {
            return native;
        }
    }

    const DIR_CANDIDATES: &[&str] = &[
        "target/release/glint-overlay",
        "target/debug/glint-overlay",
        "host/native",
    ];

    for rel in DIR_CANDIDATES {
        if let Some(root) = project_root() {
            let dir = root.join(rel);
            if overlay_dll_marker(&dir).exists() {
                return dir;
            }
        }

        let dir = PathBuf::from(rel);
        if overlay_dll_marker(&dir).exists() {
            return dir;
        }
    }

    if let Some(dir) = exe_parent()
        .map(|parent| parent.join("glint-overlay"))
        .filter(|dir| overlay_dll_marker(dir).exists())
    {
        return dir;
    }

    if let Some(root) = runtime_root() {
        let native = root.join("native");
        if overlay_dll_marker(&native).exists() {
            return native;
        }
        return root.join("host/native");
    }

    PathBuf::from("host/native")
}

/// Path to `SpinningCube.exe` at the repository root (integration / manual smoke).
pub fn spinning_cube_path() -> Option<PathBuf> {
    project_root().map(|root| root.join("SpinningCube.exe"))
}

/// Returns overlay DLL directory or an error when the x64 DLL is missing.
pub fn require_overlay_dll_dir() -> Result<PathBuf> {
    let dir = find_overlay_dll_dir();
    let marker = overlay_dll_marker(&dir);
    if marker.is_file() {
        Ok(dir)
    } else {
        bail!(
            "overlay DLL not built — run cargo build -p glint-overlay-dll and scripts/build-overlay.ps1\n  missing: {}",
            marker.display()
        )
    }
}

fn find_in_target_profiles(file_name: &str) -> Option<PathBuf> {
    let profiles = ["target/release", "target/debug"];
    for rel in profiles {
        let path = PathBuf::from(rel).join(file_name);
        if path.exists() {
            return Some(path);
        }
        if let Some(root) = project_root() {
            let path = root.join(rel).join(file_name);
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn find_beside_exe(file_name: &str) -> Option<PathBuf> {
    exe_parent()
        .map(|dir| dir.join(file_name))
        .filter(|path| path.exists())
}

fn search_roots() -> Vec<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    if let Some(parent) = exe_parent() {
        starts.push(parent);
    }
    starts
}

fn walk_up_to_root(mut dir: PathBuf) -> Option<PathBuf> {
    for _ in 0..10 {
        if is_project_root(&dir) {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn exe_parent() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_project_root_when_running_from_repo() {
        if let Some(root) = project_root() {
            assert!(is_project_root(&root));
        }
    }

    #[test]
    fn spinning_cube_marker_paths() {
        if let Some(root) = project_root() {
            let cube = root.join("SpinningCube.exe");
            if cube.is_file() {
                assert!(find_built_dll("nonexistent.dll").is_none());
            }
        }
    }
}
