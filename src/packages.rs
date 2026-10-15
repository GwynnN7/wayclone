use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use console::style;
use dialoguer::Select;

/// Parsed package specification.
pub enum PackageSpec {
    /// Standard package (native or AUR), installed via `paru -S`.
    Standard(String),
    /// Explicit AUR package (`aur:<name>`), installed via `paru -S --aur`.
    Aur(String),
    /// Local PKGBUILD package (`local:<name>`), located in `packages/{name}/`.
    Local(String),
}

/// Parse a package spec string.
pub fn parse_package_spec(spec: &str) -> PackageSpec {
    if let Some(name) = spec.strip_prefix("local:") {
        PackageSpec::Local(name.to_string())
    } else if let Some(name) = spec.strip_prefix("aur:") {
        PackageSpec::Aur(name.to_string())
    } else {
        PackageSpec::Standard(spec.to_string())
    }
}

/// Extract actual `pkgname` value(s) from `packages/{dir_name}/PKGBUILD`.
/// Falls back to `dir_name` if the PKGBUILD cannot be read or parsed.
fn resolve_local_pkgnames(repo_path: &Path, dir_name: &str) -> Vec<String> {
    let pkgbuild_path = repo_path.join("packages").join(dir_name).join("PKGBUILD");
    if let Ok(content) = std::fs::read_to_string(&pkgbuild_path) {
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("pkgname=") {
                let cleaned = rest.trim_matches(|c| c == '(' || c == ')' || c == '"' || c == '\'');
                let names: Vec<String> = cleaned
                    .split_whitespace()
                    .map(|s| s.trim_matches(|c| c == '"' || c == '\'').to_string())
                    .filter(|s| !s.is_empty() && !s.contains("\$"))
                    .collect();
                if !names.is_empty() {
                    return names;
                }
            }
        }
    }
    vec![dir_name.to_string()]
}

/// Ask user how to proceed if an interactive package installation still fails.
fn prompt_package_failure(label: &str) -> Result<bool> {
    let choices = &[
        "Skip this package step and continue deployment",
        "Retry interactive installation",
        "Abort deployment",
    ];
    let selection = Select::new()
        .with_prompt(format!("Package step failed for '{}'. How to proceed?", label))
        .items(choices)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok(false), // Skip and continue
        1 => Ok(true),  // Retry
        _ => bail!("Deployment aborted by user after package failure: {}", label),
    }
}

/// Unified runner for all package operations (Standard, AUR, and Local):
/// 1. Attempts automatic execution with `--noconfirm` (`auto_args`).
/// 2. If that fails (e.g. conflict or provider prompt), automatically drops into
///    interactive mode (`interactive_args`, without `--noconfirm`).
/// 3. If interactive mode also fails, prompts the user to Skip, Retry, or Abort.
fn run_with_interactive_fallback(
    label: &str,
    program: &str,
    auto_args: &[String],
    interactive_args: &[String],
    current_dir: Option<&Path>,
) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(auto_args);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }

    let status = cmd
        .status()
        .with_context(|| format!("Failed to run {} for {}", program, label))?;

    if status.success() {
        return Ok(());
    }

    println!(
        "  {} Automatic install failed for '{}'. Retrying interactively...",
        style("!").yellow().bold(),
        style(label).yellow()
    );

    loop {
        let mut retry_cmd = Command::new(program);
        retry_cmd.args(interactive_args);
        if let Some(dir) = current_dir {
            retry_cmd.current_dir(dir);
        }

        let retry = retry_cmd
            .status()
            .with_context(|| format!("Failed to run interactive {} for {}", program, label))?;

        if retry.success() {
            return Ok(());
        }

        if !prompt_package_failure(label)? {
            println!("  {} Skipping {}...", style("→").dim(), label);
            return Ok(());
        }
    }
}

/// Ensure paru (AUR helper) is installed.
pub fn ensure_paru() -> Result<()> {
    if Command::new("paru")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Ok(());
    }

    println!("paru not found, installing paru-bin...");
    let tmp = Path::new("/tmp/waysync-paru-build");
    let _ = std::fs::remove_dir_all(tmp);
    let status = Command::new("git")
        .args(["clone", "https://aur.archlinux.org/paru-bin.git"])
        .arg(tmp)
        .status()
        .context("Failed to clone paru-bin")?;
    if !status.success() {
        bail!("Failed to clone paru-bin from AUR");
    }

    let auto_args = vec!["-si".to_string(), "--noconfirm".to_string()];
    let interactive_args = vec!["-si".to_string()];
    let res = run_with_interactive_fallback(
        "paru-bin",
        "makepkg",
        &auto_args,
        &interactive_args,
        Some(tmp),
    );
    let _ = std::fs::remove_dir_all(tmp);
    res?;

    println!("paru installed successfully.");
    Ok(())
}

