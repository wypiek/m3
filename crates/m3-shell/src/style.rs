//! Applies the m3 theme to the shell's GTK display.

use gtk::gdk;
use m3_core::config::ThemeConfig;
use m3_theme::Theme;

/// Shell-specific layout rules (components come from m3-theme).
const SHELL_CSS: &str = include_str!("shell.css");
/// System tray (StatusNotifierItem) styles.
const TRAY_CSS: &str = include_str!("tray.css");
/// Desktop widgets.
const DESKTOP_CSS: &str = include_str!("desktop.css");

pub struct Style {
    provider: gtk::CssProvider,
}

impl Style {
    pub fn new() -> Self {
        let provider = gtk::CssProvider::new();
        provider.connect_parsing_error(|_, section, error| {
            tracing::warn!("CSS error at {}: {error}", section.to_str());
        });
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        Self { provider }
    }

    pub fn apply(&self, cfg: &ThemeConfig, theme: &Theme) {
        let mut css = m3_theme::css::gtk_css(theme, &cfg.font_family, &cfg.monospace_font_family, cfg.transparency);
        css.push_str(SHELL_CSS);
        css.push_str(TRAY_CSS);
        css.push_str(DESKTOP_CSS);
        self.provider.load_from_string(&css);
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_interface_color_scheme(if theme.dark {
                gtk::InterfaceColorScheme::Dark
            } else {
                gtk::InterfaceColorScheme::Light
            });
            settings.set_gtk_enable_animations(!cfg.reduced_motion);
            let size = (11.0 * cfg.text_scale).round() as i32;
            settings.set_gtk_font_name(Some(&format!("{} {size}", cfg.font_family)));
            settings.set_gtk_icon_theme_name(Some(&cfg.icon_theme));
        }
    }
}
