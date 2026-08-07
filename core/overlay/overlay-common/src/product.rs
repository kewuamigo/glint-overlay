//! Canonical Glint product identity strings (paths, pipes). Keep in sync with
//! TypeScript `host/launcher/src/product.ts`.

/// Roaming / local AppData folder name (`%APPDATA%/Glint`).
pub const APP_DATA_DIR_NAME: &str = "Glint";

/// Named-pipe leaf prefix before `-{pid}-{module}` (`\\.\pipe\glint-overlay-…`).
pub const PIPE_NAME_PREFIX: &str = "glint-overlay";

/// Marker file under the Glint AppData root after copying legacy GameOverlay data.
pub const MIGRATION_MARKER: &str = ".migrated-from-gameoverlay";

/// Legacy AppData folder name (pre-Glint).
pub const LEGACY_APP_DATA_DIR_NAME: &str = "GameOverlay";
