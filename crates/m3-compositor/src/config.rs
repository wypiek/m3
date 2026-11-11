//! The compositor's view of the m3 configuration files.

use m3_core::config::{self as cfg, CompositorConfig, ConfigError, Keybindings, ThemeConfig, WorkspacesConfig};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub compositor: CompositorConfig,
    pub keybindings: Keybindings,
    pub workspaces: WorkspacesConfig,
    pub theme: ThemeConfig,
}

impl Config {
    /// Initial load: invalid files fall back to defaults (with errors logged).
    pub fn load(dir: &Path) -> Self {
        Self::reload(dir, &Config::default()).0
    }

    /// Reload from disk, keeping the previous value of any file that fails to
    /// load. Returns the errors encountered.
    pub fn reload(dir: &Path, previous: &Config) -> (Self, Vec<ConfigError>) {
        let mut errors = Vec::new();
        let mut take = |e: Option<ConfigError>| errors.extend(e);
        let (compositor, e) = cfg::load_or(dir, previous.compositor.clone());
        take(e);
        let (keybindings, e) = cfg::load_or(dir, previous.keybindings.clone());
        take(e);
        let (workspaces, e) = cfg::load_or(dir, previous.workspaces.clone());
        take(e);
        let (theme, e) = cfg::load_or(dir, previous.theme.clone());
        take(e);
        (Self { compositor, keybindings, workspaces, theme }, errors)
    }
}

/// Resolve the theme. The wallpaper seed comes from the cache the shell fills
/// (the compositor never decodes images itself).
pub fn resolve_theme(config: &Config) -> m3_theme::Theme {
    let wp = &config.theme.wallpaper.path;
    let seed = if wp.as_os_str().is_empty() { None } else { m3_theme::wallpaper::cached_seed(wp) };
    m3_theme::Theme::resolve(&config.theme, seed, m3_core::time::local_minutes())
}
