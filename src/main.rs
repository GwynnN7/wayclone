mod config;
mod deploy;
mod hooks;
mod manifest;
mod menu;
mod packages;
mod repo;
mod state;
mod utils;

use std::collections::HashSet;

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
    /// Pull latest changes from the dotfiles repository and sync enabled components
    Update,
    /// Show current system status
    Status,
    /// Change the active environment (alias: shell)
    #[command(alias = "shell")]
    Env,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::new();
    config.ensure_dirs()?;

    match cli.command {
        Some(Commands::Init { url }) => cmd_init(&config, &url),
        Some(Commands::Update) => cmd_update(&config),
        Some(Commands::Status) => cmd_status(&config),
        Some(Commands::Env) => cmd_env(&config),
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

    if config.manifest_path().exists() {
        if let Ok(manifest) = Manifest::load(&config.manifest_path()) {
            if !manifest.available_environments().is_empty() {
                println!("\n{}", style("Select an initial environment:").cyan().bold());
                if let Some(env_choice) = menu::show_environment_menu(&manifest, None)? {
                    state.active_environment = env_choice;
                }
            }
        }
    }

    state.save(&config.state_file)?;

    println!(
        "\n{}\nRun {} to manage components.",
        style("Repository initialized successfully!").green().bold(),
        style("wayclone").cyan()
    );
    Ok(())
}

fn cmd_update(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    repo::pull_repo(&config.repo_dir)?;
    println!("{}", style("Repository updated.").green());

    let mut state = State::load(&config.state_file)?;
    let manifest = Manifest::load(&config.manifest_path())?;
    sync_enabled_components(config, &manifest, &mut state)?;
    state.save(&config.state_file)?;
    Ok(())
}

fn cmd_status(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    let state = State::load(&config.state_file)?;
    let manifest = Manifest::load(&config.manifest_path())?;
    menu::show_status(&manifest, &state);
    Ok(())
}

fn cmd_env(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    let mut state = State::load(&config.state_file)?;
    let manifest = Manifest::load(&config.manifest_path())?;

    if let Some(new_env) =
        menu::show_environment_menu(&manifest, state.active_environment.as_deref())?
    {
        change_environment(config, &manifest, &mut state, new_env.as_deref())?;
        state.save(&config.state_file)?;
    }
    Ok(())
}

fn cmd_interactive(config: &AppConfig) -> Result<()> {
    ensure_initialized(config)?;
    let mut state = State::load(&config.state_file)?;
    let term = console::Term::stdout();

    // Enter alternate screen buffer (like btop/vim) so nothing pollutes scrollback
    print!("\x1b[?1049h");
    let _ = term.flush();

    let res = (|| -> Result<()> {
        loop {
            let manifest = Manifest::load(&config.manifest_path())?;
            match menu::show_main_menu(&manifest, state.active_environment.as_deref())? {
                MainMenuAction::ManageComponents => {
                    let active_env = state.active_environment.clone();
                    let components = manifest.components_for_environment(active_env.as_deref());

                    if let Some(selection) = menu::show_components_menu(&components, &state)? {
                        menu::render_header(&term, "Applying Changes")?;
                        apply_changes(config, &manifest, &mut state, &selection)?;
                        state.save(&config.state_file)?;
                        menu::wait_for_enter()?;
                    }
                }
                MainMenuAction::ChangeEnvironment => {
                    if let Some(new_env) =
                        menu::show_environment_menu(&manifest, state.active_environment.as_deref())?
                    {
                        change_environment(config, &manifest, &mut state, new_env.as_deref())?;
                        state.save(&config.state_file)?;
                        menu::wait_for_enter()?;
                    }
                }
                MainMenuAction::UpdateRepo => {
                    menu::render_header(&term, "Update Repository")?;
                    repo::pull_repo(&config.repo_dir)?;
                    println!("{}", style("Repository updated.").green());
                    let updated_manifest = Manifest::load(&config.manifest_path())?;
                    sync_enabled_components(config, &updated_manifest, &mut state)?;
                    state.save(&config.state_file)?;
                    menu::wait_for_enter()?;
                }
                MainMenuAction::Status => {
                    menu::render_header(&term, "System Status")?;
                    menu::show_status(&manifest, &state);
                    menu::wait_for_enter()?;
                }
                MainMenuAction::Exit => {
                    break;
                }
            }
        }
        Ok(())
    })();

    // Always restore original terminal screen on exit
    print!("\x1b[?1049l");
    let _ = term.flush();

    res
}

