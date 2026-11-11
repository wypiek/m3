//! GTK 4 CSS for m3 applications.
//!
//! [`gtk_css`] returns the custom-property block for the current theme
//! followed by the shared component stylesheet (`components.css`). Every m3
//! GTK program loads exactly this, so styling is defined once.

use std::fmt::Write;

use crate::{Theme, tokens::TypeStyle};

/// The component stylesheet. It only references `var(--m3-…)` properties.
pub const COMPONENTS: &str = include_str!("components.css");

/// Custom properties for the theme (`:root { --m3-primary: …; … }`).
pub fn variables(theme: &Theme, font_family: &str, mono_family: &str, transparency: bool) -> String {
    let mut s = String::from(":root {\n");
    for (name, c) in theme.colors.roles() {
        let _ = writeln!(s, "  --m3-{name}: {};", c.hex());
    }
    let c = &theme.colors;
    let st = &theme.tokens.states;
    // Precomputed state layers so stylesheets don't need color-mix() for the
    // common cases.
    let layers = [
        ("on-surface", c.on_surface),
        ("on-surface-variant", c.on_surface_variant),
        ("primary", c.primary),
        ("on-primary", c.on_primary),
        ("on-primary-container", c.on_primary_container),
        ("on-secondary-container", c.on_secondary_container),
        ("on-tertiary-container", c.on_tertiary_container),
        ("error", c.error),
    ];
    for (name, col) in layers {
        let _ = writeln!(s, "  --m3-{name}-hover: {};", col.rgba(st.hover));
        let _ = writeln!(s, "  --m3-{name}-pressed: {};", col.rgba(st.pressed));
    }
    let _ = writeln!(s, "  --m3-disabled-content: {};", c.on_surface.rgba(st.disabled_content));
    let _ = writeln!(s, "  --m3-disabled-container: {};", c.on_surface.rgba(st.disabled_container));
    let _ = writeln!(s, "  --m3-scrim-color: {};", c.scrim.rgba(0.32));
    let _ = writeln!(s, "  --m3-shadow-1: 0 1px 2px {}, 0 1px 3px 1px {};", c.shadow.rgba(0.30), c.shadow.rgba(0.15));
    let _ = writeln!(s, "  --m3-shadow-2: 0 1px 2px {}, 0 2px 6px 2px {};", c.shadow.rgba(0.30), c.shadow.rgba(0.15));
    let _ = writeln!(s, "  --m3-shadow-3: 0 4px 8px 3px {}, 0 1px 3px {};", c.shadow.rgba(0.15), c.shadow.rgba(0.30));
    let _ = writeln!(s, "  --m3-shadow-4: 0 6px 10px 4px {}, 0 2px 3px {};", c.shadow.rgba(0.15), c.shadow.rgba(0.30));
    // Translucent containers for panels/popovers when transparency is on.
    let alpha = if transparency { 0.86 } else { 1.0 };
    let _ = writeln!(s, "  --m3-panel-bg: {};", c.surface_container.rgba(alpha));
    let _ = writeln!(s, "  --m3-sheet-bg: {};", c.surface_container_low.rgba(if transparency { 0.92 } else { 1.0 }));

    let sh = &theme.tokens.shape;
    for (name, v) in [
        ("xs", sh.extra_small),
        ("s", sh.small),
        ("m", sh.medium),
        ("l", sh.large),
        ("li", sh.large_increased),
        ("xl", sh.extra_large),
        ("xli", sh.extra_large_increased),
        ("xxl", sh.extra_extra_large),
        ("full", sh.full),
    ] {
        let _ = writeln!(s, "  --m3-radius-{name}: {v}px;");
    }
    let m = &theme.tokens.motion;
    for (name, sp) in [
        ("fast-spatial", &m.fast_spatial),
        ("default-spatial", &m.default_spatial),
        ("slow-spatial", &m.slow_spatial),
        ("fast-effects", &m.fast_effects),
        ("default-effects", &m.default_effects),
        ("slow-effects", &m.slow_effects),
    ] {
        let _ = writeln!(s, "  --m3-dur-{name}: {}ms;", sp.duration_ms);
        let _ = writeln!(s, "  --m3-ease-{name}: {};", sp.easing);
    }
    // Short aliases used throughout the stylesheets.
    let _ = writeln!(s, "  --m3-dur-short: {}ms;", m.fast_effects.duration_ms);
    let _ = writeln!(s, "  --m3-dur-medium: {}ms;", m.fast_spatial.duration_ms);
    let _ = writeln!(s, "  --m3-dur-long: {}ms;", m.default_spatial.duration_ms);
    let _ = writeln!(s, "  --m3-ease-spatial: {};", m.fast_spatial.easing);
    let _ = writeln!(s, "  --m3-ease-effects: {};", m.default_effects.easing);
    let _ = writeln!(
        s,
        "  --m3-font: \"{}\", \"Google Sans Flex\", \"Roboto Flex\", \"Inter\", \"Roboto\", \"Cantarell\", sans-serif;",
        css_escape(font_family)
    );
    let _ = writeln!(s, "  --m3-font-mono: \"{}\", monospace;", css_escape(mono_family));
    s.push_str("}\n\n");

    // Typescale as classes (custom properties can't hold whole font
    // shorthands in GTK, so emit concrete rules).
    for (class, t) in theme.tokens.typography.styles() {
        type_rule(&mut s, class, t, false);
    }
    for (class, t) in theme.tokens.emphasized.styles() {
        type_rule(&mut s, class, t, true);
    }
    s
}

