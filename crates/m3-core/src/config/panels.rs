//! `panels.toml` — the desktop panel.

use super::{ConfigFile, check_range};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PanelPosition {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

impl PanelPosition {
    pub fn is_vertical(self) -> bool {
        matches!(self, PanelPosition::Left | PanelPosition::Right)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PanelVisibility {
    #[default]
    Always,
    /// Hide until the pointer touches the screen edge.
    Autohide,
    /// Hide only while a window overlaps the panel.
    Intelligent,
}

/// Panel modules, in the order they should appear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelModule {
    Launcher,
    Workspaces,
    Tasks,
    Clock,
    Tray,
    Status,
    Notifications,
    User,
    /// Power menu: lock, log out, suspend, restart, shut down.
    Power,
    /// A "Search" pill that opens the launcher.
    Search,
}

/// How the panel is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PanelStyle {
    /// Each module floats as its own pill over the wallpaper.
    #[default]
    Islands,
    /// One continuous bar.
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClockFormat {
    #[default]
    H24,
    H12,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelsConfig {
    pub position: PanelPosition,
    pub visibility: PanelVisibility,
    /// Thickness in logical pixels.
    pub size: u32,
    /// Show a panel on every output (otherwise only on the primary one).
    pub all_outputs: bool,
    /// Detach the panel from the screen edge with a margin (floating style).
    pub floating: bool,
    pub style: PanelStyle,
    pub start: Vec<PanelModule>,
    pub center: Vec<PanelModule>,
    pub end: Vec<PanelModule>,
    /// Desktop file ids pinned to the task area, e.g. `org.mozilla.firefox.desktop`.
    pub pinned: Vec<String>,
    pub clock_format: ClockFormat,
    pub clock_seconds: bool,
    pub clock_date: bool,
    pub battery_percentage: bool,
}

impl Default for PanelsConfig {
    fn default() -> Self {
        use PanelModule::*;
        Self {
            position: PanelPosition::Bottom,
            visibility: PanelVisibility::Always,
            size: 48,
            all_outputs: true,
            floating: true,
            style: PanelStyle::Islands,
            start: vec![Power, Workspaces],
            center: vec![Launcher, Search, Tasks],
            end: vec![Tray, Status, Notifications, Clock],
            // m3's own apps; entries that aren't installed are skipped.
            pinned: vec![
                "org.m3.Browser.desktop".into(),
                "org.m3.Files.desktop".into(),
                "org.m3.Terminal.desktop".into(),
                "org.m3.Music.desktop".into(),
            ],
            clock_format: ClockFormat::H24,
            clock_seconds: false,
            clock_date: true,
            battery_percentage: true,
        }
    }
}

impl ConfigFile for PanelsConfig {
    const FILE_NAME: &'static str = "panels.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        check_range(&mut p, "size", self.size, 32, 96);
        let all: Vec<&PanelModule> = self.start.iter().chain(&self.center).chain(&self.end).collect();
        for (i, m) in all.iter().enumerate() {
            if all[..i].contains(m) {
                p.push(format!("module `{}` appears more than once", serde_plain(m)));
            }
        }
        for id in &self.pinned {
            if !id.ends_with(".desktop") || id.contains('/') {
                p.push(format!("pinned entry `{id}` must be a desktop file id like `firefox.desktop`"));
            }
        }
        p
    }
}

fn serde_plain(m: &PanelModule) -> String {
    toml::Value::try_from(m).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_modules() {
        let c = PanelsConfig { end: vec![PanelModule::Clock, PanelModule::Clock], ..Default::default() };
        assert_eq!(c.validate().len(), 1);
        assert!(c.validate()[0].contains("clock"));
    }
}
