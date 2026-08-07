use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub games: Vec<GameEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameEntry {
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub inject_metrics: bool,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let data = std::fs::read_to_string(path)
                .with_context(|| format!("read config: {}", path.display()))?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(Self::default())
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { games: vec![] }
    }
}
