//! Pure helpers turning `lock.toml` into what the lock screen shows.

use std::path::PathBuf;

use m3_core::config::{LockBackground, LockClockStyle, LockConfig, ThemeConfig};

/// The background image for an output, or `None` for the colour gradient.
/// `exists` is injected for testing (normally `Path::is_file`).
pub fn background_path(
    lock: &LockConfig,
    theme: &ThemeConfig,
    connector: Option<&str>,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    let wallpaper = || {
        connector
            .and_then(|c| theme.wallpaper.outputs.get(c))
            .filter(|p| exists(p))
            .or_else(|| Some(&theme.wallpaper.path).filter(|p| !p.as_os_str().is_empty() && exists(p)))
            .cloned()
    };
    match lock.background {
        LockBackground::Color => None,
        LockBackground::Image if exists(&lock.image) => Some(lock.image.clone()),
        // A missing image falls back to the wallpaper rather than failing.
        LockBackground::Image | LockBackground::Wallpaper => wallpaper(),
    }
}

/// CSS for the user's choices, appended after `lock.css`.
pub fn dynamic_css(lock: &LockConfig) -> String {
    let mut s = String::new();
    s.push_str(&format!(".m3-lock-scrim {{ background-color: rgba(0, 0, 0, {:.2}); }}\n", lock.dim.clamp(0.0, 0.9)));
    if lock.blur > 0 {
        // Blur fades in from the edges; scale up a little so the screen
        // edges stay covered.
        let scale = 1.0 + f64::from(lock.blur.min(64)) * 6.0 / 1200.0;
        s.push_str(&format!(
            ".m3-lock-bg {{ filter: blur({}px); transform: scale({scale:.3}); }}\n",
            lock.blur.min(64)
        ));
    }
    let px = lock.clock_size.pixels();
    let (weight, width, rond, tracking) = match lock.clock_style {
        LockClockStyle::Expressive => (700, 112, 100, -(px as i32) / 40),
        LockClockStyle::Light => (200, 125, 0, 0),
        LockClockStyle::Classic => (400, 100, 0, -1),
    };
    s.push_str(&format!(
        ".m3-lock-time {{ font-size: {px}px; font-weight: {weight}; letter-spacing: {tracking}px; \
         font-variation-settings: \"wght\" {weight}, \"wdth\" {width}, \"ROND\" {rond}; }}\n"
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3_core::config::LockClockSize;

    fn theme_with(path: &str, out: Option<(&str, &str)>) -> ThemeConfig {
        let mut t = ThemeConfig::default();
        t.wallpaper.path = path.into();
        if let Some((o, p)) = out {
            t.wallpaper.outputs.insert(o.into(), p.into());
        }
        t
    }

    #[test]
    fn background_choice() {
        let all = |_: &std::path::Path| true;
        let none = |_: &std::path::Path| false;
        let theme = theme_with("/w/main.jpg", Some(("HDMI-A-1", "/w/tv.jpg")));
        let lock = LockConfig::default();
        assert_eq!(background_path(&lock, &theme, Some("eDP-1"), all), Some("/w/main.jpg".into()));
        assert_eq!(background_path(&lock, &theme, Some("HDMI-A-1"), all), Some("/w/tv.jpg".into()));
        assert_eq!(background_path(&lock, &theme, None, none), None);
        let img = LockConfig { background: LockBackground::Image, image: "/p/lock.png".into(), ..Default::default() };
        assert_eq!(background_path(&img, &theme, None, all), Some("/p/lock.png".into()));
        // Missing image: fall back to the wallpaper.
        let only_wp = |p: &std::path::Path| p == std::path::Path::new("/w/main.jpg");
        assert_eq!(background_path(&img, &theme, None, only_wp), Some("/w/main.jpg".into()));
        let color = LockConfig { background: LockBackground::Color, ..Default::default() };
        assert_eq!(background_path(&color, &theme, None, all), None);
        // Empty wallpaper path means the built-in look.
        assert_eq!(background_path(&lock, &theme_with("", None), None, all), None);
    }

    #[test]
    fn css_reflects_settings() {
        let c = LockConfig {
            blur: 24,
            dim: 0.5,
            clock_size: LockClockSize::Small,
            clock_style: LockClockStyle::Light,
            ..Default::default()
        };
        let css = dynamic_css(&c);
        assert!(css.contains("blur(24px)"), "{css}");
        assert!(css.contains("rgba(0, 0, 0, 0.50)"));
        assert!(css.contains("font-size: 72px") && css.contains("\"wght\" 200"));
        assert!(!dynamic_css(&LockConfig::default()).contains("blur("), "no blur by default");
    }
}
