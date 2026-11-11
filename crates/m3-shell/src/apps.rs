//! Installed applications (freedesktop desktop entries) and launch history.

use std::collections::HashMap;
use std::path::PathBuf;

use gio_unix::prelude::*;
use gtk::{gdk, gio, prelude::*};
use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Clone)]
pub struct App {
    pub id: String,
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub icon: Option<gio::Icon>,
    pub info: gio_unix::DesktopAppInfo,
}

/// Launcher categories, mapped from freedesktop main categories.
pub const CATEGORIES: &[(&str, &str, &[&str])] = &[
    ("internet", "Internet", &["Network", "WebBrowser", "Email", "Chat", "InstantMessaging"]),
    ("office", "Office", &["Office", "WordProcessor", "Spreadsheet", "Presentation"]),
    ("graphics", "Graphics", &["Graphics", "Photography", "2DGraphics", "3DGraphics"]),
    ("media", "Media", &["AudioVideo", "Audio", "Video", "Music", "Player"]),
    ("development", "Development", &["Development", "IDE", "TextEditor"]),
    ("games", "Games", &["Game"]),
    ("system", "System", &["System", "Settings", "Utility", "TerminalEmulator", "FileManager", "Monitor"]),
];

impl App {
    fn from_info(info: gio_unix::DesktopAppInfo) -> Option<Self> {
        if !info.should_show() {
            return None;
        }
        let id = info.id()?.to_string();
        let categories = info
            .categories()
            .map(|c| c.split(';').filter(|s| !s.is_empty()).map(str::to_owned).collect())
            .unwrap_or_default();
        Some(Self {
            name: info.display_name().to_string(),
            description: info.description().map(|d| d.to_string()).unwrap_or_default(),
            keywords: info.keywords().iter().map(|k| k.to_string()).collect(),
            icon: info.icon(),
            categories,
            id,
            info,
        })
    }

    pub fn in_category(&self, cat: &str) -> bool {
        CATEGORIES
            .iter()
            .find(|(id, _, _)| *id == cat)
            .is_some_and(|(_, _, fd)| self.categories.iter().any(|c| fd.contains(&c.as_str())))
    }

    /// Relevance of `query` (lowercase) for this app; 0 means no match.
    pub fn score(&self, query: &str) -> u32 {
        if query.is_empty() {
            return 1;
        }
        let name = self.name.to_lowercase();
        if name == query {
            return 100;
        }
        if name.starts_with(query) {
            return 80;
        }
        if name.split_whitespace().any(|w| w.starts_with(query)) {
            return 60;
        }
        if name.contains(query) {
            return 40;
        }
        if self.keywords.iter().any(|k| k.to_lowercase().starts_with(query)) {
            return 30;
        }
        if self.id.to_lowercase().contains(query) {
            return 20;
        }
        if self.description.to_lowercase().contains(query) {
            return 10;
        }
        0
    }

    /// Launch with an activation token so the new window gets focus.
    pub fn launch(&self) {
        let ctx = gdk::Display::default().map(|d| d.app_launch_context());
        if let Err(e) = self.info.launch(&[], ctx.as_ref()) {
            warn!("failed to launch {}: {e}", self.id);
        }
    }
}

pub fn all_apps() -> Vec<App> {
    let mut apps: Vec<App> = gio::AppInfo::all()
        .into_iter()
        .filter_map(|a| a.downcast::<gio_unix::DesktopAppInfo>().ok())
        .filter_map(App::from_info)
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.id == b.id);
    apps
}

/// Find the desktop entry for a window's app id (or X11 class).
pub fn app_for_window(apps: &[App], app_id: &str) -> Option<App> {
    let lower = app_id.to_lowercase();
    apps.iter()
        .find(|a| a.id.trim_end_matches(".desktop").eq_ignore_ascii_case(app_id))
        .or_else(|| apps.iter().find(|a| a.info.startup_wm_class().is_some_and(|c| c.eq_ignore_ascii_case(app_id))))
        .or_else(|| {
            // e.g. app id "firefox" vs "org.mozilla.firefox.desktop"
            apps.iter().find(|a| {
                let id = a.id.trim_end_matches(".desktop").to_lowercase();
                id.rsplit('.').next() == Some(lower.as_str())
            })
        })
        .cloned()
}

/// Launch counts and search history, persisted in the state directory.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    pub launches: HashMap<String, u32>,
    /// Most recent first.
    pub recent: Vec<String>,
    pub searches: Vec<String>,
}

impl Usage {
    fn path() -> Option<PathBuf> {
        m3_core::paths::state_dir().map(|d| d.join("launcher.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_vec(self) {
            Ok(bytes) => {
                if let Err(e) = m3_core::config::atomic_write(&path, &bytes) {
                    warn!("cannot save launcher history: {e}");
                }
            }
            Err(e) => warn!("cannot serialize launcher history: {e}"),
        }
    }

    pub fn record_launch(&mut self, id: &str) {
        *self.launches.entry(id.to_owned()).or_default() += 1;
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(12);
        self.save();
    }

    pub fn record_search(&mut self, q: &str) {
        let q = q.trim();
        if q.len() < 2 {
            return;
        }
        self.searches.retain(|s| s != q);
        self.searches.insert(0, q.to_owned());
        self.searches.truncate(8);
        self.save();
    }
}