fn type_rule(s: &mut String, class: &str, t: &TypeStyle, emphasized: bool) {
    // Large styles use Google Sans Flex's rounded terminals (ROND axis);
    // fonts without the axis simply ignore it.
    let large = class.starts_with("display") || class.starts_with("headline");
    let rond = match (large, emphasized) {
        (true, true) => "font-variation-settings: \"ROND\" 100, \"wdth\" 106; ",
        (true, false) | (false, true) => "font-variation-settings: \"ROND\" 100; ",
        (false, false) => "",
    };
    let suffix = if emphasized { "-emphasized" } else { "" };
    let _ = writeln!(
        s,
        ".m3-{class}{suffix} {{ font-size: {}px; font-weight: {}; letter-spacing: {}px; {rond}}}",
        t.size, t.weight, t.tracking
    );
}

/// Strip characters that could break out of a CSS string.
fn css_escape(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '"' | '\\' | ';' | '{' | '}' | '\n' | '\r')).collect()
}

/// The full stylesheet for an m3 GTK application.
pub fn gtk_css(theme: &Theme, font_family: &str, mono_family: &str, transparency: bool) -> String {
    let mut s = variables(theme, font_family, mono_family, transparency);
    // Base font size for body text follows the text scale.
    let _ = writeln!(s, "window.m3, .m3-layer {{ font-size: {}px; }}\n", theme.tokens.typography.body_medium.size);
    s.push_str(COMPONENTS);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use m3_core::config::ThemeConfig;

    #[test]
    fn variables_cover_every_role() {
        let t = Theme::resolve(&ThemeConfig::default(), None, 0);
        let css = gtk_css(&t, "Inter", "JetBrains Mono", true);
        for (name, c) in t.colors.roles() {
            assert!(css.contains(&format!("--m3-{name}: {};", c.hex())), "{name}");
        }
        // Every var() used by the component sheet and the app stylesheets
        // is defined.
        let sheets = [
            ("components.css", COMPONENTS),
            ("shell.css", include_str!("../../m3-shell/src/shell.css")),
            ("tray.css", include_str!("../../m3-shell/src/tray.css")),
            ("desktop.css", include_str!("../../m3-shell/src/desktop.css")),
            ("settings.css", include_str!("../../m3-settings/src/settings.css")),
            ("lock.css", include_str!("../../m3-lock/src/lock.css")),
        ];
        for (file, sheet) in sheets {
            let mut rest = sheet;
            while let Some(i) = rest.find("var(--") {
                rest = &rest[i + 4..];
                let end = rest.find([')', ',']).unwrap_or(rest.len());
                let name = &rest[..end];
                assert!(css.contains(&format!("{name}:")), "{file}: undefined CSS variable {name}");
            }
        }
    }

    #[test]
    fn font_names_cannot_inject_css() {
        let t = Theme::resolve(&ThemeConfig::default(), None, 0);
        let css = variables(&t, "Evil\"; } * { color: red", "x", false);
        assert!(!css.contains("Evil\";"));
    }

    #[test]
    fn braces_balance() {
        let t = Theme::resolve(&ThemeConfig::default(), None, 0);
        let css = gtk_css(&t, "Inter", "Mono", false);
        let stripped: String = {
            // Ignore braces inside comments.
            let mut out = String::new();
            let mut rest = css.as_str();
            while let Some(i) = rest.find("/*") {
                out.push_str(&rest[..i]);
                rest = &rest[i + 2..];
                let j = rest.find("*/").expect("unterminated comment");
                rest = &rest[j + 2..];
            }
            out.push_str(rest);
            out
        };
        assert_eq!(stripped.matches('{').count(), stripped.matches('}').count());
    }
}
