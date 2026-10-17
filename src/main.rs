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
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use console::style;

use config::AppConfig;
use manifest::Manifest;
use menu::{ComponentSelection, MainMenuAction};
use state::State;

#[derive(Parser)]
#[command(
    name = "waysync",
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
                println!("\n{}", style("Select an environment:").cyan().bold());
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
        style("waysync").cyan()
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
    if let Err(error) = repo::pull_repo(&config.repo_dir) {
        eprintln!(
            "Warning: could not refresh the dotfiles repository; using the local copy: {}",
            error
        );
    }
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
                        run_with_visible_output(&term, "Applying Changes", || {
                            apply_changes(config, &manifest, &mut state, &selection)?;
                            state.save(&config.state_file)
                        })?;
                        menu::show_completion(
                            &term,
                            "Changes Applied",
                            "Component changes completed successfully.",
                        )?;
                        menu::wait_for_enter()?;
                    }
                }
                MainMenuAction::SyncSpecific => {
                    let active_env = state.active_environment.clone();
                    let components = manifest.components_for_environment(active_env.as_deref());

                    if let Some(selection) = menu::show_sync_specific_menu(&components, &state)? {
                        run_with_visible_output(&term, "Syncing Selected Components", || {
                            apply_changes(config, &manifest, &mut state, &selection)?;
                            state.save(&config.state_file)
                        })?;
                        menu::show_completion(
                            &term,
                            "Components Synced",
                            "Selected components synced successfully.",
                        )?;
                        menu::wait_for_enter()?;
                    }
                }
                MainMenuAction::UninstallSpecific => {
                    let active_env = state.active_environment.clone();
                    let installed_components: Vec<_> = manifest
                        .components_for_environment(active_env.as_deref())
                        .into_iter()
                        .filter(|c| {
                            state.is_enabled(&c.name) || state.deployed.contains_key(&c.name)
                        })
                        .collect();

                    if installed_components.is_empty() {
                        menu::show_completion(
                            &term,
                            "Uninstall Components",
                            "No components are currently installed.",
                        )?;
                        menu::wait_for_enter()?;
                    } else if let Some(selection) =
                        menu::show_uninstall_menu(&installed_components, &state)?
                    {
                        run_with_visible_output(&term, "Uninstalling Selected Components", || {
                            apply_changes(config, &manifest, &mut state, &selection)?;
                            state.save(&config.state_file)
                        })?;
                        menu::show_completion(
                            &term,
                            "Components Uninstalled",
                            "Selected components uninstalled successfully.",
                        )?;
                        menu::wait_for_enter()?;
                    }
                }
                MainMenuAction::ChangeEnvironment => {
                    if let Some(new_env) =
                        menu::show_environment_menu(&manifest, state.active_environment.as_deref())?
                    {
                        run_with_visible_output(&term, "Changing Environment", || {
                            change_environment(config, &manifest, &mut state, new_env.as_deref())?;
                            state.save(&config.state_file)
                        })?;
                        menu::show_completion(
                            &term,
                            "Environment Changed",
                            "Environment configuration completed.",
                        )?;
                        menu::wait_for_enter()?;
                    }
                }
                MainMenuAction::UpdateRepo => {
                    run_with_visible_output(&term, "Update Repository", || {
                        repo::pull_repo(&config.repo_dir)?;
                        println!("{}", style("Repository updated.").green());
                        let updated_manifest = Manifest::load(&config.manifest_path())?;
                        sync_enabled_components(config, &updated_manifest, &mut state)?;
                        state.save(&config.state_file)
                    })?;
                    menu::show_completion(
                        &term,
                        "Repository Updated",
                        "Repository refresh and enabled-component sync completed.",
                    )?;
                    menu::wait_for_enter()?;
                }
                MainMenuAction::Status => {
                    menu::show_status_menu(&manifest, &state)?;
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

/// Temporarily leave the alternate screen so package output stays in terminal scrollback.
/// This also keeps child commands on the same terminal as the initial sudo validation.
fn run_with_visible_output<T>(
    term: &console::Term,
    subtitle: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    print!("\x1b[?1049l");
    term.flush()?;
    println!("\n  {} {}", style("waysync").cyan().bold(), style(format!("── {}", subtitle)).dim());
    println!("  {}\n", style("────────────────────────────────────────────────────────").dim());

    let result = operation();

    print!("\x1b[?1049h");
    let _ = term.flush();
    result
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
        skip_ask: false,
    };
    apply_changes(
        config,
        manifest,
        state,
        &selection,
    )
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

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static SUDO_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Keeps a validated sudo credential alive for the duration of one apply operation
/// and shields child hooks (like `hyprpm`) from wiping the timestamp via `sudo -k`.
struct SudoSession {
    stop: Option<Sender<()>>,
    keepalive: Option<thread::JoinHandle<()>>,
    shim_dir: Option<PathBuf>,
    old_path: Option<String>,
    owns_session: bool,
}

impl Drop for SudoSession {
    fn drop(&mut self) {
        if !self.owns_session {
            return;
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(keepalive) = self.keepalive.take() {
            let _ = keepalive.join();
        }
        if let Some(ref old_path) = self.old_path {
            std::env::set_var("PATH", old_path);
        }
        if let Some(ref shim_dir) = self.shim_dir {
            let _ = std::fs::remove_dir_all(shim_dir);
        }
        SUDO_SESSION_ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn start_sudo_session() -> Result<SudoSession> {
    // If an outer operation (e.g. change_environment) already started a session, reuse it.
    if SUDO_SESSION_ACTIVE.swap(true, Ordering::SeqCst) {
        return Ok(SudoSession {
            stop: None,
            keepalive: None,
            shim_dir: None,
            old_path: None,
            owns_session: false,
        });
    }

    println!(
        "\n{}",
        style("Authentication is required for this operation.").cyan()
    );
    let status = std::process::Command::new("/usr/bin/sudo")
        .arg("-v")
        .status()
        .context("Failed to request administrator authentication")?;
    if !status.success() {
        SUDO_SESSION_ACTIVE.store(false, Ordering::SeqCst);
        bail!("Administrator authentication was not accepted");
    }

    // Create a temporary PATH shim for `sudo` that strips `-k` / `--reset-timestamp`
    // so tools like `hyprpm` cannot invalidate our active sudo session mid-run.
    // Uses \x24 instead of literal '\$' to prevent terminal copy-paste escaping.
    let shim_dir = std::env::temp_dir().join(format!("waysync-sudo-shim-{}", std::process::id()));
    std::fs::create_dir_all(&shim_dir)?;
    let shim_script = shim_dir.join("sudo");
    let script_content = concat!(
        "#!/bin/sh\n",
        "for arg in \"\x24@\"; do\n",
        "    shift\n",
        "    case \"\x24arg\" in\n",
        "        -k|--reset-timestamp|-K|--remove-timestamp) ;;\n",
        "        *) set -- \"\x24@\" \"\x24arg\" ;;\n",
        "    esac\n",
        "done\n",
        "if [ \"\x24#\" -eq 0 ]; then\n",
        "    exit 0\n",
        "fi\n",
        "exec /usr/bin/sudo \"\x24@\"\n"
    );
    std::fs::write(&shim_script, script_content)?;
    std::fs::set_permissions(&shim_script, std::fs::Permissions::from_mode(0o755))?;

    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", shim_dir.display(), old_path);
    std::env::set_var("PATH", &new_path);

    let (stop, receiver) = mpsc::channel();
    let keepalive = thread::spawn(move || {
        loop {
            match receiver.recv_timeout(Duration::from_secs(30)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = std::process::Command::new("/usr/bin/sudo")
                        .args(["-n", "-v"])
                        .status();
                }
            }
        }
    });

    Ok(SudoSession {
        stop: Some(stop),
        keepalive: Some(keepalive),
        shim_dir: Some(shim_dir),
        old_path: Some(old_path),
        owns_session: true,
    })
}

/// Detect if a hook command invokes `sudo`, `hyprpm`, or `pkexec` anywhere in the script.
fn hook_requires_sudo(cmd: &str) -> bool {
    cmd.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .any(|token| matches!(token, "sudo" | "hyprpm" | "pkexec"))
}

fn component_deployment_requires_sudo(
    component: &manifest::Component,
    active_env: Option<&str>,
) -> bool {
    component
        .files
        .iter()
        .any(|file| file.sudo && file.applies_to_environment(active_env))
        || component
            .post_install
            .iter()
            .chain(component.post_deploy.iter())
            .filter(|hook| hook.applies_to_environment(active_env))
            .any(|hook| hook_requires_sudo(hook.cmd()))
}

fn component_requires_sudo(component: &manifest::Component, active_env: Option<&str>) -> bool {
    !component.packages.is_empty() || component_deployment_requires_sudo(component, active_env)
}

fn selection_requires_sudo(
    manifest: &Manifest,
    selection: &ComponentSelection,
    active_env: Option<&str>,
) -> bool {
    selection
        .enabled
        .iter()
        .chain(selection.disabled.iter())
        .filter_map(|name| manifest.find_component(name))
        .any(|component| component_requires_sudo(component, active_env))
}

fn enabled_environment_components_require_sudo(
    manifest: &Manifest,
    state: &State,
    environment: &str,
) -> bool {
    manifest
        .components_only_for_environment(environment)
        .into_iter()
        .filter(|component| {
            state.is_enabled(&component.name) || state.deployed.contains_key(&component.name)
        })
        .any(|component| component_requires_sudo(component, Some(environment)))
}

fn enabled_global_components_require_sudo(
    manifest: &Manifest,
    state: &State,
    active_env: Option<&str>,
) -> bool {
    manifest
        .components
        .iter()
        .filter(|component| component.environment.is_none() && state.is_enabled(&component.name))
        .any(|component| component_deployment_requires_sudo(component, active_env))
}

fn apply_changes(
    config: &AppConfig,
    manifest: &Manifest,
    state: &mut State,
    selection: &ComponentSelection,
) -> Result<()> {
    let enabled = selection
        .enabled
        .iter()
        .filter_map(|name| {
            let component = manifest.find_component(name)?;
            let already_installed =
                state.is_enabled(&component.name) || state.deployed.contains_key(&component.name);
            let needs_confirmation = 
                component.ask && !selection.skip_ask && already_installed && !selection.reinstall;

            if !needs_confirmation {
                return Some(component.name.clone());
            }

            match menu::confirm_component_update(&component.name) {
                Ok(true) => Some(component.name.clone()),
                Ok(false) => {
                    println!(
                        "  {} Skipping optional update for {}.",
                        style("→").dim(),
                        style(&component.name).yellow()
                    );
                    None
                }
                Err(error) => {
                    eprintln!(
                        "  {} Could not ask about {}: {}",
                        style("!").yellow(),
                        component.name,
                        error
                    );
                    None
                }
            }
        })
        .collect();
    let selection = ComponentSelection {
        enabled,
        disabled: selection.disabled.clone(),
        reinstall: selection.reinstall,
        skip_ask: selection.skip_ask,
    };
    let needed = !selection.reinstall;
    let active_env = state.active_environment.clone();
    let _sudo_session = selection_requires_sudo(manifest, &selection, active_env.as_deref())
        .then(start_sudo_session)
        .transpose()?;

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
                    packages::uninstall_packages(&pkgs_to_remove, &config.repo_dir)?;
                }
                deploy::undeploy_component_files(comp, state)?;
            }
        }
        state.components.insert(name.clone(), false);
        state.save(&config.state_file)?;
    }

    let needs_paru = selection.enabled.iter().any(|name| {
        manifest
            .find_component(name)
            .map(|c| !c.packages.is_empty())
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
                hooks::run_hooks(
                    &comp.post_install,
                    "post_install",
                    active_env.as_deref(),
                    &config.repo_dir,
                )?;
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
                hooks::run_hooks(
                    &comp.post_deploy,
                    "post_deploy",
                    active_env.as_deref(),
                    &config.repo_dir,
                )?;
            }
        }
        state.components.insert(name.clone(), true);
        state.save(&config.state_file)?;
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

    let old_environment_requires_sudo = old_env
        .as_deref()
        .map(|environment| {
            enabled_environment_components_require_sudo(manifest, state, environment)
        })
        .unwrap_or(false);
    let new_selection_requires_sudo = env_selection
        .as_ref()
        .map(|selection| selection_requires_sudo(manifest, selection, new_env))
        .unwrap_or(false);
    let global_components_require_sudo =
        enabled_global_components_require_sudo(manifest, state, new_env);
    let _sudo_session = (old_environment_requires_sudo
        || new_selection_requires_sudo
        || global_components_require_sudo)
        .then(start_sudo_session)
        .transpose()?;

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
                packages::uninstall_packages(&pkgs_to_remove, &config.repo_dir)?;
            }
            deploy::undeploy_component_files(comp, state)?;
        }
    }

    state.active_environment = new_env.map(|s| s.to_string());

    if let Some(ref sel) = env_selection {
        apply_changes(config, manifest, state, sel)?;
    }

    let global_to_update: Vec<_> = manifest
        .components
        .iter()
        .filter(|c| {
            c.environment.is_none()
                && state.is_enabled(&c.name)
                && (!c.files.is_empty()
                    || c.post_install
                        .iter()
                        .any(|h| h.environment() == new_env && new_env.is_some())
                    || !c.post_deploy.is_empty())
        })
        .collect();

    if !global_to_update.is_empty() {
        println!("\n  Re-deploying global components for new environment overrides...");
        for comp in global_to_update {
            println!(
                "  {} {} (override update)",
                style("[~]").yellow(),
                comp.name
            );
            if !comp.files.is_empty() {
                deploy::redeploy_component_files(comp, config, state, new_env)?;
            }
            // Run any post_install hooks specifically scoped to the newly activated environment
            if new_env.is_some() {
                let env_specific_install: Vec<_> = comp
                    .post_install
                    .iter()
                    .filter(|h| h.environment() == new_env)
                    .cloned()
                    .collect();
                if !env_specific_install.is_empty() {
                    hooks::run_hooks(
                        &env_specific_install,
                        "post_install",
                        new_env,
                        &config.repo_dir,
                    )?;
                }
            }
            if !comp.post_deploy.is_empty() {
                hooks::run_hooks(&comp.post_deploy, "post_deploy", new_env, &config.repo_dir)?;
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
            style("waysync init <url>").cyan()
        );
    }
    Ok(())
}
