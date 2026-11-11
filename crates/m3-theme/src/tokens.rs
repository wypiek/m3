//! Non-colour design tokens: shape, typography, motion, elevation, spacing.
//!
//! Values follow the Material 3 **Expressive** token tables (2025): the
//! extended shape scale, the emphasized type scale and the spring-based
//! motion scheme, scaled by the user's corner and text preferences.

use serde::{Deserialize, Serialize};

/// Corner radii in logical pixels (M3 Expressive shape scale).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub extra_small: f64,
    pub small: f64,
    pub medium: f64,
    pub large: f64,
    pub large_increased: f64,
    pub extra_large: f64,
    pub extra_large_increased: f64,
    pub extra_extra_large: f64,
    /// Pill shape.
    pub full: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeStyle {
    pub size: f64,
    pub line_height: f64,
    pub weight: u16,
    pub tracking: f64,
}

/// The baseline type scale plus the Expressive *emphasized* variants
/// (heavier weights for hierarchy and personality).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Typography {
    pub display_large: TypeStyle,
    pub display_medium: TypeStyle,
    pub display_small: TypeStyle,
    pub headline_large: TypeStyle,
    pub headline_medium: TypeStyle,
    pub headline_small: TypeStyle,
    pub title_large: TypeStyle,
    pub title_medium: TypeStyle,
    pub title_small: TypeStyle,
    pub body_large: TypeStyle,
    pub body_medium: TypeStyle,
    pub body_small: TypeStyle,
    pub label_large: TypeStyle,
    pub label_medium: TypeStyle,
    pub label_small: TypeStyle,
}

impl Typography {
    /// `(css-name, style)` pairs, e.g. `("headline-large", …)`.
    pub fn styles(&self) -> [(&'static str, &TypeStyle); 15] {
        [
            ("display-large", &self.display_large),
            ("display-medium", &self.display_medium),
            ("display-small", &self.display_small),
            ("headline-large", &self.headline_large),
            ("headline-medium", &self.headline_medium),
            ("headline-small", &self.headline_small),
            ("title-large", &self.title_large),
            ("title-medium", &self.title_medium),
            ("title-small", &self.title_small),
            ("body-large", &self.body_large),
            ("body-medium", &self.body_medium),
            ("body-small", &self.body_small),
            ("label-large", &self.label_large),
            ("label-medium", &self.label_medium),
            ("label-small", &self.label_small),
        ]
    }
}

/// One spring from the Expressive motion scheme, as a CSS duration + a
/// cubic-bézier approximation (the published M3 Expressive values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    pub duration_ms: u32,
    pub easing: String,
}

/// M3 Expressive motion: *spatial* springs (position, size, shape — they
/// overshoot slightly) and *effects* springs (colour, opacity — no bounce),
/// each in fast / default / slow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Motion {
    pub fast_spatial: Spring,
    pub default_spatial: Spring,
    pub slow_spatial: Spring,
    pub fast_effects: Spring,
    pub default_effects: Spring,
    pub slow_effects: Spring,
}

/// State-layer opacities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct States {
    pub hover: f64,
    pub focus: f64,
    pub pressed: f64,
    pub dragged: f64,
    pub disabled_content: f64,
    pub disabled_container: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub shape: Shape,
    pub typography: Typography,
    /// Emphasized variants of the same scale.
    pub emphasized: Typography,
    pub motion: Motion,
    pub states: States,
    /// Base spacing unit (4dp grid).
    pub spacing: f64,
    /// Minimum interactive target size.
    pub touch_target: f64,
}

fn ts(size: f64, line: f64, weight: u16, tracking: f64, k: f64) -> TypeStyle {
    TypeStyle {
        size: (size * k * 100.0).round() / 100.0,
        line_height: (line * k * 100.0).round() / 100.0,
        weight,
        tracking,
    }
}

fn spring(ms: u32, easing: &str, reduced: bool) -> Spring {
    if reduced {
        Spring { duration_ms: 0, easing: "linear".into() }
    } else {
        Spring { duration_ms: ms, easing: easing.into() }
    }
}

