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
pub use glint_overlay_client::{
    ACHIEVEMENTS_DLL_NAME, METRICS_DLL_NAME, OVERLAY_DLL_X64, OverlayDllPaths, inject_stopped,
    overlay_dll_marker, overlay_dll_paths, overlay_dll_ref,
};
pub use inject::{
    default_achievements_dll, default_metrics_dll, default_overlay_dll_dir, inject_achievements_dll,
    inject_achievements_dll_stopped, inject_metrics_dll, inject_overlay_cef_pipe, overlay_cef_pipe,
};
pub use options::{DEFAULT_GAMES_CONFIG, ENV_OVERLAY_DLL_DIR, InjectOptions};
pub use paths::{
    PROJECT_HOST_MARKER, find_overlay_dll_dir, install_root, is_project_root, project_root,
    require_overlay_dll_dir, runtime_root, spinning_cube_path,
};
pub use process::{
    ProcessEntry, find_pid_by_name, is_process_alive, list_all_processes, list_running_targets,
    resolve_pid_target,
};
pub use steam::{ScannedGame, scan_games, steam_display_name};
