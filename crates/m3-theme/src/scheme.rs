//! Material 3 colour roles generated from a seed colour.

use m3_core::config::SchemeVariant;
use material_colors::{
    color::Rgb as McRgb,
    dynamic_color::{DynamicScheme, Platform, SpecVersion, Variant},
    hct::Hct,
    palette::TonalPalette,
};
use serde::{Deserialize, Serialize};

/// An sRGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn from_u32(v: u32) -> Self {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn to_u32(self) -> u32 {
        (u32::from(self.0) << 16) | (u32::from(self.1) << 8) | u32::from(self.2)
    }

    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// `rgba(r, g, b, a)` for CSS.
    pub fn rgba(self, alpha: f64) -> String {
        format!("rgba({}, {}, {}, {:.3})", self.0, self.1, self.2, alpha.clamp(0.0, 1.0))
    }

    /// Components as linear-light-agnostic 0..1 floats (for GL clear colours).
    pub fn to_f32(self) -> [f32; 3] {
        [self.0 as f32 / 255.0, self.1 as f32 / 255.0, self.2 as f32 / 255.0]
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f64 {
        let lin = |c: u8| {
            let c = c as f64 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.0) + 0.7152 * lin(self.1) + 0.0722 * lin(self.2)
    }

    /// WCAG contrast ratio between two colours (1..21).
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Alpha-composite `over` on top of `self`.
    pub fn mix(self, over: Rgb, alpha: f64) -> Rgb {
        let m = |a: u8, b: u8| ((a as f64) * (1.0 - alpha) + (b as f64) * alpha).round() as u8;
        Rgb(m(self.0, over.0), m(self.1, over.1), m(self.2, over.2))
    }
}

impl From<McRgb> for Rgb {
    fn from(c: McRgb) -> Self {
        Rgb(c.red, c.green, c.blue)
    }
}

macro_rules! colors {
    ($($field:ident),* $(,)?) => {
        /// Every Material 3 colour role.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub struct Colors {
            $(pub $field: Rgb,)*
        }

        impl Colors {
            fn from_scheme(s: &DynamicScheme) -> Self {
                Self { $($field: s.$field().into(),)* }
            }

            /// `(css-name, colour)` pairs, e.g. `("on-primary-container", …)`.
            pub fn roles(&self) -> Vec<(String, Rgb)> {
                vec![$((stringify!($field).replace('_', "-"), self.$field),)*]
            }
        }
    };
}

colors! {
    primary, on_primary, primary_container, on_primary_container, inverse_primary,
    primary_fixed, primary_fixed_dim, on_primary_fixed, on_primary_fixed_variant,
    secondary, on_secondary, secondary_container, on_secondary_container,
    secondary_fixed, secondary_fixed_dim, on_secondary_fixed, on_secondary_fixed_variant,
    tertiary, on_tertiary, tertiary_container, on_tertiary_container,
    tertiary_fixed, tertiary_fixed_dim, on_tertiary_fixed, on_tertiary_fixed_variant,
    error, on_error, error_container, on_error_container,
    background, on_background,
    surface, on_surface, surface_variant, on_surface_variant,
    surface_dim, surface_bright, surface_tint,
    surface_container_lowest, surface_container_low, surface_container,
    surface_container_high, surface_container_highest,
    inverse_surface, inverse_on_surface,
    outline, outline_variant, shadow, scrim,
}

fn variant(v: SchemeVariant) -> Variant {
    match v {
        SchemeVariant::TonalSpot => Variant::TonalSpot,
        SchemeVariant::Vibrant => Variant::Vibrant,
        SchemeVariant::Expressive => Variant::Expressive,
        SchemeVariant::Content => Variant::Content,
        SchemeVariant::Fidelity => Variant::Fidelity,
        SchemeVariant::Neutral => Variant::Neutral,
        SchemeVariant::Monochrome => Variant::Monochrome,
        SchemeVariant::Rainbow => Variant::Rainbow,
        SchemeVariant::FruitSalad => Variant::FruitSalad,
    }
}

fn scale_chroma(p: TonalPalette, k: f64) -> TonalPalette {
    if (k - 1.0).abs() < f64::EPSILON {
        return p;
    }
    TonalPalette::from_hue_and_chroma(p.hue(), (p.chroma() * k).max(0.0))
}