fn sync_enabled_components(
    config: &AppConfig,
    manifest: &Manifest,
    state: &mut State,
) -> Result<()> {
    let active_env = state.active_environment.clone();
    let visible = manifest.components_for_environment(active_env.as_deref());

    let enabled: Vec<String> = visible
        .iter()
        .filter(|c| state.is_enabled(&c.name))
        .map(|c| c.name.clone())
        .collect();

    if enabled.is_empty() {
        return Ok(());
    }

    println!("\n{}", style("Syncing enabled components...").cyan());
    let selection = ComponentSelection {
        enabled,
        disabled: Vec::new(),
        reinstall: false,
    };
    apply_changes(config, manifest, state, &selection)
}

fn collect_retained_packages(
    manifest: &Manifest,
    active_env: Option<&str>,
    enabled_names: &HashSet<&str>,
) -> HashSet<String> {
    let mut retained = HashSet::new();
    for comp in manifest.components_for_environment(active_env) {
        if enabled_names.contains(comp.name.as_str()) {
            for pkg in &comp.packages {
                retained.insert(pkg.clone());
            }
        }
    }
    retained
}

fn apply_changes(
    config: &AppConfig,
    manifest: &Manifest,
    state: &mut State,
    selection: &ComponentSelection,
) -> Result<()> {
    let needed = !selection.reinstall;
    let active_env = state.active_environment.clone();

    let mut final_enabled: HashSet<&str> = manifest
        .components_for_environment(active_env.as_deref())
        .into_iter()
        .filter(|c| state.is_enabled(&c.name))
        .map(|c| c.name.as_str())
        .collect();

    for name in &selection.disabled {
        final_enabled.remove(name.as_str());
    }
    for name in &selection.enabled {
        final_enabled.insert(name.as_str());
    }

    let retained_pkgs =
        collect_retained_packages(manifest, active_env.as_deref(), &final_enabled);

    for name in &selection.disabled {
        let was_deployed = state.is_enabled(name) || state.deployed.contains_key(name);
        if was_deployed {
            if let Some(comp) = manifest.find_component(name) {
                println!(
                    "\n{} {}",
                    style("[-]").red().bold(),
                    style(&comp.name).red()
                );
                let pkgs_to_remove: Vec<String> = comp
                    .packages
                    .iter()
                    .filter(|p| !retained_pkgs.contains(*p))
                    .cloned()
                    .collect();
                if !pkgs_to_remove.is_empty() {
                    packages::uninstall_packages(&pkgs_to_remove)?;
                }
                deploy::undeploy_component_files(comp, state)?;
            }
        }
        state.components.insert(name.clone(), false);
    }

    let needs_paru = selection.enabled.iter().any(|name| {
        manifest
            .find_component(name)
            .map(|c| c.packages.iter().any(|p| !p.starts_with("local:")))
            .unwrap_or(false)
    });
    if needs_paru {
        packages::ensure_paru()?;
    }

    for name in &selection.enabled {
        if let Some(comp) = manifest.find_component(name) {
            let was_deployed = state.deployed.contains_key(name) || state.is_enabled(name);

            println!(
                "\n{} {}",
                style("[+]").green().bold(),
                style(&comp.name).cyan()
            );

            if !comp.packages.is_empty() {
                packages::install_packages(&comp.packages, &config.repo_dir, needed)?;
            }

            if !was_deployed && !comp.post_install.is_empty() {
                hooks::run_hooks(&comp.post_install, "post_install")?;
            }

            if !comp.files.is_empty() {
                deploy::redeploy_component_files(
                    comp,
                    config,
                    state,
                    active_env.as_deref(),
                )?;
            }

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

fn change_environment(
    config: &AppConfig,
    manifest: &Manifest,
    state: &mut State,
    new_env: Option<&str>,
) -> Result<()> {
    let old_env = state.active_environment.clone();

    let env_selection = if let Some(env_name) = new_env {
        let new_env_components = manifest.components_only_for_environment(env_name);
        if !new_env_components.is_empty() {
            println!(
                "\nConfigure components for environment {}:",
                style(env_name).cyan().bold()
            );
            match menu::show_components_menu(&new_env_components, state)? {
                Some(sel) => Some(sel),
                None => {
                    println!("{}", style("Environment change cancelled.").dim());
                    return Ok(());
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let term = console::Term::stdout();
    menu::render_header(&term, "Changing Environment")?;

    let new_label = new_env.unwrap_or("none");

    if old_env.as_deref() == new_env {
        if let Some(ref sel) = env_selection {
            apply_changes(config, manifest, state, sel)?;
        } else {
            println!("Environment is already set to {}.", style(new_label).cyan());
        }
        return Ok(());
    }

    println!(
        "\nChanging environment: {} → {}",
        style(old_env.as_deref().unwrap_or("none")).red(),
        style(new_label).green().bold()
    );

    let mut new_active_names: HashSet<&str> = manifest
        .components
        .iter()
        .filter(|c| c.environment.is_none() && state.is_enabled(&c.name))
        .map(|c| c.name.as_str())
        .collect();

    if let Some(ref sel) = env_selection {
        for name in &sel.enabled {
            new_active_names.insert(name.as_str());
        }
    }
    let retained_pkgs = collect_retained_packages(manifest, new_env, &new_active_names);

    if let Some(ref old) = old_env {
        let old_components: Vec<_> = manifest
            .components_only_for_environment(old)
            .into_iter()
            .filter(|c| state.is_enabled(&c.name) || state.deployed.contains_key(&c.name))
            .collect();

        for comp in old_components {
            println!(
                "  {} {} (environment: {})",
                style("[-]").red(),
                comp.name,
                old
            );
            let pkgs_to_remove: Vec<String> = comp
                .packages
                .iter()
                .filter(|p| !retained_pkgs.contains(*p))
                .cloned()
                .collect();
            if !pkgs_to_remove.is_empty() {
                packages::uninstall_packages(&pkgs_to_remove)?;
            }
            deploy::undeploy_component_files(comp, state)?;
        }
    }

    state.active_environment = new_env.map(|s| s.to_string());

    if let Some(ref sel) = env_selection {
        apply_changes(config, manifest, state, sel)?;
    }

    let global_with_files: Vec<_> = manifest
        .components
        .iter()
        .filter(|c| c.environment.is_none() && state.is_enabled(&c.name) && !c.files.is_empty())
        .collect();

    if !global_with_files.is_empty() {
        println!("\n  Re-deploying global components for new overlay...");
        for comp in global_with_files {
            println!("  {} {} (overlay update)", style("[~]").yellow(), comp.name);
            deploy::redeploy_component_files(comp, config, state, new_env)?;
            if !comp.post_deploy.is_empty() {
                hooks::run_hooks(&comp.post_deploy, "post_deploy")?;
            }
        }
    }

    println!(
        "\n{}",
        style(format!("✓ Environment changed to {}", new_label))
            .green()
            .bold()
    );
    Ok(())
}

fn ensure_initialized(config: &AppConfig) -> Result<()> {
    if !config.repo_dir.exists() {
        bail!(
            "No repository configured. Run '{}' first.",
            style("wayclone init <url>").cyan()
        );
    }
    Ok(())
}
