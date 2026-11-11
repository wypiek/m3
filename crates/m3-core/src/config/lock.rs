//! `lock.toml` — how the lock screen looks.
//!
//! Only appearance: nothing here can weaken authentication or run commands.

use super::{ClockFormat, ConfigFile, check_range};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What fills the lock screen behind the clock and card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LockBackground {
    /// The desktop wallpaper from `theme.toml` (per-output overrides too).
    #[default]
    Wallpaper,
    /// `image`.
    Image,
    /// A theme-coloured gradient.
    Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LockClockSize {
    Small,
    Medium,
    #[default]
    Large,
}

impl LockClockSize {
    /// Clock font size in logical pixels.
    pub fn pixels(self) -> u32 {
        match self {
            LockClockSize::Small => 72,
            LockClockSize::Medium => 96,
            LockClockSize::Large => 128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LockClockStyle {
    /// Heavy, wide and rounded (Google Sans Flex variations).
    #[default]
    Expressive,
    /// Thin and wide.
    Light,
    /// Regular weight.
    Classic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LockClockPosition {
    /// Clock above the card, centred.
    #[default]
    Center,
    /// Clock at the top, card in the middle.
    Top,
    /// Clock on the left, card on the right.
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DateFormat {
    /// `Tue 10 Nov`
    Short,
    /// `Tuesday, 10 November`
    #[default]
    Long,
}

impl DateFormat {
    pub fn pattern(self) -> &'static str {
        match self {
            DateFormat::Short => "%a %-d %b",
            DateFormat::Long => "%A, %-d %B",
        }
    }
}

/// Shape of the user picture (Material 3 Expressive shapes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AvatarShape {
    Circle,
    Square,
    Cookie4,
    Cookie9,
    #[default]
    Cookie12,
    Clover4,
    Clover8,
    Sunny,
    Flower,
    Burst,
    Pentagon,
}

impl AvatarShape {
    pub const ALL: [AvatarShape; 11] = [
        AvatarShape::Circle,
        AvatarShape::Square,
        AvatarShape::Cookie4,
        AvatarShape::Cookie9,
        AvatarShape::Cookie12,
        AvatarShape::Clover4,
        AvatarShape::Clover8,
        AvatarShape::Sunny,
        AvatarShape::Flower,
        AvatarShape::Burst,
        AvatarShape::Pentagon,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AvatarShape::Circle => "Circle",
            AvatarShape::Square => "Rounded square",
            AvatarShape::Cookie4 => "Cookie (4)",
            AvatarShape::Cookie9 => "Cookie (9)",
            AvatarShape::Cookie12 => "Cookie (12)",
            AvatarShape::Clover4 => "Clover (4)",
            AvatarShape::Clover8 => "Clover (8)",
            AvatarShape::Sunny => "Sunny",
            AvatarShape::Flower => "Flower",
            AvatarShape::Burst => "Burst",
            AvatarShape::Pentagon => "Pentagon",
        }
    }
}

/// Longest owner message shown on the lock screen.
pub const LOCK_MESSAGE_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockConfig {
    pub background: LockBackground,
    /// Image for [`LockBackground::Image`].
    pub image: PathBuf,
    /// Background blur radius in logical pixels (0 = sharp).
    pub blur: u32,
    /// Darkening over the background, 0 (none) … 0.9.
    pub dim: f64,

    pub clock_format: ClockFormat,
    pub clock_seconds: bool,
    pub clock_size: LockClockSize,
    pub clock_style: LockClockStyle,
    pub clock_position: LockClockPosition,
    pub show_date: bool,
    pub date_format: DateFormat,

    /// Show the user's picture and name.
    pub show_user: bool,
    pub avatar_shape: AvatarShape,
    /// Plain text shown under the card ("If found, please call …").
    pub message: String,

    /// Now-playing card with play/pause and next (MPRIS).
    pub show_media: bool,
    /// Battery level chip (only when the device has a battery).
    pub show_battery: bool,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            background: LockBackground::Wallpaper,
            image: PathBuf::new(),
            blur: 0,
            dim: 0.32,
            clock_format: ClockFormat::H24,
            clock_seconds: false,
            clock_size: LockClockSize::Large,
            clock_style: LockClockStyle::Expressive,
            clock_position: LockClockPosition::Center,
            show_date: true,
            date_format: DateFormat::Long,
            show_user: true,
            avatar_shape: AvatarShape::Cookie12,
            message: String::new(),
            show_media: true,
            show_battery: true,
        }
    }
}

impl LockConfig {
    /// `strftime` pattern for the clock.
    pub fn time_pattern(&self) -> &'static str {
        match (self.clock_format, self.clock_seconds) {
            (ClockFormat::H24, false) => "%H:%M",
            (ClockFormat::H24, true) => "%H:%M:%S",
            (ClockFormat::H12, false) => "%-I:%M",
            (ClockFormat::H12, true) => "%-I:%M:%S",
        }
    }

    /// The owner message, trimmed and limited to [`LOCK_MESSAGE_MAX`]
    /// characters on a single paragraph per line.
    pub fn message_text(&self) -> String {
        self.message.trim().chars().filter(|c| !c.is_control() || *c == '\n').take(LOCK_MESSAGE_MAX).collect()
    }
}

