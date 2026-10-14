use std::path::PathBuf;
use anyhow::Result;

pub struct AppConfig {
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    pub repo_dir: PathBuf,
    pub state_file: PathBuf,
}

impl AppConfig {
    pub fn new() -> Self {
        let home = dirs::home_dir().expect("Cannot determine home directory");
        let state_dir = dirs::state_dir()
            .unwrap_or_else(|| home.join(".local/state"))
            .join("wayclone");
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| home.join(".local/share"))
            .join("wayclone");
        let repo_dir = data_dir.join("repo");
        let state_file = state_dir.join("state.toml");
        Self {
            state_dir,
            data_dir,
            repo_dir,
            state_file,
        }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        std::fs::create_dir_all(&self.data_dir)?;
        Ok(())
    }

    pub fn dotfiles_dir(&self) -> PathBuf {
        self.repo_dir.join("dotfiles")
    }

    pub fn packages_dir(&self) -> PathBuf {
        self.repo_dir.join("packages")
    }

    pub fn overlay_dir(&self, shell: &str) -> PathBuf {
        self.repo_dir.join(format!("overlays/{}", shell))
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.repo_dir.join("manifest.toml")
    }
}
