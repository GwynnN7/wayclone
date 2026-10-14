use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Result};
use console::style;
use dialoguer::Select;
use similar::{ChangeTag, TextDiff};

use crate::config::AppConfig;
use crate::manifest::{Component, FileEntry};
use crate::state::State;
use crate::utils::{
    collect_directory_files, copy_file, expand_env, hash_contents, read_file_contents,
    remove_path, resolve_file_source, FileSource,
};

/// Deploy all files for a component.
pub fn deploy_component_files(
    component: &Component,
    config: &AppConfig,
    state: &mut State,
    active_shell: Option<&str>,
) -> Result<()> {
    let mut deployed_files: Vec<String> = Vec::new();

    for file in component
        .files
        .iter()
        .filter(|file| file.applies_to_environment(active_shell))
    {
        let mut file_deployed = deploy_file_entry(file, config, state, active_shell)?;
        deployed_files.append(&mut file_deployed);
    }

    state
        .deployed
        .insert(component.name.clone(), deployed_files);
    Ok(())
}

/// Deploy a single file entry. Returns the list of deployed destination paths.
fn deploy_file_entry(
    file: &FileEntry,
    config: &AppConfig,
    state: &mut State,
    active_shell: Option<&str>,
) -> Result<Vec<String>> {
    let dest_expanded = expand_env(&file.dest);
    let dest = PathBuf::from(&dest_expanded);
    let source = resolve_file_source(&file.src, config, active_shell);

    match source {
        FileSource::Repo(url) => {
            // Destination must be a directory for repo sources
            if dest.exists() && dest.is_file() {
                bail!(
                    "Destination for repo source must be a directory, not a file: {}",
                    dest.display()
                );
            }
            if dest.exists() {
                crate::repo::pull_repo(&dest)?;
            } else {
                crate::repo::clone_repo(&url, &dest)?;
            }
            Ok(vec![dest_expanded])
        }
        FileSource::Local(src_path) => {
            if !src_path.exists() {
                bail!("Source not found: {}", src_path.display());
            }

            if src_path.is_dir() {
                deploy_directory(&file.src, &dest, file, config, state, active_shell)
            } else {
                deploy_single_file(&src_path, &dest, file, state)?;
                Ok(vec![dest.to_string_lossy().to_string()])
            }
        }
    }
}

/// Deploy a directory with environment override merging.
fn deploy_directory(
    src_key: &str,
    dest: &Path,
    file: &FileEntry,
    config: &AppConfig,
    state: &mut State,
    active_shell: Option<&str>,
) -> Result<Vec<String>> {
    let files = collect_directory_files(src_key, config, active_shell)?;
    let mut deployed = Vec::new();

    for (source_path, relative) in &files {
        let file_dest = dest.join(relative);
        deploy_single_file(source_path, &file_dest, file, state)?;
        deployed.push(file_dest.to_string_lossy().to_string());
    }

    Ok(deployed)
}

/// Deploy a single file with diff checking.
fn deploy_single_file(
    src: &Path,
    dest: &Path,
    file: &FileEntry,
    state: &mut State,
) -> Result<()> {
    let src_contents = std::fs::read(src)?;
    let src_hash = hash_contents(&src_contents);
    let dest_key = dest.to_string_lossy().to_string();

    if dest.exists() && file.check {
        let dest_contents = read_file_contents(dest, file.sudo)?;
        let dest_hash = hash_contents(&dest_contents);

        // Check if local file was modified since last deploy AND differs from upstream
        let last_deployed_hash = state.hashes.get(&dest_key).cloned();

        let local_modified = match &last_deployed_hash {
            Some(h) => *h != dest_hash,
            None => true, // No record = assume potentially modified
        };

        if local_modified && dest_hash != src_hash {
            // Conflict: local file modified AND upstream changed
            println!(
                "\n  {} has been modified locally",
                style(dest.display()).yellow()
            );
            show_diff(&dest_contents, &src_contents, dest);

            match prompt_diff_action()? {
                DiffAction::Overwrite => {
                    copy_file(src, dest, file.sudo)?;
                    state.hashes.insert(dest_key, src_hash);
                }
                DiffAction::KeepLocal => {
                    // Keep local version, update hash to track current local state
                    state.hashes.insert(dest_key, dest_hash);
                }
                DiffAction::KeepLocalCopyNew => {
                    let new_path = PathBuf::from(format!("{}.new", dest.display()));
                    copy_file(src, &new_path, file.sudo)?;
                    state.hashes.insert(dest_key, dest_hash);
                }
            }
            return Ok(());
        }

        // If hashes match (no change upstream or locally), skip
        if dest_hash == src_hash {
            state.hashes.insert(dest_key, src_hash);
            return Ok(());
        }
    }

    // No conflict or check=false: just copy
    copy_file(src, dest, file.sudo)?;
    state.hashes.insert(dest_key, src_hash);
    Ok(())
}