/// Generate all colour roles.
///
/// * `contrast` −1 … 1 (Material contrast level).
/// * `accent` multiplies the chroma of the accent palettes.
/// * `surface_tint` multiplies the chroma of the neutral palettes.
pub fn generate(seed: u32, v: SchemeVariant, dark: bool, contrast: f64, accent: f64, surface_tint: f64) -> Colors {
    let rgb = McRgb::new((seed >> 16) as u8, (seed >> 8) as u8, seed as u8);
    let mut scheme = DynamicScheme::from_spec(
        Hct::new(rgb),
        variant(v),
        dark,
        Some(contrast.clamp(-1.0, 1.0)),
        Platform::Phone,
        SpecVersion::Spec2025,
    );
    scheme.primary_palette = scale_chroma(scheme.primary_palette, accent);
    scheme.secondary_palette = scale_chroma(scheme.secondary_palette, accent);
    scheme.tertiary_palette = scale_chroma(scheme.tertiary_palette, accent);
    scheme.neutral_palette = scale_chroma(scheme.neutral_palette, surface_tint);
    scheme.neutral_variant_palette = scale_chroma(scheme.neutral_variant_palette, surface_tint);
    Colors::from_scheme(&scheme)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_variants() -> impl Iterator<Item = SchemeVariant> {
        SchemeVariant::ALL.into_iter()
    }

    #[test]
    fn hex_roundtrip() {
        let c = Rgb::from_u32(0x6750a4);
        assert_eq!(c.hex(), "#6750a4");
        assert_eq!(c.to_u32(), 0x6750a4);
    }

    #[test]
    fn text_on_surfaces_is_readable() {
        // WCAG AA (4.5:1) for body text on every surface level, every variant,
        // light and dark, for a range of seeds.
        for seed in [0x6750a4, 0xff0000, 0x00aa44, 0x0061a4, 0xffcc00, 0x777777] {
            for v in all_variants() {
                for dark in [false, true] {
                    let c = generate(seed, v, dark, 0.0, 1.0, 1.0);
                    for (name, bg) in [
                        ("surface", c.surface),
                        ("surface_container", c.surface_container),
                        ("surface_container_high", c.surface_container_high),
                        ("surface_container_highest", c.surface_container_highest),
                    ] {
                        let ratio = c.on_surface.contrast(bg);
                        assert!(ratio >= 4.5, "{seed:06x} {v:?} dark={dark} on_surface/{name} = {ratio:.2}");
                    }
                    assert!(c.on_primary.contrast(c.primary) >= 4.5, "{seed:06x} {v:?} {dark} on_primary");
                    assert!(
                        c.on_primary_container.contrast(c.primary_container) >= 4.5,
                        "{seed:06x} {v:?} {dark} on_primary_container"
                    );
                    assert!(c.on_secondary_container.contrast(c.secondary_container) >= 4.5);
                }
            }
        }
    }

    #[test]
    fn high_contrast_increases_contrast() {
        let std = generate(0x6750a4, SchemeVariant::TonalSpot, false, 0.0, 1.0, 1.0);
        let high = generate(0x6750a4, SchemeVariant::TonalSpot, false, 1.0, 1.0, 1.0);
        assert!(high.on_surface_variant.contrast(high.surface) > std.on_surface_variant.contrast(std.surface));
        assert!(high.outline.contrast(high.surface) > std.outline.contrast(std.surface));
    }

    #[test]
    fn dark_surfaces_are_dark_and_ordered() {
        let c = generate(0x0061a4, SchemeVariant::TonalSpot, true, 0.0, 1.0, 1.0);
        let l = |c: Rgb| c.luminance();
        assert!(l(c.surface_container_lowest) <= l(c.surface_container_low));
        assert!(l(c.surface_container_low) <= l(c.surface_container));
        assert!(l(c.surface_container) <= l(c.surface_container_high));
        assert!(l(c.surface_container_high) <= l(c.surface_container_highest));
        assert!(l(c.surface) < 0.05);
    }

    fn saturation(c: Rgb) -> f64 {
        let (r, g, b) = (c.0 as f64, c.1 as f64, c.2 as f64);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        if max == 0.0 { 0.0 } else { (max - min) / max }
    }

    #[test]
    fn accent_and_surface_controls_are_independent() {
        let base = generate(0xd04040, SchemeVariant::TonalSpot, false, 0.0, 1.0, 1.0);
        let grey_accent = generate(0xd04040, SchemeVariant::TonalSpot, false, 0.0, 0.0, 1.0);
        let vivid = generate(0xd04040, SchemeVariant::TonalSpot, false, 0.0, 2.0, 1.0);
        assert!(saturation(grey_accent.primary) < 0.05, "{:?}", grey_accent.primary);
        assert!(saturation(vivid.primary) > saturation(base.primary));
        // Surfaces unchanged by accent intensity.
        assert_eq!(base.surface, vivid.surface);
        let neutral_surfaces = generate(0xd04040, SchemeVariant::TonalSpot, false, 0.0, 1.0, 0.0);
        assert!(saturation(neutral_surfaces.surface_container) < saturation(base.surface_container) + 1e-9);
        assert_eq!(neutral_surfaces.primary, base.primary);
    }

    #[test]
    fn different_seeds_give_different_primaries() {
        let a = generate(0xff0000, SchemeVariant::TonalSpot, false, 0.0, 1.0, 1.0);
        let b = generate(0x0000ff, SchemeVariant::TonalSpot, false, 0.0, 1.0, 1.0);
        assert_ne!(a.primary, b.primary);
        assert!(a.primary.0 > a.primary.2, "red seed → reddish primary {:?}", a.primary);
        assert!(b.primary.2 > b.primary.0, "blue seed → bluish primary {:?}", b.primary);
    }
}
