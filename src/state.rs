use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct State {
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub active_shell: Option<String>,
    #[serde(default)]
    pub components: HashMap<String, bool>,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
    /// Tracks which files were deployed per component for clean undeploy.
    #[serde(default)]
    pub deployed: HashMap<String, Vec<String>>,
}

impl State {
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read state: {}", path.display()))?;
            let state: State = toml::from_str(&content)
                .with_context(|| "Failed to parse state file")?;
            Ok(state)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize state")?;
        std::fs::write(path, content)
            .with_context(|| format!("Failed to write state: {}", path.display()))?;
        Ok(())
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.components.get(name).copied().unwrap_or(false)
    }

    pub fn is_first_run(&self) -> bool {
        self.components.is_empty()
    }
}
