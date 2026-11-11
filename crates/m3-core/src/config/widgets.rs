//! `widgets.toml` — the desktop widgets drawn above the wallpaper.
//!
//! Nothing here runs commands; shortcuts are desktop-file ids launched
//! through GIO. Weather is off until the user enters a place, because it
//! asks an outside service (Open-Meteo) for the forecast.

use super::{ConfigFile, check_range};
use serde::{Deserialize, Serialize};

/// Which screen edge the widget column sits at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WidgetSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

/// A "N days to go" countdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Countdown {
    /// Shown above the count, e.g. "Summer holiday". Empty hides the widget.
    pub title: String,
    /// `YYYY-MM-DD`.
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Weather {
    pub enabled: bool,
    /// Place name shown in Settings (e.g. "Warsaw").
    pub place: String,
    pub latitude: f64,
    pub longitude: f64,
    pub unit: TemperatureUnit,
}

impl Default for Weather {
    fn default() -> Self {
        Self { enabled: false, place: String::new(), latitude: 0.0, longitude: 0.0, unit: TemperatureUnit::Celsius }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetsConfig {
    /// Show desktop widgets at all.
    pub enabled: bool,
    /// Big clock + "Good …" greeting on the opposite side.
    pub clock: bool,
    pub greeting: bool,
    /// Name in the greeting pill; empty uses the account's real name.
    pub name: String,
    pub side: WidgetSide,
    pub day: bool,
    pub shortcuts: bool,
    /// Up to three desktop-file ids for the shortcut circles.
    pub shortcut_apps: Vec<String>,
    pub media: bool,
    pub countdown: Countdown,
    pub storage: bool,
    pub system: bool,
    pub weather: Weather,
}

impl Default for WidgetsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            clock: true,
            greeting: true,
            name: String::new(),
            side: WidgetSide::Right,
            day: true,
            shortcuts: true,
            shortcut_apps: vec![
                "org.m3.Browser.desktop".into(),
                "org.m3.Files.desktop".into(),
                "org.m3.Music.desktop".into(),
            ],
            media: true,
            countdown: Countdown::default(),
            storage: true,
            system: true,
            weather: Weather::default(),
        }
    }
}

/// Parse `YYYY-MM-DD` into (year, month, day).
pub fn parse_date(s: &str) -> Result<(i32, u32, u32), String> {
    let err = || format!("`{s}` is not a date like \"2026-12-24\"");
    let mut it = s.trim().splitn(3, '-');
    let (y, m, d) = (it.next().ok_or_else(err)?, it.next().ok_or_else(err)?, it.next().ok_or_else(err)?);
    let (y, m, d): (i32, u32, u32) =
        (y.parse().map_err(|_| err())?, m.parse().map_err(|_| err())?, d.parse().map_err(|_| err())?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(err());
    }
    Ok((y, m, d))
}

impl ConfigFile for WidgetsConfig {
    const FILE_NAME: &'static str = "widgets.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.name.chars().count() > 40 {
            p.push("`name`: at most 40 characters".into());
        }
        if self.shortcut_apps.len() > 3 {
            p.push("`shortcut_apps`: at most three apps".into());
        }
        for id in &self.shortcut_apps {
            if !id.ends_with(".desktop") || id.contains('/') {
                p.push(format!("`shortcut_apps`: `{id}` must be a desktop file id like `firefox.desktop`"));
            }
        }
        if !self.countdown.title.is_empty() {
            if self.countdown.title.chars().count() > 60 {
                p.push("`countdown.title`: at most 60 characters".into());
            }
            if let Err(e) = parse_date(&self.countdown.date) {
                p.push(format!("`countdown.date`: {e}"));
            }
        }
        check_range(&mut p, "weather.latitude", self.weather.latitude, -90.0, 90.0);
        check_range(&mut p, "weather.longitude", self.weather.longitude, -180.0, 180.0);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(WidgetsConfig::default().validate().is_empty());
        assert!(!WidgetsConfig::default().weather.enabled, "weather is opt-in");
    }

    #[test]
    fn dates_and_limits() {
        assert_eq!(parse_date("2026-12-24"), Ok((2026, 12, 24)));
        assert!(parse_date("2026-13-01").is_err());
        assert!(parse_date("tomorrow").is_err());
        let c = WidgetsConfig {
            countdown: Countdown { title: "Holiday".into(), date: "soon".into() },
            shortcut_apps: vec!["a.desktop".into(), "b.desktop".into(), "c.desktop".into(), "d.desktop".into()],
            ..Default::default()
        };
        assert_eq!(c.validate().len(), 2);
    }
}
