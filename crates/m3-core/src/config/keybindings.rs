//! `keybindings.toml` — keyboard shortcuts.
//!
//! ```toml
//! schema_version = 1
//! inherit_defaults = true          # start from m3's default bindings
//!
//! [bindings]
//! "Super+Return" = { action = "spawn", command = ["foot"] }
//! "Super+Shift+S" = { action = "screenshot" }
//! "Super+Q" = "disabled"           # remove a default binding
//! ```
//!
//! Commands are executed directly as an argv (no shell interpretation).

use super::ConfigFile;
use m3_protocol::{Action, Direction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybindings {
    pub inherit_defaults: bool,
    pub bindings: BTreeMap<String, Binding>,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self { inherit_defaults: true, bindings: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Binding {
    Disabled(Disabled),
    Action(Action),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disabled {
    Disabled,
}

impl ConfigFile for Keybindings {
    const FILE_NAME: &'static str = "keybindings.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        let mut seen: BTreeMap<KeyCombo, &str> = BTreeMap::new();
        for (combo, binding) in &self.bindings {
            match KeyCombo::parse(combo) {
                Ok(c) => {
                    if let Some(prev) = seen.insert(c.clone(), combo) {
                        p.push(format!("`{combo}` and `{prev}` are the same shortcut"));
                    }
                }
                Err(e) => p.push(e),
            }
            if let Binding::Action(Action::Spawn { command }) = binding {
                if command.is_empty() || command[0].trim().is_empty() {
                    p.push(format!("`{combo}`: spawn needs a non-empty `command` list"));
                }
            }
        }
        p
    }
}

impl Keybindings {
    /// The effective bindings: defaults (if inherited) overlaid with the
    /// user's bindings, minus disabled ones. Unparseable combos are skipped
    /// (validation reports them).
    pub fn effective(&self) -> Vec<(KeyCombo, Action)> {
        let mut map: BTreeMap<KeyCombo, Option<Action>> = BTreeMap::new();
        if self.inherit_defaults {
            for (combo, action) in default_bindings() {
                if let Ok(c) = KeyCombo::parse(&combo) {
                    map.insert(c, Some(action));
                }
            }
        }
        for (combo, binding) in &self.bindings {
            if let Ok(c) = KeyCombo::parse(combo) {
                map.insert(
                    c,
                    match binding {
                        Binding::Disabled(_) => None,
                        Binding::Action(a) => Some(a.clone()),
                    },
                );
            }
        }
        map.into_iter().filter_map(|(c, a)| a.map(|a| (c, a))).collect()
    }
}

/// m3's default shortcuts.
pub fn default_bindings() -> Vec<(String, Action)> {
    use Action::*;
    let mut b: Vec<(String, Action)> = vec![
        ("Super+Return".into(), SpawnTerminal),
        ("Super+Space".into(), ToggleLauncher),
        ("Super+A".into(), ToggleLauncher),
        ("Super+Tab".into(), ToggleOverview),
        ("Super+S".into(), ToggleQuickSettings),
        ("Super+N".into(), ToggleNotifications),
        ("Alt+Tab".into(), CycleWindows { reverse: false }),
        ("Alt+Shift+Tab".into(), CycleWindows { reverse: true }),
        ("Super+Q".into(), CloseWindow),
        ("Alt+F4".into(), CloseWindow),
        ("Super+F".into(), ToggleFullscreen),
        ("Super+Up".into(), ToggleMaximize),
        ("Super+Down".into(), Minimize),
        ("Super+Left".into(), Snap { direction: Direction::Left }),
        ("Super+Right".into(), Snap { direction: Direction::Right }),
        ("Super+T".into(), ToggleFloating),
        ("Super+Ctrl+Space".into(), CycleLayout),
        ("Super+J".into(), FocusNext),
        ("Super+K".into(), FocusPrevious),
        ("Super+Ctrl+Left".into(), PreviousWorkspace),
        ("Super+Ctrl+Right".into(), NextWorkspace),
        ("Super+Page_Up".into(), PreviousWorkspace),
        ("Super+Page_Down".into(), NextWorkspace),
        ("Super+L".into(), Lock),
        ("Print".into(), Screenshot),
        ("Shift+Print".into(), ScreenshotRegion),
        ("Super+Shift+S".into(), ScreenshotRegion),
        ("XF86AudioRaiseVolume".into(), VolumeUp),
        ("XF86AudioLowerVolume".into(), VolumeDown),
        ("XF86AudioMute".into(), VolumeMute),
        ("XF86AudioMicMute".into(), MicMute),
        ("XF86MonBrightnessUp".into(), BrightnessUp),
        ("XF86MonBrightnessDown".into(), BrightnessDown),
        ("XF86AudioPlay".into(), MediaPlayPause),
        ("XF86AudioNext".into(), MediaNext),
        ("XF86AudioPrev".into(), MediaPrevious),
    ];
    for i in 1..=9 {
        b.push((format!("Super+{i}"), SwitchWorkspace { index: i }));
        b.push((format!("Super+Shift+{i}"), MoveToWorkspace { index: i }));
    }
    b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

/// A parsed shortcut. `key` is an XKB keysym name; single letters are stored
/// lowercase so `Super+Q` and `Super+q` are the same binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyCombo {
    pub modifiers: Modifiers,
    pub key: String,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut m = Modifiers::default();
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = match parts.split_last() {
            Some((k, mods)) if !k.is_empty() => (*k, mods),
            // "Super++" style: the key itself is '+'.
            _ if s.ends_with("++") => ("plus", &parts[..parts.len().saturating_sub(2)]),
            _ => return Err(format!("`{s}`: missing key after the last `+`")),
        };
        for md in mods {
            match md.to_ascii_lowercase().as_str() {
                "super" | "logo" | "mod4" | "win" | "meta" => m.logo = true,
                "ctrl" | "control" => m.ctrl = true,
                "alt" | "mod1" => m.alt = true,
                "shift" => m.shift = true,
                other => return Err(format!("`{s}`: unknown modifier `{other}`")),
            }
        }
        let key = if key.chars().count() == 1 { key.to_lowercase() } else { key.to_owned() };
        Ok(Self { modifiers: m, key })
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        if m.logo {
            f.write_str("Super+")?;
        }
        if m.ctrl {
            f.write_str("Ctrl+")?;
        }
        if m.alt {
            f.write_str("Alt+")?;
        }
        if m.shift {
            f.write_str("Shift+")?;
        }
        let k = if self.key.chars().count() == 1 { self.key.to_uppercase() } else { self.key.clone() };
        f.write_str(&k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_combos() {
        let c = KeyCombo::parse("Super+Shift+Q").unwrap();
        assert!(c.modifiers.logo && c.modifiers.shift && !c.modifiers.ctrl);
        assert_eq!(c.key, "q");
        assert_eq!(c, KeyCombo::parse("logo+shift+q").unwrap());
        assert_eq!(c.to_string(), "Super+Shift+Q");
        assert_eq!(KeyCombo::parse("Print").unwrap().key, "Print");
        assert!(KeyCombo::parse("Hyper+x").is_err());
        assert!(KeyCombo::parse("Super+").is_err());
    }

    #[test]
    fn defaults_are_valid_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (c, _) in default_bindings() {
            let k = KeyCombo::parse(&c).unwrap();
            assert!(seen.insert(k), "duplicate default {c}");
        }
    }

    #[test]
    fn overrides_and_disabling() {
        let kb: Keybindings = toml::from_str(
            r#"
            [bindings]
            "Super+Q" = "disabled"
            "Super+Return" = { action = "spawn", command = ["foot", "-e", "htop"] }
            "Super+Shift+X" = { action = "quit" }
            "#,
        )
        .unwrap();
        assert!(kb.validate().is_empty(), "{:?}", kb.validate());
        let eff = kb.effective();
        let get = |s: &str| {
            let k = KeyCombo::parse(s).unwrap();
            eff.iter().find(|(c, _)| *c == k).map(|(_, a)| a.clone())
        };
        assert_eq!(get("Super+Q"), None);
        assert_eq!(get("Super+Shift+X"), Some(Action::Quit));
        assert_eq!(
            get("Super+Return"),
            Some(Action::Spawn { command: vec!["foot".into(), "-e".into(), "htop".into()] })
        );
        assert_eq!(get("Super+1"), Some(Action::SwitchWorkspace { index: 1 }));
    }

    #[test]
    fn invalid_bindings_are_reported() {
        let kb: Keybindings = toml::from_str(
            r#"
            [bindings]
            "Hyper+Q" = { action = "quit" }
            "Super+E" = { action = "spawn", command = [] }
            "Super+X" = { action = "quit" }
            "super+x" = { action = "lock" }
            "#,
        )
        .unwrap();
        let p = kb.validate();
        assert_eq!(p.len(), 3, "{p:?}");
    }
}
