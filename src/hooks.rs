use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::manifest::HookEntry;

pub fn run_hooks(
    hooks: &[HookEntry],
    label: &str,
    active_env: Option<&str>,
    repo_dir: &Path,
) -> Result<()> {
    let applicable: Vec<&HookEntry> = hooks
        .iter()
        .filter(|h| h.applies_to_environment(active_env))
        .collect();

    for (i, hook) in applicable.iter().enumerate() {
        let cmd_str = hook.cmd();
        println!(
            "  Running {} hook [{}/{}]: {}",
            label,
            i + 1,
            applicable.len(),
            cmd_str
        );
        let status = Command::new("sh")
            .args(["-c", cmd_str])
            .env("WAYSYNC_REPO_DIR", repo_dir)
            .env("WAYSYNC_ENV", active_env.unwrap_or(""))
            .status()
            .with_context(|| format!("Failed to run hook: {}", cmd_str))?;
        if !status.success() {
            eprintln!("  Warning: hook exited with non-zero status: {}", cmd_str);
        }
    }
    Ok(())
}