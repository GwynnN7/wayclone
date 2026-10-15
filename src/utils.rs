use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::config::AppConfig;

static COPY_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Resolved source for a file entry.
pub enum FileSource {
    /// A local path (file or directory) in the dotfiles/overrides tree.
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
/// Handles "repo:user/repo" -> Repo, otherwise Local with override resolution.
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

/// Resolve a local dotfile path with environment override priority.
/// Checks overrides/{environment}/{src} first, then dotfiles/{src}.
fn resolve_local_path(src: &str, config: &AppConfig, active_shell: Option<&str>) -> PathBuf {
    if let Some(environment) = active_shell {
        let override_path = config.override_dir(environment).join(src);
        if override_path.exists() {
            return override_path;
        }
    }
    config.dotfiles_dir().join(src)
}

/// For directory sources, collect all files with environment override merging.
/// Returns (absolute_source_path, relative_path_within_src) pairs.
pub fn collect_directory_files(
    src: &str,
    config: &AppConfig,
    active_shell: Option<&str>,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    let base_dir = config.dotfiles_dir().join(src);
    let override_dir =
        active_shell.map(|environment| config.override_dir(environment).join(src));

    // Use HashMap to merge: override files replace base files by relative path.
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

    // Replace base files with environment override files.
    if let Some(ref override_path) = override_dir {
        if override_path.exists() && override_path.is_dir() {
            for entry in WalkDir::new(override_path)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    if let Ok(relative) = entry.path().strip_prefix(override_path) {
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

/// Build a same-directory temporary path so replacement can use an atomic rename.
fn temporary_copy_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("waysync-file");
    let sequence = COPY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dest.with_file_name(format!(
        ".{}.waysync-{}-{}.tmp",
        name,
        std::process::id(),
        sequence
    ))
}

/// Copy a file atomically, optionally with sudo.
pub fn copy_file(src: &Path, dest: &Path, sudo: bool) -> Result<()> {
    let temp = temporary_copy_path(dest);

    if sudo {
        if let Some(parent) = dest.parent() {
            run_cmd("sudo", &["mkdir", "-p", &parent.to_string_lossy()])?;
        }
        // Copy beside the destination, then rename it into place. Renaming replaces
        // an executable even while its previous inode is still running.
        let copy_result = (|| -> Result<()> {
            run_cmd(
                "sudo",
                &[
                    "cp",
                    "--preserve=mode",
                    &src.to_string_lossy(),
                    &temp.to_string_lossy(),
                ],
            )?;
            run_cmd(
                "sudo",
                &["mv", "-f", &temp.to_string_lossy(), &dest.to_string_lossy()],
            )
        })();
        if let Err(error) = copy_result {
            let _ = Command::new("sudo").args(["rm", "-f"]).arg(&temp).status();
            return Err(error);
        }
        // sudoers drop-in files must be 0440 and owned by root:root
        if dest.starts_with("/etc/sudoers.d") {
            run_cmd("sudo", &["chmod", "0440", &dest.to_string_lossy()])?;
        }
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
        let copy_result = (|| -> Result<()> {
            fs::copy(src, &temp)
                .with_context(|| format!("Failed to copy {} -> {}", src.display(), temp.display()))?;
            fs::rename(&temp, dest).with_context(|| {
                format!("Failed to replace {} with {}", dest.display(), temp.display())
            })
        })();
        if let Err(error) = copy_result {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_config() -> AppConfig {
        let unique = format!(
            "waysync-utils-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let state_dir = root.join("state");
        let data_dir = root.join("data");

        AppConfig {
            state_file: state_dir.join("state.toml"),
            repo_dir: data_dir.join("repo"),
            state_dir,
            data_dir,
        }
    }

    fn local_path(source: FileSource) -> PathBuf {
        match source {
            FileSource::Local(path) => path,
            FileSource::Repo(_) => panic!("expected a local file source"),
        }
    }

    #[test]
    fn environment_override_replaces_a_single_base_file() {
        let config = test_config();
        let base = config.dotfiles_dir().join("theme.toml");
        let environment_override = config.override_dir("caelestia").join("theme.toml");
        std::fs::create_dir_all(base.parent().expect("base parent")).unwrap();
        std::fs::create_dir_all(environment_override.parent().expect("override parent")).unwrap();
        std::fs::write(&base, "base").unwrap();
        std::fs::write(&environment_override, "caelestia").unwrap();

        assert_eq!(
            local_path(resolve_file_source("theme.toml", &config, None)),
            base
        );
        assert_eq!(
            local_path(resolve_file_source("theme.toml", &config, Some("caelestia"))),
            environment_override
        );
        assert_eq!(
            local_path(resolve_file_source("theme.toml", &config, Some("noctalia"))),
            config.dotfiles_dir().join("theme.toml")
        );

        std::fs::remove_dir_all(config.data_dir.parent().expect("test root")).unwrap();
    }

    #[test]
    fn environment_override_merges_directories_by_relative_path() {
        let config = test_config();
        let base = config.dotfiles_dir().join("hypr");
        let environment_override = config.override_dir("caelestia").join("hypr");
        std::fs::create_dir_all(base.join("nested")).unwrap();
        std::fs::create_dir_all(environment_override.join("nested")).unwrap();
        std::fs::write(base.join("base-only.conf"), "base-only").unwrap();
        std::fs::write(base.join("replace.conf"), "base").unwrap();
        std::fs::write(base.join("nested/base-only.conf"), "nested-base").unwrap();
        std::fs::write(environment_override.join("replace.conf"), "caelestia").unwrap();
        std::fs::write(
            environment_override.join("nested/override-only.conf"),
            "nested-override",
        )
        .unwrap();

        let files: BTreeMap<_, _> = collect_directory_files("hypr", &config, Some("caelestia"))
            .unwrap()
            .into_iter()
            .map(|(source, relative)| (relative, std::fs::read_to_string(source).unwrap()))
            .collect();

        assert_eq!(
            files,
            BTreeMap::from([
                (PathBuf::from("base-only.conf"), "base-only".to_string()),
                (PathBuf::from("replace.conf"), "caelestia".to_string()),
                (
                    PathBuf::from("nested/base-only.conf"),
                    "nested-base".to_string()
                ),
                (
                    PathBuf::from("nested/override-only.conf"),
                    "nested-override".to_string()
                ),
            ])
        );

        std::fs::remove_dir_all(config.data_dir.parent().expect("test root")).unwrap();
    }

    #[test]
    fn copy_file_replaces_an_existing_file() {
        let root = std::env::temp_dir().join(format!(
            "waysync-copy-file-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        std::fs::write(&source, "new content").unwrap();
        std::fs::write(&destination, "old content").unwrap();

        copy_file(&source, &destination, false).unwrap();

        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new content");
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .all(|entry| !entry.unwrap().file_name().to_string_lossy().contains(".waysync-"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
