//! `config.toml` — compositor behaviour, input devices, outputs and windows.

use super::{ConfigFile, check_range};
use m3_protocol::Transform;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CompositorConfig {
    pub general: General,
    pub input: Input,
    /// Per-output settings keyed by connector name (`eDP-1`, `DP-2`, …) or
    /// by `"make model serial"` for stable identification across ports.
    pub outputs: BTreeMap<String, OutputConfig>,
    pub windows: Windows,
    pub animations: Animations,
    pub idle: Idle,
    pub night_light: NightLight,
    #[serde(rename = "window_rule")]
    pub window_rules: Vec<WindowRule>,
}

impl ConfigFile for CompositorConfig {
    const FILE_NAME: &'static str = "config.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        let k = &self.input.keyboard;
        check_range(&mut p, "input.keyboard.repeat_rate", k.repeat_rate, 1, 200);
        check_range(&mut p, "input.keyboard.repeat_delay", k.repeat_delay, 100, 2000);
        check_range(&mut p, "input.pointer.accel_speed", self.input.pointer.accel_speed, -1.0, 1.0);
        check_range(&mut p, "input.touchpad.accel_speed", self.input.touchpad.accel_speed, -1.0, 1.0);
        check_range(&mut p, "input.touchpad.scroll_factor", self.input.touchpad.scroll_factor, 0.1, 10.0);
        check_range(&mut p, "general.cursor_size", self.general.cursor_size, 8, 256);
        for (name, o) in &self.outputs {
            if let Some(s) = o.scale {
                check_range(&mut p, &format!("outputs.{name}.scale"), s, 0.5, 4.0);
            }
            if let Some(m) = &o.mode {
                if let Err(e) = OutputMode::parse(m) {
                    p.push(format!("`outputs.{name}.mode`: {e}"));
                }
            }
        }
        let w = &self.windows;
        check_range(&mut p, "windows.gaps", w.gaps, 0, 64);
        check_range(&mut p, "windows.border_width", w.border_width, 0, 16);
        check_range(&mut p, "windows.corner_radius", w.corner_radius, 0, 48);
        check_range(&mut p, "windows.snap_threshold", w.snap_threshold, 0, 128);
        check_range(&mut p, "windows.master_ratio", w.master_ratio, 0.1, 0.9);
        check_range(&mut p, "animations.speed", self.animations.speed, 0.1, 5.0);
        let n = &self.night_light;
        check_range(&mut p, "night_light.temperature", n.temperature, 1500, 6500);
        for (k, v) in [("night_light.from", &n.from), ("night_light.until", &n.until)] {
            if let Err(e) = super::parse_hhmm(v) {
                p.push(format!("`{k}`: {e}"));
            }
        }
        for (i, r) in self.window_rules.iter().enumerate() {
            if r.app_id.is_none() && r.title.is_none() {
                p.push(format!("window_rule #{}: needs at least one of `app_id` or `title`", i + 1));
            }
            if let Some(o) = r.opacity {
                check_range(&mut p, &format!("window_rule #{}.opacity", i + 1), o, 0.1, 1.0);
            }
            if r.workspace == Some(0) {
                p.push(format!("window_rule #{}: workspaces are numbered from 1", i + 1));
            }
        }
        p
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub focus_follows_mouse: bool,
    /// Give keyboard focus to newly mapped windows.
    pub focus_new_windows: bool,
    /// Start XWayland on demand for X11 applications.
    pub xwayland: bool,
    pub cursor_theme: String,
    pub cursor_size: u32,
    /// Top-left hot corner opens the overview.
    pub hot_corner: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            focus_follows_mouse: false,
            focus_new_windows: true,
            xwayland: true,
            cursor_theme: "default".into(),
            cursor_size: 24,
            hot_corner: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Input {
    pub keyboard: InputKeyboard,
    pub pointer: InputPointer,
    pub touchpad: InputTouchpad,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputKeyboard {
    /// XKB layouts, comma separated (`"us,de"`).
    pub layout: String,
    pub variant: String,
    pub options: String,
    pub model: String,
    /// Repeats per second.
    pub repeat_rate: i32,
    /// Milliseconds before repeat starts.
    pub repeat_delay: i32,
    pub numlock: bool,
}

impl Default for InputKeyboard {
    fn default() -> Self {
        Self {
            layout: "us".into(),
            variant: String::new(),
            options: String::new(),
            model: String::new(),
            repeat_rate: 25,
            repeat_delay: 600,
            numlock: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccelProfile {
    #[default]
    Adaptive,
    Flat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InputPointer {
    /// libinput acceleration speed, -1.0 … 1.0.
    pub accel_speed: f64,
    pub accel_profile: AccelProfile,
    pub natural_scroll: bool,
    pub left_handed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputTouchpad {
    pub tap_to_click: bool,
    pub natural_scroll: bool,
    pub accel_speed: f64,
    pub disable_while_typing: bool,
    /// Click with two/three fingers for right/middle click instead of
    /// bottom-area buttons.
    pub clickfinger: bool,
    pub scroll_factor: f64,
}

impl Default for InputTouchpad {
    fn default() -> Self {
        Self {
            tap_to_click: true,
            natural_scroll: true,
            accel_speed: 0.0,
            disable_while_typing: true,
            clickfinger: true,
            scroll_factor: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub enabled: bool,
    /// `"1920x1080"` or `"1920x1080@60.000"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    pub transform: Transform,
    /// Logical position of the top-left corner in the global space.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<[i32; 2]>,
    pub primary: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self { enabled: true, mode: None, scale: None, transform: Transform::Normal, position: None, primary: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputMode {
    pub width: i32,
    pub height: i32,
    /// Refresh rate in millihertz.
    pub refresh_mhz: Option<i32>,
}

impl OutputMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        let err = || format!("`{s}` is not a mode like \"1920x1080\" or \"1920x1080@59.94\"");
        let (res, refresh) = match s.split_once('@') {
            Some((r, hz)) => {
                let hz: f64 = hz.trim().parse().map_err(|_| err())?;
                if !(1.0..=1000.0).contains(&hz) {
                    return Err(err());
                }
                (r, Some((hz * 1000.0).round() as i32))
            }
            None => (s, None),
        };
        let (w, h) = res.trim().split_once('x').ok_or_else(err)?;
        let width: i32 = w.parse().map_err(|_| err())?;
        let height: i32 = h.parse().map_err(|_| err())?;
        if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
            return Err(err());
        }
        Ok(Self { width, height, refresh_mhz: refresh })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecorationMode {
    /// Let clients draw their own decorations (GTK, libadwaita, Electron with
    /// CSD); the compositor draws Material title bars for clients that ask
    /// for server-side decorations.
    #[default]
    PreferClient,
    /// Ask every client that supports xdg-decoration to let m3 decorate it.
    PreferServer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Windows {
    pub decorations: DecorationMode,
    /// Gap between tiled windows, logical pixels.
    pub gaps: i32,
    pub border_width: i32,
    pub corner_radius: i32,
    pub shadows: bool,
    /// Distance at which windows snap to output edges and other windows.
    pub snap_threshold: i32,
    /// Fraction of the output used by the master area in the tiling layout.
    pub master_ratio: f64,
    /// Dim unfocused windows slightly.
    pub dim_inactive: bool,
}

impl Default for Windows {
    fn default() -> Self {
        Self {
            decorations: DecorationMode::PreferClient,
            gaps: 8,
            border_width: 2,
            corner_radius: 16,
            shadows: true,
            snap_threshold: 16,
            master_ratio: 0.55,
            dim_inactive: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animations {
    pub enabled: bool,
    /// Multiplier for animation speed: 2.0 is twice as fast.
    pub speed: f64,
}

impl Default for Animations {
    fn default() -> Self {
        Self { enabled: true, speed: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Idle {
    /// Seconds of inactivity before the screen locks. 0 disables.
    pub lock_after: u32,
    /// Seconds of inactivity before outputs are powered off. 0 disables.
    pub screen_off_after: u32,
    /// Seconds of inactivity before suspending. 0 disables.
    pub suspend_after: u32,
    /// Lock the session before the system suspends.
    pub lock_on_suspend: bool,
}

/// When night light is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NightLightMode {
    #[default]
    Off,
    Always,
    /// Between `from` and `until` (local time).
    Scheduled,
}

/// Warmer screen colours in the evening (display gamma).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NightLight {
    pub mode: NightLightMode,
    /// Colour temperature in kelvin while on (1500 … 6500; lower is warmer).
    pub temperature: u32,
    /// `HH:MM`, for [`NightLightMode::Scheduled`].
    pub from: String,
    pub until: String,
}

impl Default for NightLight {
    fn default() -> Self {
        Self { mode: NightLightMode::Off, temperature: 4000, from: "20:00".into(), until: "07:00".into() }
    }
}

impl NightLight {
    /// Whether night light applies at `minutes` since local midnight.
    pub fn active_at(&self, minutes: u32) -> bool {
        match self.mode {
            NightLightMode::Off => false,
            NightLightMode::Always => true,
            NightLightMode::Scheduled => {
                let (Ok(from), Ok(until)) = (super::parse_hhmm(&self.from), super::parse_hhmm(&self.until)) else {
                    return false;
                };
                if from <= until { minutes >= from && minutes < until } else { minutes >= from || minutes < until }
            }
        }
    }
}

impl Default for Idle {
    fn default() -> Self {
        Self { lock_after: 300, screen_off_after: 600, suspend_after: 0, lock_on_suspend: true }
    }
}

/// Per-application window preferences. All `Option` fields left unset have
/// no effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowRule {
    /// Glob matched against the app id / X11 class (`*` and `?`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Glob matched against the window title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floating: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fullscreen: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<[i32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

impl WindowRule {
    pub fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        let field = |pat: &Option<String>, val: Option<&str>| match pat {
            None => true,
            Some(p) => crate::glob::matches(p, val.unwrap_or("")),
        };
        (self.app_id.is_some() || self.title.is_some()) && field(&self.app_id, app_id) && field(&self.title, title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parsing() {
        assert_eq!(
            OutputMode::parse("2560x1440@143.9").unwrap(),
            OutputMode { width: 2560, height: 1440, refresh_mhz: Some(143_900) }
        );
        assert_eq!(OutputMode::parse("800x600").unwrap().refresh_mhz, None);
        assert!(OutputMode::parse("800").is_err());
        assert!(OutputMode::parse("0x600").is_err());
        assert!(OutputMode::parse("800x600@0").is_err());
    }

    #[test]
    fn window_rules() {
        let r = WindowRule { app_id: Some("org.gnome.*".into()), floating: Some(true), ..Default::default() };
        assert!(r.matches(Some("org.gnome.Calculator"), None));
        assert!(!r.matches(Some("firefox"), Some("org.gnome")));
        assert!(!WindowRule::default().matches(Some("x"), Some("y")));
    }

    #[test]
    fn rules_parse_from_toml() {
        let cfg: CompositorConfig = toml::from_str(
            r#"
            [[window_rule]]
            app_id = "pavucontrol"
            floating = true
            size = [640, 480]

            [outputs.eDP-1]
            scale = 1.25
            transform = "90"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.window_rules[0].size, Some([640, 480]));
        assert_eq!(cfg.outputs["eDP-1"].scale, Some(1.25));
        assert_eq!(cfg.outputs["eDP-1"].transform, Transform::Rotate90);
        assert!(cfg.validate().is_empty());
    }
}
