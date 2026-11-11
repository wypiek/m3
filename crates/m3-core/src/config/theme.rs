//! `theme.toml` — colours, appearance and wallpaper.

use super::{ConfigFile, check_range};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
    /// Dark between `dark_from` and `dark_until` (local time).
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorSource {
    /// Extract the seed colour from the current wallpaper.
    #[default]
    Wallpaper,
    /// Use `seed_color`.
    Seed,
    /// Use a named preset from [`PRESETS`].
    Preset,
}

/// Material 3 dynamic scheme variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemeVariant {
    TonalSpot,
    Vibrant,
    Expressive,
    /// Default: keeps the source's character (warm wine + gold for m3's
    /// default seed, wallpaper colours otherwise).
    #[default]
    Content,
    Fidelity,
    Neutral,
    Monochrome,
    Rainbow,
    FruitSalad,
}

impl SchemeVariant {
    pub const ALL: [SchemeVariant; 9] = [
        SchemeVariant::TonalSpot,
        SchemeVariant::Vibrant,
        SchemeVariant::Expressive,
        SchemeVariant::Content,
        SchemeVariant::Fidelity,
        SchemeVariant::Neutral,
        SchemeVariant::Monochrome,
        SchemeVariant::Rainbow,
        SchemeVariant::FruitSalad,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SchemeVariant::TonalSpot => "Tonal spot",
            SchemeVariant::Vibrant => "Vibrant",
            SchemeVariant::Expressive => "Expressive",
            SchemeVariant::Content => "Content",
            SchemeVariant::Fidelity => "Fidelity",
            SchemeVariant::Neutral => "Neutral",
            SchemeVariant::Monochrome => "Monochrome",
            SchemeVariant::Rainbow => "Rainbow",
            SchemeVariant::FruitSalad => "Fruit salad",
        }
    }
}

/// Built-in seed colours, (id, label, hex).
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("wine", "Wine", "#8c4a5c"),
    ("violet", "Violet", "#6750a4"),
    ("ocean", "Ocean", "#0061a4"),
    ("teal", "Teal", "#006a6a"),
    ("forest", "Forest", "#386a20"),
    ("sunflower", "Sunflower", "#7c5800"),
    ("coral", "Coral", "#a73a2b"),
    ("rose", "Rose", "#9c4060"),
    ("slate", "Slate", "#5b5f71"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WallpaperFit {
    /// Scale to cover the output, cropping the overflow.
    #[default]
    Fill,
    /// Scale to fit inside the output, letterboxed with the surface colour.
    Fit,
    Center,
    Tile,
    Stretch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wallpaper {
    /// Image file. Empty means the built-in default.
    pub path: PathBuf,
    pub fit: WallpaperFit,
    /// Per-output overrides keyed by output name.
    pub outputs: BTreeMap<String, PathBuf>,
    /// Rotate through images in this directory. Empty disables the slideshow.
    pub slideshow_dir: PathBuf,
    pub slideshow_minutes: u32,
    pub slideshow_shuffle: bool,
    /// Cross-fade duration in milliseconds when the wallpaper changes.
    pub transition_ms: u32,
}

impl Default for Wallpaper {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            fit: WallpaperFit::Fill,
            outputs: BTreeMap::new(),
            slideshow_dir: PathBuf::new(),
            slideshow_minutes: 30,
            slideshow_shuffle: true,
            transition_ms: 600,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub mode: ThemeMode,
    /// `HH:MM`, local time, for [`ThemeMode::Auto`].
    pub dark_from: String,
    pub dark_until: String,
    pub source: ColorSource,
    /// `#rrggbb`.
    pub seed_color: String,
    pub preset: String,
    pub variant: SchemeVariant,
    /// Material contrast level: -1 (reduced) … 0 (standard) … 1 (high).
    pub contrast: f64,
    /// Multiplier for accent chroma: 0 is grey, 1 is the scheme default,
    /// 2 is very saturated.
    pub accent_intensity: f64,
    /// Multiplier for the chroma of neutral (surface) colours.
    pub surface_tint: f64,
    /// Scale for every corner radius token (0 = square, 1 = default).
    pub corner_scale: f64,
    pub font_family: String,
    pub monospace_font_family: String,
    /// Text scale factor (accessibility "larger text").
    pub text_scale: f64,
    pub icon_theme: String,
    /// Translucent panels and popovers.
    pub transparency: bool,
    pub reduced_motion: bool,
    pub high_contrast: bool,
    pub wallpaper: Wallpaper,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            mode: ThemeMode::Dark,
            dark_from: "19:00".into(),
            dark_until: "07:00".into(),
            source: ColorSource::Wallpaper,
            seed_color: "#8c4a5c".into(),
            preset: "wine".into(),
            variant: SchemeVariant::Content,
            contrast: 0.0,
            accent_intensity: 1.0,
            surface_tint: 1.0,
            corner_scale: 1.0,
            font_family: "Google Sans Flex".into(),
            monospace_font_family: "JetBrains Mono".into(),
            text_scale: 1.0,
            icon_theme: "Adwaita".into(),
            transparency: true,
            reduced_motion: false,
            high_contrast: false,
            wallpaper: Wallpaper::default(),
        }
    }
}

