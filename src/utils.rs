use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::config::AppConfig;

/// Resolved source for a file entry.
pub enum FileSource {
    /// A local path (file or directory) in the dotfiles/overlays tree.
    Local(PathBuf),
    /// A git repository URL to clone.
    Repo(String),
}

/// Expand environment variables in a string.
/// Handles $VAR, ${VAR}, and ${VAR:-default} patterns.
pub fn expand_env(s: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let get_var = |name: &str| -> String {
            std::env::var(name).unwrap_or_else(|_| match name {
                "XDG_CONFIG_HOME" => format!("{}/.config", home),
                "XDG_DATA_HOME" => format!("{}/.local/share", home),
                "XDG_STATE_HOME" => format!("{}/.local/state", home),
                "XDG_CACHE_HOME" => format!("{}/.cache", home),
                "XDG_BIN_HOME" => format!("{}/.local/bin", home),
                "XDG_CONFIG_SERVICE" => format!("{}/.config/systemd/user", home),
                "HOME" => home.clone(),
                _ => String::new(),
            })
        };

    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_expr = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    var_expr.push(c);
                }
                if let Some(idx) = var_expr.find(":-") {
                    let var_name = &var_expr[..idx];
                    let default = &var_expr[idx + 2..];
                    let val = std::env::var(var_name)
                        .unwrap_or_else(|_| expand_env(default));
                    result.push_str(&val);
                } else {
                    result.push_str(&get_var(&var_expr));
                }
            } else {
                // Read var name: alphanumeric + underscore
                let mut var_name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        var_name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if var_name.is_empty() {
                    result.push('$');
                } else {
                    result.push_str(&get_var(&var_name));
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Compute SHA256 hash of a byte slice.
pub fn hash_contents(contents: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents);
    hex::encode(hasher.finalize())
}

/// Resolve a file source string to a FileSource.
/// Handles "repo:user/repo" -> Repo, otherwise Local with overlay resolution.
pub fn resolve_file_source(
    src: &str,
    config: &AppConfig,
    active_shell: Option<&str>,
) -> FileSource {
    if let Some(repo) = src.strip_prefix("repo:") {
        FileSource::Repo(format!("https://github.com/{}", repo))
    } else {
        FileSource::Local(resolve_local_path(src, config, active_shell))
    }
}

/// Resolve a local dotfile path with overlay priority.
/// Checks overlays/{shell}/{src} first, then dotfiles/{src}.
fn resolve_local_path(src: &str, config: &AppConfig, active_shell: Option<&str>) -> PathBuf {
    if let Some(shell) = active_shell {
        let overlay_path = config.overlay_dir(shell).join(src);
        if overlay_path.exists() {
            return overlay_path;
        }
    }
    config.dotfiles_dir().join(src)
}

/// For directory sources, collect all files with overlay merging.
/// Returns (absolute_source_path, relative_path_within_src) pairs.
pub fn collect_directory_files(
    src: &str,
    config: &AppConfig,
    active_shell: Option<&str>,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let base_dir = config.dotfiles_dir().join(src);
    let overlay_dir = active_shell.map(|s| config.overlay_dir(s).join(src));

    // Use HashMap to merge: overlay files override base files by relative path
    let mut files: HashMap<PathBuf, PathBuf> = HashMap::new();

    // Collect base files
    if base_dir.exists() && base_dir.is_dir() {
        for entry in WalkDir::new(&base_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Ok(relative) = entry.path().strip_prefix(&base_dir) {
                    files.insert(relative.to_path_buf(), entry.path().to_path_buf());
                }
            }
        }
    }

    // Override with overlay files
    if let Some(ref overlay) = overlay_dir {
        if overlay.exists() && overlay.is_dir() {
            for entry in WalkDir::new(overlay)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    if let Ok(relative) = entry.path().strip_prefix(overlay) {
                        files.insert(relative.to_path_buf(), entry.path().to_path_buf());
                    }
                }
            }
        }
    }

    let mut result: Vec<_> = files.into_iter().map(|(rel, src)| (src, rel)).collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(result)
}

/// Copy a file or directory, optionally with sudo.
pub fn copy_file(src: &Path, dest: &Path, sudo: bool) -> Result<()> {
    if sudo {
        if let Some(parent) = dest.parent() {
            run_cmd("sudo", &["mkdir", "-p", &parent.to_string_lossy()])?;
        }
        run_cmd("sudo", &["cp", "-p", &src.to_string_lossy(), &dest.to_string_lossy()])?;
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
        fs::copy(src, dest)
            .with_context(|| format!("Failed to copy {} -> {}", src.display(), dest.display()))?;
    }
    Ok(())
}

/// Remove a file or directory, optionally with sudo.
pub fn remove_path(path: &Path, sudo: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if sudo {
        run_cmd("sudo", &["rm", "-rf", &path.to_string_lossy()])?;
    } else if path.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
    } else {
        fs::remove_file(path)
            .with_context(|| format!("Failed to remove file: {}", path.display()))?;
    }
    Ok(())
}

/// Read file contents, using sudo if needed.
pub fn read_file_contents(path: &Path, sudo: bool) -> Result<Vec<u8>> {
    if sudo {
        let output = Command::new("sudo")
            .args(["cat"])
            .arg(path)
            .output()
            .with_context(|| format!("Failed to read {} with sudo", path.display()))?;
        if !output.status.success() {
            bail!("sudo cat failed for {}", path.display());
        }
        Ok(output.stdout)
    } else {
        Ok(fs::read(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?)
    }
}

/// Run a command and check its exit status.
fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to run: {} {}", program, args.join(" ")))?;
    if !status.success() {
        bail!("{} {} failed with exit code {:?}", program, args.join(" "), status.code());
    }
    Ok(())
}
