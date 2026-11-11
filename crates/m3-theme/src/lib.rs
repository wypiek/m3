//! The m3 design system: Material 3 colour schemes and design tokens.
//!
//! Everything visual in m3 derives from a [`Theme`]: the compositor reads its
//! colours for the background and lock fallback, the shell and settings app
//! render [`css::gtk_css`] into GTK, and the lock screen uses the same tokens.
//!
//! Colours come from Google's Material Color Utilities (via the
//! `material-colors` crate) using the 2025 "Expressive" spec for the
//! variants that support it, plus two m3 extensions: *accent intensity*
//! (scales the chroma of primary/secondary/tertiary palettes) and *surface
//! tint* (scales the chroma of neutral palettes), so accents and surfaces can
//! be tuned independently.

pub mod css;
pub mod scheme;
pub mod tokens;
pub mod wallpaper;

pub use scheme::{Colors, Rgb};
pub use tokens::Tokens;

use m3_core::config::{ColorSource, ThemeConfig, parse_hex_color};

/// The fallback seed when nothing else is available (Material baseline violet).
pub const DEFAULT_SEED: u32 = 0x6750a4;

/// A fully resolved theme.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub seed: u32,
    pub colors: Colors,
    pub tokens: Tokens,
}

impl Theme {
    /// Resolve `cfg` into a theme.
    ///
    /// * `wallpaper_seed` — the seed extracted from the current wallpaper, if
    ///   the source is the wallpaper and one is known.
    /// * `minutes_since_midnight` — local time, for the automatic light/dark
    ///   schedule.
    pub fn resolve(cfg: &ThemeConfig, wallpaper_seed: Option<u32>, minutes_since_midnight: u32) -> Self {
        let seed = seed_for(cfg, wallpaper_seed);
        let dark = cfg.is_dark_at(minutes_since_midnight);
        let contrast = if cfg.high_contrast { cfg.contrast.max(1.0) } else { cfg.contrast };
        let colors = scheme::generate(seed, cfg.variant, dark, contrast, cfg.accent_intensity, cfg.surface_tint);
        let tokens = Tokens::new(cfg.corner_scale, cfg.text_scale, cfg.reduced_motion);
        Self { dark, seed, colors, tokens }
    }
}

/// Which seed colour a configuration selects.
pub fn seed_for(cfg: &ThemeConfig, wallpaper_seed: Option<u32>) -> u32 {
    match cfg.source {
        ColorSource::Wallpaper => wallpaper_seed.or_else(|| parse_hex_color(&cfg.seed_color).ok()),
        ColorSource::Seed => parse_hex_color(&cfg.seed_color).ok(),
        ColorSource::Preset => cfg.preset_color(),
    }
    .unwrap_or(DEFAULT_SEED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3_core::config::{SchemeVariant, ThemeMode};

    #[test]
    fn seed_selection() {
        let mut c = ThemeConfig { source: ColorSource::Seed, seed_color: "#ff0000".into(), ..Default::default() };
        assert_eq!(seed_for(&c, Some(0x00ff00)), 0xff0000);
        c.source = ColorSource::Wallpaper;
        assert_eq!(seed_for(&c, Some(0x00ff00)), 0x00ff00);
        assert_eq!(seed_for(&c, None), 0xff0000, "falls back to the seed colour");
        c.source = ColorSource::Preset;
        c.preset = "teal".into();
        assert_eq!(seed_for(&c, None), 0x006a6a);
    }

    #[test]
    fn mode_follows_config() {
        let mut c = ThemeConfig { mode: ThemeMode::Light, ..Default::default() };
        assert!(!Theme::resolve(&c, None, 0).dark);
        c.mode = ThemeMode::Dark;
        let t = Theme::resolve(&c, None, 0);
        assert!(t.dark);
        assert!(t.colors.surface.luminance() < 0.1);
        c.mode = ThemeMode::Auto;
        assert!(Theme::resolve(&c, None, 22 * 60).dark);
        assert!(!Theme::resolve(&c, None, 12 * 60).dark);
    }

    #[test]
    fn every_variant_resolves() {
        for v in SchemeVariant::ALL {
            for dark in [false, true] {
                let c = ThemeConfig {
                    variant: v,
                    mode: if dark { ThemeMode::Dark } else { ThemeMode::Light },
                    ..Default::default()
                };
                let t = Theme::resolve(&c, Some(0x3366cc), 0);
                assert_eq!(t.dark, dark, "{v:?}");
            }
        }
    }
}
