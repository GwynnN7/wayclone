use anyhow::Result;
use console::style;
use dialoguer::{theme::ColorfulTheme, MultiSelect, Select};

use crate::manifest::{Component, Manifest};
use crate::state::State;

/// Main menu actions.
pub enum MainMenuAction {
    ManageComponents,
    ChangeShell,
    UpdateRepo,
    Status,
    Exit,
}

/// Result of the component selection menu.
pub struct ComponentSelection {
    /// Names of components the user wants enabled.
    pub enabled: Vec<String>,
    /// Names of components the user wants disabled.
    pub disabled: Vec<String>,
    /// If true, reinstall all packages (no --needed).
    pub reinstall: bool,
}

/// Show the main menu and return the selected action.
pub fn show_main_menu(manifest: &Manifest) -> Result<MainMenuAction> {
    let has_shells = manifest.shells.is_some();

    let mut items = vec!["📦 Manage Components"];
    if has_shells {
        items.push("🐚 Change Shell");
    }
    items.extend_from_slice(&["🔄 Update Repository", "📊 Status", "🚪 Exit"]);

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("{}", style("wayclone").cyan().bold()))
        .items(&items)
        .default(0)
        .interact()?;

    // Map selection index back to action accounting for optional shell entry
    if has_shells {
        Ok(match selection {
            0 => MainMenuAction::ManageComponents,
            1 => MainMenuAction::ChangeShell,
            2 => MainMenuAction::UpdateRepo,
            3 => MainMenuAction::Status,
            _ => MainMenuAction::Exit,
        })
    } else {
        Ok(match selection {
            0 => MainMenuAction::ManageComponents,
            1 => MainMenuAction::UpdateRepo,
            2 => MainMenuAction::Status,
            _ => MainMenuAction::Exit,
        })
    }
}

/// Show the component selection menu with categories.
/// Components are grouped by category and displayed as `category/name`.
pub fn show_components_menu(
    components: &[&Component],
    state: &State,
) -> Result<Option<ComponentSelection>> {
    if components.is_empty() {
        println!("No components available.");
        return Ok(None);
    }

    let is_first_run = state.is_first_run();

    // Group components by category, maintaining order of first appearance
    let mut category_order: Vec<String> = Vec::new();
    let mut by_category: std::collections::HashMap<String, Vec<&Component>> =
        std::collections::HashMap::new();

    for comp in components {
        let cat = comp
            .category
            .as_deref()
            .unwrap_or("general")
            .to_string();
        if !by_category.contains_key(&cat) {
            category_order.push(cat.clone());
        }
        by_category.entry(cat).or_default().push(comp);
    }

    // Build display items and defaults
    let mut items: Vec<String> = Vec::new();
    let mut defaults: Vec<bool> = Vec::new();
    let mut names: Vec<String> = Vec::new();

    for cat in &category_order {
        let comps = &by_category[cat];
        for comp in comps {
            let enabled = if is_first_run {
                comp.default
            } else {
                state.is_enabled(&comp.name)
            };

            let shell_tag = comp
                .shell
                .as_ref()
                .map(|s| format!(" {}", style(format!("[{}]", s)).dim()))
                .unwrap_or_default();

            let pkg_count = comp.packages.len();
            let file_count = comp.files.len();
            let meta = format!(
                " {}",
                style(format!("({}pkg, {}files)", pkg_count, file_count)).dim()
            );

            items.push(format!("{}/{}{}{}", cat, comp.name, shell_tag, meta));
            defaults.push(enabled);
            names.push(comp.name.clone());
        }
    }

    // Add reinstall toggle at the end
    items.push(format!(
        "{}",
        style("⟳ Reinstall (force reinstall all packages)").yellow()
    ));
    defaults.push(false);

    let selected = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select components (↑↓ navigate, space toggle, enter confirm)")
        .items(&items)
        .defaults(&defaults)
        .interact_opt()?;

    let selected = match selected {
        Some(s) => s,
        None => return Ok(None), // User pressed Escape
    };

    let reinstall = selected.contains(&(items.len() - 1));

    let mut enabled = Vec::new();
    let mut disabled = Vec::new();

    for (i, name) in names.iter().enumerate() {
        if selected.contains(&i) {
            enabled.push(name.clone());
        } else {
            disabled.push(name.clone());
        }
    }

    Ok(Some(ComponentSelection {
        enabled,
        disabled,
        reinstall,
    }))
}

/// Show a shell selection menu.
pub fn show_shell_menu(manifest: &Manifest, current: Option<&str>) -> Result<Option<String>> {
    let shells = manifest.available_shells();
    if shells.is_empty() {
        println!("No shells configured in manifest.");
        return Ok(None);
    }

    let current_label = current.unwrap_or("none");
    let items: Vec<String> = shells
        .iter()
        .map(|s| {
            if Some(*s) == current {
                format!("{} {}", s, style("(active)").green())
            } else {
                s.to_string()
            }
        })
        .collect();

    let default = current
        .and_then(|c| shells.iter().position(|s| *s == c))
        .unwrap_or(0);

    println!(
        "Current shell: {}",
        style(current_label).cyan().bold()
    );

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select shell")
        .items(&items)
        .default(default)
        .interact_opt()?;

    match selection {
        Some(idx) => Ok(Some(shells[idx].to_string())),
        None => Ok(None),
    }
}

/// Display current system status.
pub fn show_status(manifest: &Manifest, state: &State) {
    println!("\n{}", style("═══ wayclone status ═══").cyan().bold());

    if let Some(ref url) = state.repo_url {
        println!("Repository: {}", style(url).dim());
    }
    if let Some(ref shell) = state.active_shell {
        println!("Active shell: {}", style(shell).cyan().bold());
    }

    let active_shell = state.active_shell.as_deref();
    let components = manifest.components_for_shell(active_shell);

    if components.is_empty() {
        println!("No components configured.");
        return;
    }

    // Group by category
    let mut category_order: Vec<String> = Vec::new();
    let mut by_category: std::collections::HashMap<String, Vec<&Component>> =
        std::collections::HashMap::new();

    for comp in &components {
        let cat = comp
            .category
            .as_deref()
            .unwrap_or("general")
            .to_string();
        if !by_category.contains_key(&cat) {
            category_order.push(cat.clone());
        }
        by_category.entry(cat).or_default().push(comp);
    }

    println!();
    for cat in &category_order {
        println!("  {}", style(format!("┌─ {} ─┐", cat)).dim());
        for comp in &by_category[cat] {
            let enabled = state.is_enabled(&comp.name);
            let status_icon = if enabled {
                style("✓").green().bold()
            } else {
                style("✗").red()
            };
            let deployed_count = state
                .deployed
                .get(&comp.name)
                .map(|d| d.len())
                .unwrap_or(0);

            let shell_tag = comp
                .shell
                .as_ref()
                .map(|s| format!(" [{}]", s))
                .unwrap_or_default();

            if enabled && deployed_count > 0 {
                println!(
                    "  {} {}{} {} files deployed",
                    status_icon,
                    comp.name,
                    style(&shell_tag).dim(),
                    style(format!("({})", deployed_count)).dim()
                );
            } else {
                println!(
                    "  {} {}{}",
                    status_icon,
                    comp.name,
                    style(&shell_tag).dim()
                );
            }
        }
    }
    println!();
}
