mod config;
mod deploy;
mod hooks;
mod manifest;
mod menu;
mod packages;
mod repo;
mod state;
mod utils;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use console::style;

use config::AppConfig;
use manifest::Manifest;
use menu::{ComponentSelection, MainMenuAction};
use state::State;

#[derive(Parser)]
#[command(
    name = "wayclone",
    about = "Declarative dotfile & package manager for Arch-based systems",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize with a dotfiles repository URL
    Init {
        /// Git repository URL
        url: String,
    },
    /// Pull latest changes from the dotfiles repository
    Update,
    /// Show current system status
    Status,
    /// Change the active shell
    Shell,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::new();
    config.ensure_dirs()?;

    match cli.command {
        Some(Commands::Init { url }) => cmd_init(&config, &url),
        Some(Commands::Update) => cmd_update(&config),
        Some(Commands::Status) => cmd_status(&config),
        Some(Commands::Shell) => cmd_shell(&config),
        None => cmd_interactive(&config),
    }
}

fn cmd_init(config: &AppConfig, url: &str) -> Result<()> {
    if config.repo_dir.exists() {
        bail!(
            "Repository already initialized at {}. Remove it first to re-initialize.",
            config.repo_dir.display()
        );
    }

    repo::clone_repo(url, &config.repo_dir)?;

    let mut state = State::default();
    state.repo_url = Some(url.to_string());
    state.save(&config.state_file)?;

    println!(
        "{}\nRun {} to manage components.",
        style("Repository initialized successfully!").green().bold(),
        style("wayclone").cyan()
    );
    Ok(())
}

fn cmd_update(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    repo::pull_repo(&config.repo_dir)?;
    println!("{}", style("Repository updated.").green());
    Ok(())
}

fn cmd_status(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    let state = State::load(&config.state_file)?;
    let manifest = Manifest::load(&config.manifest_path())?;
    menu::show_status(&manifest, &state);
    Ok(())
}

fn cmd_shell(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    let mut state = State::load(&config.state_file)?;
    let manifest = Manifest::load(&config.manifest_path())?;

    if let Some(new_shell) = menu::show_shell_menu(&manifest, state.active_shell.as_deref())? {
        if state.active_shell.as_deref() == Some(new_shell.as_str()) {
            println!("Shell is already set to {}.", style(&new_shell).cyan());
            return Ok(());
        }
        change_shell(config, &manifest, &mut state, &new_shell)?;
        state.save(&config.state_file)?;
    }
    Ok(())
}

fn cmd_interactive(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    let mut state = State::load(&config.state_file)?;
    let manifest = Manifest::load(&config.manifest_path())?;

    loop {
        println!();
        match menu::show_main_menu(&manifest)? {
            MainMenuAction::ManageComponents => {
                let active_shell = state.active_shell.clone();
                let components = manifest.components_for_shell(active_shell.as_deref());

                if let Some(selection) =
                    menu::show_components_menu(&components, &state)?
                {
                    apply_changes(config, &manifest, &mut state, &selection)?;
                    state.save(&config.state_file)?;
                }
            }
            MainMenuAction::ChangeShell => {
                if let Some(new_shell) =
                    menu::show_shell_menu(&manifest, state.active_shell.as_deref())?
                {
                    if state.active_shell.as_deref() != Some(new_shell.as_str()) {
                        change_shell(config, &manifest, &mut state, &new_shell)?;
                        state.save(&config.state_file)?;
                    } else {
                        println!("Shell is already set to {}.", style(&new_shell).cyan());
                    }
                }
            }
            MainMenuAction::UpdateRepo => {
                repo::pull_repo(&config.repo_dir)?;
                println!("{}", style("Repository updated.").green());
            }
            MainMenuAction::Status => {
                menu::show_status(&manifest, &state);
            }
            MainMenuAction::Exit => {
                println!("{}", style("Goodbye!").dim());
                break;
            }
        }
    }

    Ok(())
}