impl ConfigFile for LockConfig {
    const FILE_NAME: &'static str = "lock.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        check_range(&mut p, "blur", self.blur, 0, 64);
        check_range(&mut p, "dim", self.dim, 0.0, 0.9);
        if self.background == LockBackground::Image && self.image.as_os_str().is_empty() {
            p.push("`image` must be set when `background = \"image\"`".into());
        }
        if !self.image.as_os_str().is_empty() && !self.image.is_absolute() {
            p.push(format!("`image` must be an absolute path (found `{}`)", self.image.display()));
        }
        let n = self.message.chars().count();
        if n > LOCK_MESSAGE_MAX {
            p.push(format!("`message` is {n} characters long; the limit is {LOCK_MESSAGE_MAX}"));
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(LockConfig::default().validate().is_empty());
    }

    #[test]
    fn validation() {
        let c = LockConfig { blur: 65, dim: 0.95, background: LockBackground::Image, ..Default::default() };
        let p = c.validate();
        assert_eq!(p.len(), 3, "{p:?}");
        let c = LockConfig { image: "relative.png".into(), ..Default::default() };
        assert_eq!(c.validate().len(), 1);
        let c = LockConfig { message: "x".repeat(201), ..Default::default() };
        assert_eq!(c.validate().len(), 1);
    }

    #[test]
    fn time_patterns() {
        let mut c = LockConfig::default();
        assert_eq!(c.time_pattern(), "%H:%M");
        c.clock_format = ClockFormat::H12;
        c.clock_seconds = true;
        assert_eq!(c.time_pattern(), "%-I:%M:%S");
        assert_eq!(DateFormat::Short.pattern(), "%a %-d %b");
    }

    #[test]
    fn message_is_plain_and_bounded() {
        let c = LockConfig { message: format!("  hi\u{7}{}  ", "y".repeat(300)), ..Default::default() };
        let m = c.message_text();
        assert!(m.starts_with("hi") && !m.contains('\u{7}'));
        assert_eq!(m.chars().count(), LOCK_MESSAGE_MAX);
    }

    #[test]
    fn parses_snake_case() {
        let c: LockConfig = toml::from_str(
            "background = \"color\"\nclock_style = \"light\"\nclock_position = \"left\"\navatar_shape = \"clover8\"\n",
        )
        .expect("parse");
        assert_eq!(c.background, LockBackground::Color);
        assert_eq!(c.clock_style, LockClockStyle::Light);
        assert_eq!(c.clock_position, LockClockPosition::Left);
        assert_eq!(c.avatar_shape, AvatarShape::Clover8);
    }
}
