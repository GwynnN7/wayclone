use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Clone a git repository to the given path.
pub fn clone_repo(url: &str, path: &Path) -> Result<()> {
    println!("Cloning {} ...", url);
    if path.exists() {
        bail!("Repository already exists at {}", path.display());
    }
    let status = Command::new("git")
        .args(["clone", url])
        .arg(path)
        .status()
        .context("Failed to run git clone")?;
    if !status.success() {
        bail!("git clone failed for {}", url);
    }
    Ok(())
}

/// Pull the latest changes in a repository.
pub fn pull_repo(path: &Path) -> Result<()> {
    println!("Updating repository at {} ...", path.display());
    let status = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(path)
        .status()
        .context("Failed to run git pull")?;
    if !status.success() {
        bail!("git pull failed at {}", path.display());
    }
    Ok(())
}
