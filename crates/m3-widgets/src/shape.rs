//! The Material 3 Expressive shape library as polar functions.
//!
//! Every shape is a radius function `r(θ)` around the centre, so any two
//! shapes morph smoothly by interpolating their radii at the same angles —
//! the same idea as M3's shape morphing, without needing vertex matching.

use std::f64::consts::PI;

/// Shapes from the M3 Expressive shape set (the ones that are star-shaped
/// around their centre, which is what a polar morph can represent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Circle,
    /// Superellipse — a soft rounded square.
    Square,
    /// Shallow scallops: `Cookie(4)`, `Cookie(6)`, `Cookie(7)`, `Cookie(9)`, `Cookie(12)`.
    Cookie(u32),
    /// Round lobes with deep, narrow notches: `Clover(4)`, `Clover(8)`.
    Clover(u32),
    /// Narrow soft points with wide valleys: `Sunny(8)`.
    Sunny(u32),
    /// Petals: `Flower(8)`.
    Flower(u32),
    /// Many sharp-ish points: `Burst(12)`.
    Burst(u32),
    /// Rounded pentagon-like shape.
    Pentagon,
}

/// Number of samples used for drawing and morphing.
pub const SAMPLES: usize = 192;

impl Shape {
    /// Unnormalised radius at angle `theta` (radians), roughly in `0.6..=1.2`.
    pub fn radius(self, theta: f64) -> f64 {
        // `f` is 0 at a lobe's centre and 1 at a notch.
        let notch = |n: u32| (1.0 - (f64::from(n.max(1)) * theta).cos()) / 2.0;
        match self {
            Shape::Circle => 1.0,
            Shape::Square => {
                let (c, s) = (theta.cos().abs(), theta.sin().abs());
                (c.powi(5) + s.powi(5)).powf(-1.0 / 5.0)
            }
            Shape::Cookie(n) => {
                let depth = match n {
                    0..=4 => 0.15,
                    5..=6 => 0.12,
                    7 => 0.105,
                    8..=9 => 0.09,
                    _ => 0.07,
                };
                1.0 - depth * notch(n)
            }
            Shape::Clover(n) => {
                let depth = if n <= 4 { 0.34 } else { 0.2 };
                1.0 - depth * notch(n).powf(2.6)
            }
            Shape::Sunny(n) => 1.0 - 0.16 * notch(n).powf(0.75),
            Shape::Flower(n) => 1.0 - 0.2 * notch(n).powf(1.6),
            Shape::Burst(n) => 1.0 - 0.22 * notch(n).powf(0.55),
            Shape::Pentagon => {
                // A regular pentagon's polar radius, softened towards a circle.
                let k = 2.0 * PI / 5.0;
                let a = (theta + PI / 2.0).rem_euclid(k) - k / 2.0;
                let poly = (PI / 5.0).cos() / a.cos();
                0.35 + 0.65 * poly
            }
        }
    }

    /// Sampled radii, normalised so the largest is 1.
    pub fn radii(self) -> [f64; SAMPLES] {
        let mut r = [0.0; SAMPLES];
        for (i, v) in r.iter_mut().enumerate() {
            *v = self.radius(angle(i));
        }
        let max = r.iter().copied().fold(f64::MIN, f64::max);
        if max > 0.0 {
            for v in &mut r {
                *v /= max;
            }
        }
        r
    }
}

/// Angle of sample `i` (starting at the top, clockwise in screen space).
pub fn angle(i: usize) -> f64 {
    -PI / 2.0 + 2.0 * PI * (i as f64) / (SAMPLES as f64)
}

/// Interpolate two sampled shapes (`t` may overshoot slightly for springs).
pub fn morph(a: &[f64; SAMPLES], b: &[f64; SAMPLES], t: f64) -> [f64; SAMPLES] {
    let mut r = [0.0; SAMPLES];
    for i in 0..SAMPLES {
        r[i] = (a[i] + (b[i] - a[i]) * t).max(0.0);
    }
    r
}

/// Outline points for radii `r`, centred in a `w`×`h` box, rotated by
/// `rotation` degrees. The shape fills the box's shorter side.
pub fn outline(r: &[f64; SAMPLES], w: f64, h: f64, rotation: f64) -> Vec<(f64, f64)> {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let scale = w.min(h) / 2.0;
    let rot = rotation.to_radians();
    (0..SAMPLES)
        .map(|i| {
            let a = angle(i) + rot;
            (cx + a.cos() * r[i] * scale, cy + a.sin() * r[i] * scale)
        })
        .collect()
}

/// A critically-damped-ish spring from 0 to 1 (Expressive spatial spring:
/// damping ratio `zeta`, stiffness `k`), evaluated at `t` seconds. Returns
/// the position and whether it has settled.
pub fn spring(t: f64, zeta: f64, k: f64) -> (f64, bool) {
    if t <= 0.0 {
        return (0.0, false);
    }
    let w = k.sqrt();
    let x = if zeta < 1.0 {
        let wd = w * (1.0 - zeta * zeta).sqrt();
        1.0 - (-zeta * w * t).exp() * ((wd * t).cos() + (zeta * w / wd) * (wd * t).sin())
    } else {
        1.0 - (-w * t).exp() * (1.0 + w * t)
    };
    let settled = (-zeta * w * t).exp() < 0.002;
    (if settled { 1.0 } else { x }, settled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Shape; 10] = [
        Shape::Circle,
        Shape::Square,
        Shape::Cookie(4),
        Shape::Cookie(9),
        Shape::Clover(4),
        Shape::Clover(8),
        Shape::Sunny(8),
        Shape::Flower(8),
        Shape::Burst(12),
        Shape::Pentagon,
    ];

    #[test]
    fn shapes_are_normalised_and_positive() {
        for s in ALL {
            let r = s.radii();
            let max = r.iter().copied().fold(f64::MIN, f64::max);
            let min = r.iter().copied().fold(f64::MAX, f64::min);
            assert!((max - 1.0).abs() < 1e-9, "{s:?}");
            assert!(min > 0.4, "{s:?} min {min}");
        }
    }

    #[test]
    fn lobes_and_notches() {
        // Cookie(4): lobes on the axes are further out than the diagonals.
        let c = Shape::Cookie(4);
        assert!(c.radius(0.0) > c.radius(PI / 4.0));
        // Clover notches are deeper than cookie scallops.
        let depth = |s: Shape| s.radius(0.0) - s.radius(PI / 4.0);
        assert!(depth(Shape::Clover(4)) > depth(Shape::Cookie(4)));
    }

    #[test]
    fn morph_endpoints() {
        let a = Shape::Circle.radii();
        let b = Shape::Clover(4).radii();
        assert_eq!(morph(&a, &b, 0.0), a);
        assert_eq!(morph(&a, &b, 1.0), b);
    }

    #[test]
    fn spring_settles_with_overshoot() {
        let (x0, _) = spring(0.0, 0.8, 380.0);
        assert_eq!(x0, 0.0);
        let peak = (1..200).map(|i| spring(f64::from(i) / 200.0, 0.6, 800.0).0).fold(0.0, f64::max);
        assert!(peak > 1.0, "expressive springs overshoot");
        let (x, done) = spring(2.0, 0.8, 380.0);
        assert!(done && (x - 1.0).abs() < 1e-9);
    }

    #[test]
    fn outline_fits_box() {
        let pts = outline(&Shape::Square.radii(), 100.0, 60.0, 15.0);
        for (x, y) in pts {
            assert!((19.9..=80.1).contains(&x) && (-0.1..=60.1).contains(&y));
        }
    }
}
