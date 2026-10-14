use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Parsed package specification.
pub enum PackageSpec {
    /// Standard package (native or AUR), installed via paru.
    Standard(String),
    /// Local PKGBUILD package, located in packages/{name}/.
    Local(String),
}

/// Parse a package spec string.
pub fn parse_package_spec(spec: &str) -> PackageSpec {
    if let Some(name) = spec.strip_prefix("local:") {
        PackageSpec::Local(name.to_string())
    } else {
        PackageSpec::Standard(spec.to_string())
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
    let tmp = "/tmp/wayclone-paru-build";
    let _ = std::fs::remove_dir_all(tmp);
    let status = Command::new("git")
        .args(["clone", "https://aur.archlinux.org/paru-bin.git", tmp])
        .status()
        .context("Failed to clone paru-bin")?;
    if !status.success() {
        bail!("Failed to clone paru-bin from AUR");
    }

    let status = Command::new("makepkg")
        .args(["-si", "--noconfirm"])
        .current_dir(tmp)
        .status()
        .context("Failed to build paru")?;
    let _ = std::fs::remove_dir_all(tmp);
    if !status.success() {
        bail!("Failed to install paru");
    }

    println!("paru installed successfully.");
    Ok(())
}

/// Check if a package is installed via pacman.
fn is_package_installed(name: &str) -> bool {
    Command::new("pacman")
        .args(["-Qi", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Install packages. `repo_path` is the dotfiles repo root (for local PKGBUILDs).
/// If `needed` is true, already-installed packages are skipped.
pub fn install_packages(packages: &[String], repo_path: &Path, needed: bool) -> Result<()> {
    let mut standard = Vec::new();
    let mut local = Vec::new();

    for spec in packages {
        match parse_package_spec(spec) {
            PackageSpec::Standard(name) => standard.push(name),
            PackageSpec::Local(name) => local.push(name),
        }
    }

    // Install standard packages via paru
    if !standard.is_empty() {
        let mut args = vec!["-S".to_string(), "--noconfirm".to_string()];
        if needed {
            args.push("--needed".to_string());
        }
        args.extend(standard.clone());

        println!("  Installing packages: {}", standard.join(", "));
        let status = Command::new("paru")
            .args(&args)
            .status()
            .context("Failed to run paru")?;
        if !status.success() {
            bail!("paru install failed for: {}", standard.join(", "));
        }
    }

    // Install local packages via makepkg
    for name in local {
        if needed && is_package_installed(&name) {
            println!("  {} is already installed, skipping", name);
            continue;
        }

        let pkg_dir = repo_path.join("packages").join(&name);
        if !pkg_dir.exists() {
            bail!("Local package directory not found: {}", pkg_dir.display());
        }

        println!("  Building local package: {}", name);
        let status = Command::new("makepkg")
            .args(["-si", "--noconfirm"])
            .current_dir(&pkg_dir)
            .status()
            .with_context(|| format!("Failed to build local package: {}", name))?;
        if !status.success() {
            bail!("makepkg failed for local package: {}", name);
        }
    }

    Ok(())
}

/// Uninstall packages. Silently skips packages that aren't installed.
pub fn uninstall_packages(packages: &[String]) -> Result<()> {
    let names: Vec<String> = packages
        .iter()
        .map(|spec| match parse_package_spec(spec) {
            PackageSpec::Standard(name) | PackageSpec::Local(name) => name,
        })
        .filter(|name| is_package_installed(name))
        .collect();

    if names.is_empty() {
        return Ok(());
    }

    println!("  Uninstalling packages: {}", names.join(", "));
    let status = Command::new("sudo")
        .args(["pacman", "-Rns", "--noconfirm"])
        .args(&names)
        .status()
        .context("Failed to uninstall packages")?;
    if !status.success() {
        eprintln!("  Warning: some packages could not be uninstalled");
    }
    Ok(())
}