impl Tokens {
    pub fn new(corner_scale: f64, text_scale: f64, reduced_motion: bool) -> Self {
        let c = corner_scale.clamp(0.0, 2.0);
        let k = text_scale.clamp(0.5, 3.0);
        let shape = Shape {
            extra_small: 4.0 * c,
            small: 8.0 * c,
            medium: 12.0 * c,
            large: 16.0 * c,
            large_increased: 20.0 * c,
            extra_large: 28.0 * c,
            extra_large_increased: 32.0 * c,
            extra_extra_large: 48.0 * c,
            full: if c == 0.0 { 0.0 } else { 9999.0 },
        };
        let typography = Typography {
            display_large: ts(57.0, 64.0, 400, -0.25, k),
            display_medium: ts(45.0, 52.0, 400, 0.0, k),
            display_small: ts(36.0, 44.0, 400, 0.0, k),
            headline_large: ts(32.0, 40.0, 400, 0.0, k),
            headline_medium: ts(28.0, 36.0, 400, 0.0, k),
            headline_small: ts(24.0, 32.0, 400, 0.0, k),
            title_large: ts(22.0, 28.0, 400, 0.0, k),
            title_medium: ts(16.0, 24.0, 500, 0.15, k),
            title_small: ts(14.0, 20.0, 500, 0.1, k),
            body_large: ts(16.0, 24.0, 400, 0.5, k),
            body_medium: ts(14.0, 20.0, 400, 0.25, k),
            body_small: ts(12.0, 16.0, 400, 0.4, k),
            label_large: ts(14.0, 20.0, 500, 0.1, k),
            label_medium: ts(12.0, 16.0, 500, 0.5, k),
            label_small: ts(11.0, 16.0, 500, 0.5, k),
        };
        let emphasized = Typography {
            display_large: ts(57.0, 64.0, 650, -0.5, k),
            display_medium: ts(45.0, 52.0, 650, -0.25, k),
            display_small: ts(36.0, 44.0, 650, -0.25, k),
            headline_large: ts(32.0, 40.0, 620, 0.0, k),
            headline_medium: ts(28.0, 36.0, 620, 0.0, k),
            headline_small: ts(24.0, 32.0, 620, 0.0, k),
            title_large: ts(22.0, 28.0, 600, 0.0, k),
            title_medium: ts(16.0, 24.0, 650, 0.15, k),
            title_small: ts(14.0, 20.0, 650, 0.1, k),
            body_large: ts(16.0, 24.0, 500, 0.5, k),
            body_medium: ts(14.0, 20.0, 500, 0.25, k),
            body_small: ts(12.0, 16.0, 500, 0.4, k),
            label_large: ts(14.0, 20.0, 600, 0.1, k),
            label_medium: ts(12.0, 16.0, 600, 0.5, k),
            label_small: ts(11.0, 16.0, 600, 0.5, k),
        };
        let r = reduced_motion;
        let motion = Motion {
            fast_spatial: spring(350, "cubic-bezier(0.42, 1.67, 0.21, 0.90)", r),
            default_spatial: spring(500, "cubic-bezier(0.38, 1.21, 0.22, 1.00)", r),
            slow_spatial: spring(650, "cubic-bezier(0.39, 1.29, 0.35, 0.98)", r),
            fast_effects: spring(150, "cubic-bezier(0.31, 0.94, 0.34, 1.00)", r),
            default_effects: spring(200, "cubic-bezier(0.34, 0.80, 0.34, 1.00)", r),
            slow_effects: spring(300, "cubic-bezier(0.34, 0.88, 0.34, 1.00)", r),
        };
        let states = States {
            hover: 0.08,
            focus: 0.10,
            pressed: 0.10,
            dragged: 0.16,
            disabled_content: 0.38,
            disabled_container: 0.12,
        };
        Self { shape, typography, emphasized, motion, states, spacing: 4.0, touch_target: 48.0 }
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::new(1.0, 1.0, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling() {
        let t = Tokens::new(0.5, 1.5, false);
        assert_eq!(t.shape.large, 8.0);
        assert_eq!(t.typography.body_medium.size, 21.0);
        assert_eq!(Tokens::new(0.0, 1.0, false).shape.full, 0.0, "square corners everywhere");
    }

    #[test]
    fn emphasized_is_heavier() {
        let t = Tokens::default();
        for ((_, base), (_, emph)) in t.typography.styles().iter().zip(t.emphasized.styles().iter()) {
            assert!(emph.weight > base.weight);
            assert_eq!(emph.size, base.size);
        }
    }

    #[test]
    fn reduced_motion_disables_animation() {
        let t = Tokens::new(1.0, 1.0, true);
        assert_eq!((t.motion.fast_spatial.duration_ms, t.motion.slow_effects.duration_ms), (0, 0));
    }
}
