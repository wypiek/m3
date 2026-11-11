//! Terminal colours derived from the m3 theme.
//!
//! The background, foreground, cursor and selection use Material roles
//! directly. The 16 ANSI colours keep their conventional hues (so programs
//! still look right) but are *harmonised* towards the theme's primary hue —
//! rotated by up to 15°, as Material's colour harmonisation does — and their
//! lightness is chosen so every colour stays readable on the background in
//! both light and dark themes.

use m3_theme::{Colors, Rgb};

/// Minimum contrast of the normal ANSI colours against the background.
pub const NORMAL_CONTRAST: f64 = 4.5;
/// Minimum contrast of the bright ANSI colours against the background.
pub const BRIGHT_CONTRAST: f64 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub cursor_foreground: Rgb,
    pub selection: Rgb,
    pub selection_foreground: Rgb,
    pub ansi: [Rgb; 16],
}

/// (hue in degrees, saturation) of red, green, yellow, blue, magenta, cyan.
const HUES: [(f64, f64); 6] = [(2.0, 0.70), (128.0, 0.50), (45.0, 0.75), (218.0, 0.70), (300.0, 0.50), (186.0, 0.60)];

pub fn rgb_to_hsl(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (f64::from(c.0) / 255.0, f64::from(c.1) / 255.0, f64::from(c.2) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f64| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb(to(r), to(g), to(b))
}

/// Rotate `hue` towards `towards` by half the difference, at most 15°.
pub fn harmonize(hue: f64, towards: f64) -> f64 {
    let diff = (towards - hue + 540.0).rem_euclid(360.0) - 180.0;
    (hue + diff.signum() * (diff.abs() * 0.5).min(15.0)).rem_euclid(360.0)
}

/// The colour of `hue`/`sat` with the lightness closest to `preferred` that
/// reaches `min_contrast` against `bg` (or the most contrasting one).
fn readable(hue: f64, sat: f64, bg: Rgb, min_contrast: f64, preferred: f64) -> Rgb {
    let mut best: Option<(f64, Rgb)> = None;
    let mut fallback = (0.0, hsl_to_rgb(hue, sat, preferred));
    for i in 0..=160 {
        let l = 0.10 + 0.85 * f64::from(i) / 160.0;
        let c = hsl_to_rgb(hue, sat, l);
        let contrast = c.contrast(bg);
        if contrast > fallback.0 {
            fallback = (contrast, c);
        }
        if contrast >= min_contrast {
            let dist = (l - preferred).abs();
            if best.as_ref().is_none_or(|(d, _)| dist < *d) {
                best = Some((dist, c));
            }
        }
    }
    best.map(|b| b.1).unwrap_or(fallback.1)
}

/// Build the terminal palette for a resolved theme.
pub fn palette(c: &Colors, dark: bool) -> Palette {
    let bg = c.surface;
    let (ph, ps, _) = rgb_to_hsl(c.primary);
    let tint = ps > 0.08;
    let hue = |h: f64| if tint { harmonize(h, ph) } else { h };
    let (pref_n, pref_b) = if dark { (0.64, 0.76) } else { (0.38, 0.50) };
    let mut ansi = [Rgb(0, 0, 0); 16];
    for (i, (h, s)) in HUES.iter().enumerate() {
        ansi[i + 1] = readable(hue(*h), *s, bg, NORMAL_CONTRAST, pref_n);
        ansi[i + 9] = readable(hue(*h), (*s + 0.1).min(1.0), bg, BRIGHT_CONTRAST, pref_b);
    }
    if dark {
        ansi[0] = c.surface_container_highest;
        ansi[8] = c.outline;
        ansi[7] = c.on_surface_variant;
        ansi[15] = c.on_surface;
    } else {
        ansi[0] = c.on_surface;
        ansi[8] = c.on_surface_variant;
        ansi[7] = c.outline;
        ansi[15] = c.outline_variant;
    }
    Palette {
        foreground: c.on_surface,
        background: bg,
        cursor: c.primary,
        cursor_foreground: c.on_primary,
        selection: c.primary_container,
        selection_foreground: c.on_primary_container,
        ansi,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3_core::config::SchemeVariant;

    fn colors(seed: u32, dark: bool) -> Colors {
        m3_theme::scheme::generate(seed, SchemeVariant::TonalSpot, dark, 0.0, 1.0, 1.0)
    }

    #[test]
    fn hsl_round_trip() {
        for c in [Rgb(255, 0, 0), Rgb(18, 140, 200), Rgb(250, 250, 250), Rgb(3, 4, 5), Rgb(120, 60, 200)] {
            let (h, s, l) = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for (a, b) in [(c.0, back.0), (c.1, back.1), (c.2, back.2)] {
                assert!(a.abs_diff(b) <= 1, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn harmonize_moves_at_most_15_degrees_towards_primary() {
        assert_eq!(harmonize(0.0, 20.0), 10.0);
        assert_eq!(harmonize(0.0, 100.0), 15.0);
        assert_eq!(harmonize(10.0, 350.0), 0.0);
        assert_eq!(harmonize(350.0, 30.0), 5.0, "wraps around 360");
        assert_eq!(harmonize(200.0, 200.0), 200.0);
    }

    #[test]
    fn ansi_colours_are_readable_in_light_and_dark() {
        for seed in [0x6750a4, 0x006a6a, 0xa73a2b, 0x7c5800, 0x5b5f71] {
            for dark in [false, true] {
                let p = palette(&colors(seed, dark), dark);
                assert!(p.foreground.contrast(p.background) >= 7.0);
                for i in (1..=6).chain(9..=14) {
                    let min = if i < 8 { NORMAL_CONTRAST } else { BRIGHT_CONTRAST };
                    let c = p.ansi[i].contrast(p.background);
                    assert!(c >= min - 0.05, "seed {seed:06x} dark {dark} colour {i}: {c:.2}");
                }
                // The text colours used most (white/bright white in dark,
                // black in light) are clearly readable.
                let text = if dark { p.ansi[15] } else { p.ansi[0] };
                assert!(text.contrast(p.background) >= 7.0);
            }
        }
    }

    #[test]
    fn hues_stay_recognisable() {
        let p = palette(&colors(0x0061a4, true), true);
        let (red, _, _) = rgb_to_hsl(p.ansi[1]);
        let (green, _, _) = rgb_to_hsl(p.ansi[2]);
        let (blue, _, _) = rgb_to_hsl(p.ansi[4]);
        let near = |h: f64, t: f64| ((h - t + 540.0).rem_euclid(360.0) - 180.0).abs() <= 20.0;
        assert!(near(red, 2.0), "red {red}");
        assert!(near(green, 128.0), "green {green}");
        assert!(near(blue, 218.0), "blue {blue}");
    }
}