/// Check if an exact package name is installed via pacman (`pacman -Qq`).
fn is_exact_package_installed(name: &str) -> bool {
    Command::new("pacman")
        .args(["-Qq", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Install packages. `repo_path` is the dotfiles repo root (for local PKGBUILDs).
pub fn install_packages(packages: &[String], repo_path: &Path, needed: bool) -> Result<()> {
    let mut standard = Vec::new();
    let mut aur = Vec::new();
    let mut local = Vec::new();

    for spec in packages {
        match parse_package_spec(spec) {
            PackageSpec::Standard(name) => standard.push(name),
            PackageSpec::Aur(name) => aur.push(name),
            PackageSpec::Local(name) => local.push(name),
        }
    }

    // 1. Explicit AUR packages (`aur:<name>`)
    for pkg in aur {
        if needed && is_exact_package_installed(&pkg) {
            println!("  {} (AUR) is already installed, skipping", pkg);
            continue;
        }

        let label = format!("aur:{}", pkg);
        println!("  Installing AUR package: {}", pkg);

        let mut base_args = vec!["-S".to_string(), "--aur".to_string()];
        if needed {
            base_args.push("--needed".to_string());
        }

        let mut auto_args = base_args.clone();
        auto_args.push("--noconfirm".to_string());
        auto_args.push(pkg.clone());

        let mut interactive_args = base_args;
        interactive_args.push(pkg);

        run_with_interactive_fallback(&label, "paru", &auto_args, &interactive_args, None)?;
    }

    // 2. Standard packages (native or AUR via `paru -S`)
    if !standard.is_empty() {
        let label = standard.join(", ");
        println!("  Installing packages: {}", label);

        let mut base_args = vec!["-S".to_string()];
        if needed {
            base_args.push("--needed".to_string());
        }

        let mut auto_args = base_args.clone();
        auto_args.push("--noconfirm".to_string());
        auto_args.extend(standard.clone());

        let mut interactive_args = base_args;
        interactive_args.extend(standard);

        run_with_interactive_fallback(&label, "paru", &auto_args, &interactive_args, None)?;
    }

    // 3. Local PKGBUILD packages (`local:<name>`) built via `paru -Ui`
    for name in local {
        let pkgnames = resolve_local_pkgnames(repo_path, &name);
        if needed && pkgnames.iter().all(|p| is_exact_package_installed(p)) {
            println!("  {} is already installed, skipping", name);
            continue;
        }

        let pkg_dir = repo_path.join("packages").join(&name);
        if !pkg_dir.exists() {
            bail!("Local package directory not found: {}", pkg_dir.display());
        }

        let label = format!("local:{}", name);
        println!("  Building local package: {}", name);

        let auto_args = vec!["-Ui".to_string(), "--noconfirm".to_string()];
        let interactive_args = vec!["-Ui".to_string()];

        run_with_interactive_fallback(
            &label,
            "paru",
            &auto_args,
            &interactive_args,
            Some(&pkg_dir),
        )?;
    }

    Ok(())
}

/// Uninstall packages. Silently skips packages that aren't installed.
pub fn uninstall_packages(packages: &[String], repo_path: &Path) -> Result<()> {
    let mut names = Vec::new();
    for spec in packages {
        match parse_package_spec(spec) {
            PackageSpec::Standard(name) | PackageSpec::Aur(name) => {
                if is_exact_package_installed(&name) {
                    names.push(name);
                }
            }
            PackageSpec::Local(dir_name) => {
                for pkg in resolve_local_pkgnames(repo_path, &dir_name) {
                    if is_exact_package_installed(&pkg) {
                        names.push(pkg);
                    }
                }
            }
        }
    }

    if names.is_empty() {
        return Ok(());
    }

    let label = format!("uninstall ({})", names.join(", "));
    println!("  Uninstalling packages: {}", names.join(", "));

    let mut auto_args = vec![
        "pacman".to_string(),
        "-Rns".to_string(),
        "--noconfirm".to_string(),
    ];
    auto_args.extend(names.clone());

    let mut interactive_args = vec!["pacman".to_string(), "-Rns".to_string()];
    interactive_args.extend(names);

    run_with_interactive_fallback(&label, "sudo", &auto_args, &interactive_args, None)
}