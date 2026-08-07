//! Shared inject configuration constants (launcher + inject helpers).

/// Default games config path (Steam scan / launcher).
pub const DEFAULT_GAMES_CONFIG: &str = "config/games.json";

/// Environment variable the launcher may set for overlay DLL resolution.
pub const ENV_OVERLAY_DLL_DIR: &str = "GLINT_OVERLAY_DLL_DIR";

/// Optional path overrides (kept for call-site ergonomics; launcher uses defaults).
#[derive(Debug, Clone, Default)]
pub struct InjectOptions {
    pub dll_dir: Option<std::path::PathBuf>,
    pub metrics_dll: Option<std::path::PathBuf>,
}