/// Show a unified diff between local and upstream content.
fn show_diff(local: &[u8], upstream: &[u8], path: &Path) {
    let local_str = String::from_utf8_lossy(local);
    let upstream_str = String::from_utf8_lossy(upstream);

    let diff = TextDiff::from_lines(local_str.as_ref(), upstream_str.as_ref());

    println!("  --- {} (local)", path.display());
    println!("  +++ {} (upstream)", path.display());

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => print!("  {}{}", style("-").red(), style(&change).red()),
            ChangeTag::Insert => print!("  {}{}", style("+").green(), style(&change).green()),
            ChangeTag::Equal => print!("   {}", change),
        }
    }
    println!();
}

enum DiffAction {
    Overwrite,
    KeepLocal,
    KeepLocalCopyNew,
}

fn prompt_diff_action() -> Result<DiffAction> {
    let choices = &[
        "Overwrite with upstream version",
        "Keep local version",
        "Keep local, save upstream as .new",
    ];
    let selection = Select::new()
        .with_prompt("How to resolve?")
        .items(choices)
        .default(0)
        .interact()?;
    Ok(match selection {
        0 => DiffAction::Overwrite,
        1 => DiffAction::KeepLocal,
        _ => DiffAction::KeepLocalCopyNew,
    })
}

/// Check if a file path requires sudo based on the component's file entries.
fn path_needs_sudo(component: &Component, file_path: &str) -> bool {
    component.files.iter().any(|f| {
        let dest = expand_env(&f.dest);
        file_path.starts_with(&dest) && f.sudo
    })
}

/// Clean up empty parent directories up to safe system/user boundaries.
fn cleanup_empty_parents(file_path: &Path, sudo: bool) {
    let protected: HashSet<PathBuf> = [
        "/",
        "/etc",
        "/usr",
        "/usr/bin",
        "/usr/lib",
        "/usr/local",
        "/usr/local/bin",
        "/usr/share",
        &expand_env("$HOME"),
        &expand_env("$XDG_CONFIG_HOME"),
        &expand_env("$XDG_DATA_HOME"),
        &expand_env("$XDG_STATE_HOME"),
        &expand_env("$XDG_CACHE_HOME"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();

    let mut parent = file_path.parent();
    while let Some(dir) = parent {
        if protected.contains(dir) || !dir.exists() {
            break;
        }

        let removed = if sudo {
            Command::new("sudo")
                .args(["rmdir", "--ignore-fail-on-non-empty"])
                .arg(dir)
                .status()
                .map(|s| s.success() && !dir.exists())
                .unwrap_or(false)
        } else {
            std::fs::remove_dir(dir).is_ok()
        };

        if removed {
            parent = dir.parent();
        } else {
            break;
        }
    }
}

/// Remove all deployed files for a component.
pub fn undeploy_component_files(component: &Component, state: &mut State) -> Result<()> {
    if let Some(files) = state.deployed.remove(&component.name) {
        for file_path in &files {
            let path = PathBuf::from(file_path);
            let needs_sudo = path_needs_sudo(component, file_path);
            if path.exists() {
                if let Err(e) = remove_path(&path, needs_sudo) {
                    eprintln!("  Warning: failed to remove {}: {}", path.display(), e);
                }
            }
            state.hashes.remove(file_path);
            cleanup_empty_parents(&path, needs_sudo);
        }
    }
    Ok(())
}

/// Re-deploy a component's files, cleaning up orphaned files from the previous deployment.
pub fn redeploy_component_files(
    component: &Component,
    config: &AppConfig,
    state: &mut State,
    active_shell: Option<&str>,
) -> Result<()> {
    let old_deployed = state
        .deployed
        .get(&component.name)
        .cloned()
        .unwrap_or_default();

    // Deploy new files
    deploy_component_files(component, config, state, active_shell)?;

    // Find orphaned files (were deployed before but not in new deployment)
    let new_deployed = state
        .deployed
        .get(&component.name)
        .cloned()
        .unwrap_or_default();
    let new_set: HashSet<_> = new_deployed.iter().collect();

    for old_file in &old_deployed {
        if !new_set.contains(old_file) {
            let path = PathBuf::from(old_file);
            let needs_sudo = path_needs_sudo(component, old_file);
            if path.exists() {
                if let Err(e) = remove_path(&path, needs_sudo) {
                    eprintln!(
                        "  Warning: failed to remove orphaned file {}: {}",
                        path.display(),
                        e
                    );
                }
            }
            state.hashes.remove(old_file);
            cleanup_empty_parents(&path, needs_sudo);
        }
    }

    Ok(())
}
