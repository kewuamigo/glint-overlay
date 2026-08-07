//! Shared injection and process utilities for the game overlay platform.
//!
//! The standalone `glint-injector` CLI was removed — the launcher owns
//! attach (`inject_overlay_cef_pipe` + metrics DLL).

pub mod config;
pub mod inject;
pub mod options;
pub mod paths;
pub mod process;
pub mod steam;

pub use config::{AppConfig, GameEntry};
pub use inject::{
    default_metrics_dll, default_overlay_dll_dir, inject_metrics_dll, inject_overlay_cef_pipe,
};
pub use options::{InjectOptions, DEFAULT_GAMES_CONFIG, ENV_OVERLAY_DLL_DIR};
pub use paths::{
    find_overlay_dll_dir, install_root, is_project_root, project_root, require_overlay_dll_dir,
    runtime_root, spinning_cube_path, PROJECT_HOST_MARKER,
};
pub use glint_overlay_client::{
    overlay_dll_marker, overlay_dll_paths, overlay_dll_ref, OverlayDllPaths, OVERLAY_DLL_X64,
    METRICS_DLL_NAME,
};
pub use process::{
    find_pid_by_name, is_process_alive, list_all_processes, list_running_targets,
    resolve_pid_target, ProcessEntry,
};
pub use steam::{scan_games, steam_display_name, ScannedGame};
