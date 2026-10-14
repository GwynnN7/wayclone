use std::process::Command;

use anyhow::{Context, Result};

pub fn run_hooks(hooks: &[String], label: &str) -> Result<()> {
    for (i, hook) in hooks.iter().enumerate() {
        println!(
            "  Running {} hook [{}/{}]: {}",
            label,
            i + 1,
            hooks.len(),
            hook
        );
        let status = Command::new("sh")
            .args(["-c", hook])
            .status()
            .with_context(|| format!("Failed to run hook: {}", hook))?;
        if !status.success() {
            eprintln!("  Warning: hook exited with non-zero status: {}", hook);
        }
    }
    Ok(())
}
