# Waysync

**Waysync** is a declarative dotfile and package manager for Arch-based Linux distributions. It provides a full-screen interactive TUI to manage system packages (native Arch packages, AUR packages via `paru`, and local `PKGBUILD`s), deploy dotfiles with SHA256 conflict detection, and switch between desktop environments or shells (such as `caelestia`, `noctalia`, GNOME, or KDE) using per-file overlays.

## Quick Installation

Install `waysync` directly via `curl | bash`:

```bash
curl -fsSL https://raw.githubusercontent.com/GwynnN7/waysync/main/install.sh | bash
```

Install `waysync` and initialize your dotfiles repository in a single command:

```bash
curl -fsSL https://raw.githubusercontent.com/GwynnN7/waysync/main/install.sh | bash -s -- https://github.com/GwynnN7/dotfiles.git
```

## CLI Usage

| Command | Description |
| :--- | :--- |
| `waysync init <url>` | Clone your dotfiles repository into `~/.local/share/waysync/repo` and initialize state. |
| `waysync` | Launch the full-screen interactive TUI menu. |
| `waysync env` | Switch the active environment (or select `none` for global-only) and configure environment-bound components. |
| `waysync update` | Pull the latest changes from your dotfiles repository and sync enabled components. |
| `waysync status` | Display the current repository, active environment, and deployed component status. |

### Interactive TUI Controls

**Categories View:**
* `Up` / `Down` (or `j` / `k`): Navigate items.
* `Space`: Toggle an entire category on or off (if partially enabled, pressing `Space` enables all components in that category).
* `Enter` (or `Right` / `l`): Open a category to toggle individual components, or trigger `Save & Apply`.
* `Esc` / `q`: Cancel or exit.

**Component View (Inside a Category):**
* `Up` / `Down` (or `j` / `k`): Navigate components.
* `Space`: Toggle the selected component.
* `a`: Toggle all components in the current category.
* `Enter` / `Esc` / `Left` / `h` / `q`: Return to the Categories view.

## Dotfiles Repository Structure

waysync expects your dotfiles repository to contain `manifest.toml` at the root alongside `dotfiles/`, and optionally `overlays/` and `packages/`:

```text
my-dotfiles-repo/
├── manifest.toml          # Declarative configuration of environments & components
├── dotfiles/              # Base/global dotfiles (files and directories)
├── overlays/              # Optional environment-specific file overrides
│   └── <shell>/
└── packages/              # Optional local PKGBUILDs (referenced as "local:<name>")
    ├── shell/
    │   └── PKGBUILD
    └── kdeconnect/
        └── PKGBUILD
```

### How Overlays Work (`dotfiles/` vs `overlays/<env>/`)

When an environment (e.g., `caelestia` or `noctalia`) is active, waysync resolves every `[[components.files]]` entry as follows:

1. **Single File (`src = "starship.toml"`):** waysync checks if `overlays/<env>/starship.toml` exists. If found, it deploys the overlay file; otherwise, it falls back to `dotfiles/starship.toml`.
2. **Directory (`src = "hypr"`):** waysync scans `dotfiles/hypr/` first and then overlays `overlays/<env>/hypr/` on top. Files are merged by their relative path—meaning `overlays/noctalia/hypr/hyprland.conf` overrides `dotfiles/hypr/hyprland.conf`, while all other files in `dotfiles/hypr/` are still deployed without duplication.
3. **Switching Environments:** Running `waysync env` uninstalls packages and undeploys files belonging to the old environment, installs the new environment's components, re-deploys global components with the new overlay, and removes any orphaned files from the previous overlay.

> **Note on External Git Repositories (`repo:<user>/<repo>`):**
> Entries using `src = "repo:<user>/<repo>"` are cloned and updated directly from GitHub into their target `dest` directories. They do not need to be stored inside `dotfiles/`.

## Manifest Schema Reference (`manifest.toml`)

```toml
# Optional: Define environments/shells. Omit if you only use global components.
[environments]
available = ["caelestia", "noctalia"]

[[components]]
name = "hypr"                          # Unique component identifier
category = "wm"                        # Menu category (defaults to "general")
default = true                         # Checked by default on first run (default: false)
environment = "caelestia"              # Optional: bind component to a specific environment
packages = [
    "hyprland",                        # Native Arch or AUR package (installed via paru)
    "local:my-custom-pkg",             # Local PKGBUILD in packages/my-custom-pkg/PKGBUILD
]
post_install = [                       # Shell commands run ONLY when newly enabled
    "hyprctl reload >/dev/null",
]
post_deploy = [                        # Shell commands run after every dotfile deployment
    "hyprctl reload >/dev/null",
]

[[components.files]]
src = "hypr"                           # Path in dotfiles/ (or overlays/<env>/), OR "repo:user/repo"
dest = "$XDG_CONFIG_HOME/hypr"         # Destination path (supports env vars)
sudo = false                           # Copy/remove using sudo (default: false)
check = true                           # Prompt with diff if modified locally (default: true)
```

### Supported Environment Variables in `dest`

waysync expands any environment variable set in your current session and provides built-in fallbacks for standard XDG variables:

* `$HOME` -> `/home/<user>`
* `$XDG_CONFIG_HOME` -> `$HOME/.config`
* `$XDG_DATA_HOME` -> `$HOME/.local/share`
* `$XDG_STATE_HOME` -> `$HOME/.local/state`
* `$XDG_CACHE_HOME` -> `$HOME/.cache`
* `$XDG_BIN_HOME` -> `$HOME/.local/bin`
* `$XDG_CONFIG_SERVICE` -> `$HOME/.config/systemd/user`
