//! Night light: colour temperature → display gamma ramps.

/// Relative RGB gains of a black body at `kelvin`, normalised so 6500 K is
/// neutral (1, 1, 1). Tanner Helland's approximation of the Planckian locus,
/// good from 1000 K to 40000 K.
pub fn white_point(kelvin: f64) -> (f64, f64, f64) {
    let raw = |k: f64| {
        let t = k.clamp(1000.0, 40000.0) / 100.0;
        let r = if t <= 66.0 { 255.0 } else { 329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2) };
        let g = if t <= 66.0 {
            99.470_802_586_1 * t.ln() - 161.119_568_166_1
        } else {
            288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
        };
        let b = if t >= 66.0 {
            255.0
        } else if t <= 19.0 {
            0.0
        } else {
            138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
        };
        (r.clamp(0.0, 255.0), g.clamp(0.0, 255.0), b.clamp(0.0, 255.0))
    };
    let (r, g, b) = raw(kelvin);
    let (wr, wg, wb) = raw(6500.0);
    ((r / wr).min(1.0), (g / wg).min(1.0), (b / wb).min(1.0))
}

/// A linear gamma ramp of `len` entries scaled by `gain`.
pub fn ramp(len: usize, gain: f64) -> Vec<u16> {
    if len < 2 {
        return vec![(gain.clamp(0.0, 1.0) * 65535.0) as u16; len];
    }
    (0..len).map(|i| ((i as f64 / (len - 1) as f64) * gain * 65535.0).round().clamp(0.0, 65535.0) as u16).collect()
}

/// The next temperature on the way from `current` to `target` (smooth
/// ~1 s transition at 25 steps per second).
pub fn step_towards(current: f64, target: f64) -> f64 {
    let d = target - current;
    if d.abs() <= 25.0 { target } else { current + d.signum() * (d.abs() * 0.15).max(25.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_and_warm() {
        let (r, g, b) = white_point(6500.0);
        assert!((r - 1.0).abs() < 1e-9 && (g - 1.0).abs() < 1e-9 && (b - 1.0).abs() < 1e-9);
        let (r, g, b) = white_point(3000.0);
        assert!(r > g && g > b && b < 0.7, "{r} {g} {b}");
    }

    #[test]
    fn ramps_are_monotonic_and_scaled() {
        let full = ramp(256, 1.0);
        assert_eq!((full[0], full[255]), (0, 65535));
        assert!(full.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(ramp(1024, 0.5)[1023], 32768);
    }

    #[test]
    fn transition_converges() {
        let mut k = 6500.0;
        let mut steps = 0;
        while k != 4000.0 {
            k = step_towards(k, 4000.0);
            steps += 1;
            assert!(steps < 100);
        }
        assert!(steps > 5, "smooth, not instant: {steps}");
    }
}
