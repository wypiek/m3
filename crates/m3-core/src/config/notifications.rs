//! `notifications.toml` — notification behaviour and per-app preferences.

use super::{ConfigFile, parse_hhmm};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppNotifications {
    /// Accept notifications from this app at all.
    pub enabled: bool,
    /// Show popups (otherwise they only go to the notification centre).
    pub popups: bool,
}

impl Default for AppNotifications {
    fn default() -> Self {
        Self { enabled: true, popups: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    /// Show popups at all.
    pub popups: bool,
    /// Automatically enable Do Not Disturb between `dnd_from` and `dnd_until`.
    pub dnd_schedule: bool,
    pub dnd_from: String,
    pub dnd_until: String,
    /// Keyed by the application name notifications report.
    pub apps: BTreeMap<String, AppNotifications>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            popups: true,
            dnd_schedule: false,
            dnd_from: "22:00".into(),
            dnd_until: "07:00".into(),
            apps: BTreeMap::new(),
        }
    }
}

impl NotificationsConfig {
    pub fn app(&self, name: &str) -> AppNotifications {
        self.apps.get(name).cloned().unwrap_or_default()
    }

    /// Whether the schedule says Do Not Disturb at `minutes` since midnight.
    pub fn scheduled_dnd(&self, minutes: u32) -> bool {
        if !self.dnd_schedule {
            return false;
        }
        let (Ok(from), Ok(until)) = (parse_hhmm(&self.dnd_from), parse_hhmm(&self.dnd_until)) else { return false };
        if from <= until { minutes >= from && minutes < until } else { minutes >= from || minutes < until }
    }
}

impl ConfigFile for NotificationsConfig {
    const FILE_NAME: &'static str = "notifications.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        for (k, v) in [("dnd_from", &self.dnd_from), ("dnd_until", &self.dnd_until)] {
            if let Err(e) = parse_hhmm(v) {
                p.push(format!("`{k}`: {e}"));
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedule() {
        let c = NotificationsConfig { dnd_schedule: true, ..Default::default() };
        assert!(c.scheduled_dnd(23 * 60));
        assert!(c.scheduled_dnd(6 * 60));
        assert!(!c.scheduled_dnd(12 * 60));
        assert!(!NotificationsConfig::default().scheduled_dnd(23 * 60));
    }
}
