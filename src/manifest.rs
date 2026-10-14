use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default, alias = "shells")]
    pub environments: Option<EnvironmentConfig>,
    #[serde(default)]
    pub components: Vec<Component>,
}

#[derive(Debug, Deserialize)]
pub struct EnvironmentConfig {
    pub available: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Component {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default, alias = "shell")]
    pub environment: Option<String>,
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
        let manifest: Manifest =
            toml::from_str(&content).with_context(|| "Failed to parse manifest.toml")?;
        Ok(manifest)
    }

    pub fn available_environments(&self) -> Vec<&str> {
        self.environments
            .as_ref()
            .map(|s| s.available.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Returns components visible for the given active environment.
    /// Global components (environment=None) are always visible.
    /// Environment-specific components are only visible when their environment matches.
    pub fn components_for_environment(&self, env: Option<&str>) -> Vec<&Component> {
        self.components
            .iter()
            .filter(|c| match (&c.environment, env) {
                (None, _) => true,
                (Some(ce), Some(e)) => ce == e,
                (Some(_), None) => false,
            })
            .collect()
    }

    /// Returns ONLY the components bound to a specific environment.
    pub fn components_only_for_environment(&self, env: &str) -> Vec<&Component> {
        self.components
            .iter()
            .filter(|c| c.environment.as_deref() == Some(env))
            .collect()
    }

    pub fn find_component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }
}
