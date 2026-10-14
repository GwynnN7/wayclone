use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub shells: Option<ShellConfig>,
    #[serde(default)]
    pub components: Vec<Component>,
}

#[derive(Debug, Deserialize)]
pub struct ShellConfig {
    pub available: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Component {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub post_install: Vec<String>,
    #[serde(default)]
    pub post_deploy: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub src: String,
    pub dest: String,
    #[serde(default)]
    pub sudo: bool,
    #[serde(default = "default_true")]
    pub check: bool,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read manifest: {}", path.display()))?;
        let manifest: Manifest = toml::from_str(&content)
            .with_context(|| "Failed to parse manifest.toml")?;
        Ok(manifest)
    }

    pub fn available_shells(&self) -> Vec<&str> {
        self.shells
            .as_ref()
            .map(|s| s.available.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Returns components visible for the given active shell.
    /// Global components (shell=None) are always visible.
    /// Shell-specific components are only visible when their shell matches.
    pub fn components_for_shell(&self, shell: Option<&str>) -> Vec<&Component> {
        self.components
            .iter()
            .filter(|c| match (&c.shell, shell) {
                (None, _) => true,
                (Some(cs), Some(s)) => cs == s,
                (Some(_), None) => false,
            })
            .collect()
    }

    /// Returns a component by name, if it exists.
    pub fn find_component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }
}