/// Parse `#rgb` / `#rrggbb` (the `#` is optional) into `0xRRGGBB`.
pub fn parse_hex_color(s: &str) -> Result<u32, String> {
    let h = s.trim().trim_start_matches('#');
    let full: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_owned(),
        _ => return Err(format!("`{s}` is not a colour like \"#6750a4\"")),
    };
    u32::from_str_radix(&full, 16).map_err(|_| format!("`{s}` is not a colour like \"#6750a4\""))
}

/// Parse `HH:MM` into minutes since midnight.
pub fn parse_hhmm(s: &str) -> Result<u32, String> {
    let err = || format!("`{s}` is not a time like \"19:30\"");
    let (h, m) = s.trim().split_once(':').ok_or_else(err)?;
    let h: u32 = h.parse().map_err(|_| err())?;
    let m: u32 = m.parse().map_err(|_| err())?;
    if h > 23 || m > 59 {
        return Err(err());
    }
    Ok(h * 60 + m)
}

impl ThemeConfig {
    /// Whether dark mode applies at `minutes` since local midnight.
    pub fn is_dark_at(&self, minutes: u32) -> bool {
        match self.mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::Auto => {
                let (Ok(from), Ok(until)) = (parse_hhmm(&self.dark_from), parse_hhmm(&self.dark_until)) else {
                    return true;
                };
                if from <= until { minutes >= from && minutes < until } else { minutes >= from || minutes < until }
            }
        }
    }

    pub fn preset_color(&self) -> Option<u32> {
        PRESETS.iter().find(|(id, _, _)| *id == self.preset).and_then(|(_, _, hex)| parse_hex_color(hex).ok())
    }
}

impl ConfigFile for ThemeConfig {
    const FILE_NAME: &'static str = "theme.toml";
    const SCHEMA_VERSION: u32 = 1;

    fn validate(&self) -> Vec<String> {
        let mut p = Vec::new();
        if let Err(e) = parse_hex_color(&self.seed_color) {
            p.push(format!("`seed_color`: {e}"));
        }
        for (k, v) in [("dark_from", &self.dark_from), ("dark_until", &self.dark_until)] {
            if let Err(e) = parse_hhmm(v) {
                p.push(format!("`{k}`: {e}"));
            }
        }
        if self.source == ColorSource::Preset && self.preset_color().is_none() {
            let ids: Vec<&str> = PRESETS.iter().map(|p| p.0).collect();
            p.push(format!("`preset`: unknown preset `{}` (available: {})", self.preset, ids.join(", ")));
        }
        check_range(&mut p, "contrast", self.contrast, -1.0, 1.0);
        check_range(&mut p, "accent_intensity", self.accent_intensity, 0.0, 2.0);
        check_range(&mut p, "surface_tint", self.surface_tint, 0.0, 3.0);
        check_range(&mut p, "corner_scale", self.corner_scale, 0.0, 2.0);
        check_range(&mut p, "text_scale", self.text_scale, 0.5, 3.0);
        check_range(&mut p, "wallpaper.slideshow_minutes", self.wallpaper.slideshow_minutes, 1, 24 * 60);
        check_range(&mut p, "wallpaper.transition_ms", self.wallpaper.transition_ms, 0, 5000);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex() {
        assert_eq!(parse_hex_color("#6750a4").unwrap(), 0x6750a4);
        assert_eq!(parse_hex_color("fff").unwrap(), 0xffffff);
        assert!(parse_hex_color("#12345").is_err());
        assert!(parse_hex_color("#gggggg").is_err());
    }

    #[test]
    fn auto_schedule_wraps_midnight() {
        let t = ThemeConfig { mode: ThemeMode::Auto, ..Default::default() };
        assert!(t.is_dark_at(parse_hhmm("23:00").unwrap()));
        assert!(t.is_dark_at(parse_hhmm("06:59").unwrap()));
        assert!(!t.is_dark_at(parse_hhmm("07:00").unwrap()));
        assert!(!t.is_dark_at(parse_hhmm("12:00").unwrap()));
        let t = ThemeConfig { mode: ThemeMode::Auto, dark_from: "01:00".into(), dark_until: "05:00".into(), ..t };
        assert!(t.is_dark_at(120));
        assert!(!t.is_dark_at(0));
    }

    #[test]
    fn presets_parse() {
        for (id, _, hex) in PRESETS {
            assert!(parse_hex_color(hex).is_ok(), "{id}");
        }
    }
}