/// Apply component changes: uninstall disabled, install enabled, deploy dotfiles.
fn apply_changes(
    config: &AppConfig,
    manifest: &Manifest,
    state: &mut State,
    selection: &ComponentSelection,
) -> Result<()> {
    let needed = !selection.reinstall;

    // Phase 1: Uninstall newly-disabled components
    for name in &selection.disabled {
        if state.is_enabled(name) {
            if let Some(comp) = manifest.find_component(name) {
                println!(
                    "\n{} {}",
                    style("[-]").red().bold(),
                    style(&comp.name).red()
                );
                if !comp.packages.is_empty() {
                    packages::uninstall_packages(&comp.packages)?;
                }
                deploy::undeploy_component_files(comp, state)?;
            }
        }
        state.components.insert(name.clone(), false);
    }

    // Phase 2: Check if we need paru
    let needs_paru = selection.enabled.iter().any(|name| {
        manifest
            .find_component(name)
            .map(|c| c.packages.iter().any(|p| !p.starts_with("local:")))
            .unwrap_or(false)
    });
    if needs_paru {
        packages::ensure_paru()?;
    }

    // Phase 3: Install/update enabled components
    let active_shell = state.active_shell.clone();
    for name in &selection.enabled {
        if let Some(comp) = manifest.find_component(name) {
            let was_enabled = state.is_enabled(name);

            println!(
                "\n{} {}",
                style("[+]").green().bold(),
                style(&comp.name).cyan()
            );

            // Install packages
            if !comp.packages.is_empty() {
                packages::install_packages(&comp.packages, &config.repo_dir, needed)?;
            }

            // Post-install hooks (only for newly installed components)
            if !was_enabled && !comp.post_install.is_empty() {
                hooks::run_hooks(&comp.post_install, "post_install")?;
            }

            // Deploy dotfiles
            if !comp.files.is_empty() {
                deploy::deploy_component_files(
                    comp,
                    config,
                    state,
                    active_shell.as_deref(),
                )?;
            }

            // Post-deploy hooks
            if !comp.post_deploy.is_empty() {
                hooks::run_hooks(&comp.post_deploy, "post_deploy")?;
            }
        }
        state.components.insert(name.clone(), true);
    }

    println!(
        "\n{}",
        style("✓ Changes applied successfully!").green().bold()
    );
    Ok(())
}

/// Change the active shell: undeploy old shell components, deploy new ones,
/// and re-deploy global components to pick up overlay changes.
fn change_shell(
    config: &AppConfig,
    manifest: &Manifest,
    state: &mut State,
    new_shell: &str,
) -> Result<()> {
    let old_shell = state.active_shell.clone();

    println!(
        "\nChanging shell{} → {}",
        old_shell
            .as_ref()
            .map(|s| format!(": {}", style(s).red()))
            .unwrap_or_default(),
        style(new_shell).green().bold()
    );

    // 1. Undeploy and uninstall old shell-specific components
    if let Some(ref old) = old_shell {
        let old_components: Vec<_> = manifest
            .components
            .iter()
            .filter(|c| c.shell.as_deref() == Some(old.as_str()) && state.is_enabled(&c.name))
            .collect();

        for comp in old_components {
            println!(
                "  {} {} (shell: {})",
                style("[-]").red(),
                comp.name,
                old
            );
            if !comp.packages.is_empty() {
                packages::uninstall_packages(&comp.packages)?;
            }
            deploy::undeploy_component_files(comp, state)?;
            state.components.insert(comp.name.clone(), false);
        }
    }

    // 2. Set new shell
    state.active_shell = Some(new_shell.to_string());

    // 3. Install and deploy new shell-specific components (default or previously enabled)
    let new_components: Vec<_> = manifest
        .components
        .iter()
        .filter(|c| c.shell.as_deref() == Some(new_shell))
        .collect();

    // Check if paru is needed
    let needs_paru = new_components.iter().any(|c| {
        (c.default || state.components.get(&c.name).copied().unwrap_or(false))
            && c.packages.iter().any(|p| !p.starts_with("local:"))
    });
    if needs_paru {
        packages::ensure_paru()?;
    }

    for comp in &new_components {
        let should_enable =
            comp.default || state.components.get(&comp.name).copied().unwrap_or(false);
        if should_enable {
            println!(
                "  {} {} (shell: {})",
                style("[+]").green(),
                comp.name,
                new_shell
            );
            if !comp.packages.is_empty() {
                packages::install_packages(&comp.packages, &config.repo_dir, true)?;
            }
            if !comp.post_install.is_empty() {
                hooks::run_hooks(&comp.post_install, "post_install")?;
            }
            if !comp.files.is_empty() {
                deploy::deploy_component_files(comp, config, state, Some(new_shell))?;
            }
            if !comp.post_deploy.is_empty() {
                hooks::run_hooks(&comp.post_deploy, "post_deploy")?;
            }
            state.components.insert(comp.name.clone(), true);
        }
    }

    // 4. Re-deploy global components to pick up overlay changes
    let global_with_files: Vec<_> = manifest
        .components
        .iter()
        .filter(|c| c.shell.is_none() && state.is_enabled(&c.name) && !c.files.is_empty())
        .collect();

    if !global_with_files.is_empty() {
        println!("\n  Re-deploying global components for new overlay...");
        for comp in global_with_files {
            println!("  {} {} (overlay update)", style("[~]").yellow(), comp.name);
            deploy::redeploy_component_files(comp, config, state, Some(new_shell))?;
        }
    }

    println!(
        "\n{}",
        style(format!("✓ Shell changed to {}", new_shell))
            .green()
            .bold()
    );
    Ok(())
}

/// Ensure the repository is initialized.
fn ensure_initialized(config: &AppConfig) -> Result<()> {
    if !config.repo_dir.exists() {
        bail!(
            "No repository configured. Run '{}' first.",
            style("wayclone init <url>").cyan()
        );
    }
    Ok(())
}
