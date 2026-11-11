//! Seed-colour extraction from wallpapers.
//!
//! Image decoding is left to the caller (the shell decodes with GdkPixbuf);
//! this module takes RGBA pixels, downsamples them, and runs Material's
//! Celebi quantizer + scorer. Results are cached per (path, size, mtime) in
//! `~/.cache/m3/wallpaper-seeds.json` so changing themes is instant.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use material_colors::{color::Rgb as McRgb, image::extract_color};
use serde::{Deserialize, Serialize};

/// Maximum number of pixels fed to the quantizer.
const SAMPLE_BUDGET: usize = 128 * 128;

/// Extract a seed colour from tightly packed RGBA8 pixels (`width * height * 4`
/// bytes). Fully transparent pixels are ignored. Returns `None` for empty or
/// malformed input.
pub fn seed_from_rgba(rgba: &[u8], width: usize, height: usize) -> Option<u32> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return None;
    }
    let step = ((width * height) as f64 / SAMPLE_BUDGET as f64).sqrt().ceil().max(1.0) as usize;
    let mut pixels = Vec::with_capacity(SAMPLE_BUDGET + width);
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            let i = (y * width + x) * 4;
            if rgba[i + 3] < 128 {
                continue;
            }
            pixels.push(McRgb::new(rgba[i], rgba[i + 1], rgba[i + 2]));
        }
    }
    if pixels.is_empty() {
        return None;
    }
    let c = extract_color(&pixels);
    Some((u32::from(c.red) << 16) | (u32::from(c.green) << 8) | u32::from(c.blue))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    entries: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    size: u64,
    mtime: i64,
    seed: u32,
}

fn cache_path() -> Option<PathBuf> {
    m3_core::paths::cache_dir().map(|d| d.join("wallpaper-seeds.json"))
}

fn fingerprint(path: &Path) -> Option<(u64, i64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.mtime()))
}

/// A cached seed for `path`, if the file hasn't changed since it was computed.
pub fn cached_seed(path: &Path) -> Option<u32> {
    cached_seed_in(&cache_path()?, path)
}

/// Remember the seed computed for `path`.
pub fn store_seed(path: &Path, seed: u32) {
    if let Some(cache) = cache_path() {
        store_seed_in(&cache, path, seed);
    }
}

fn cached_seed_in(cache: &Path, path: &Path) -> Option<u32> {
    let (size, mtime) = fingerprint(path)?;
    let text = std::fs::read_to_string(cache).ok()?;
    let file: CacheFile = serde_json::from_str(&text).ok()?;
    let e = file.entries.get(&path.to_string_lossy().into_owned())?;
    (e.size == size && e.mtime == mtime).then_some(e.seed)
}

fn store_seed_in(cache: &Path, path: &Path, seed: u32) {
    let Some((size, mtime)) = fingerprint(path) else { return };
    let mut file: CacheFile =
        std::fs::read_to_string(cache).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    file.entries.insert(path.to_string_lossy().into_owned(), CacheEntry { size, mtime, seed });
    // Bound the cache.
    while file.entries.len() > 256 {
        let Some(k) = file.entries.keys().next().cloned() else { break };
        file.entries.remove(&k);
    }
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(&file) {
        if let Err(e) = m3_core::config::atomic_write(cache, text.as_bytes()) {
            tracing::debug!("cannot write wallpaper seed cache: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect()
    }

    #[test]
    fn dominant_color_wins() {
        // Mostly blue with a red stripe.
        let (w, h) = (300, 200);
        let mut px = solid(w, h, [20, 60, 200]);
        for y in 0..20 {
            for x in 0..w {
                let i = (y * w + x) * 4;
                px[i..i + 3].copy_from_slice(&[220, 30, 30]);
            }
        }
        let seed = seed_from_rgba(&px, w, h).unwrap();
        let (r, b) = ((seed >> 16) & 0xff, seed & 0xff);
        assert!(b > r, "expected a blue seed, got {seed:06x}");
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(seed_from_rgba(&[], 0, 0), None);
        assert_eq!(seed_from_rgba(&[0; 8], 4, 4), None);
        let transparent = vec![0u8; 16 * 16 * 4];
        assert_eq!(seed_from_rgba(&transparent, 16, 16), None);
    }

    #[test]
    fn large_images_are_sampled() {
        let px = solid(2000, 1500, [10, 140, 60]);
        let t = std::time::Instant::now();
        let seed = seed_from_rgba(&px, 2000, 1500).unwrap();
        assert!(t.elapsed().as_secs() < 2);
        assert!((seed >> 8) & 0xff > (seed >> 16) & 0xff, "green {seed:06x}");
    }

    #[test]
    fn cache_invalidates_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("c.json");
        let img = dir.path().join("w.png");
        std::fs::write(&img, b"one").unwrap();
        assert_eq!(cached_seed_in(&cache, &img), None);
        store_seed_in(&cache, &img, 0x123456);
        assert_eq!(cached_seed_in(&cache, &img), Some(0x123456));
        std::fs::write(&img, b"changed!").unwrap();
        assert_eq!(cached_seed_in(&cache, &img), None);
    }
}
