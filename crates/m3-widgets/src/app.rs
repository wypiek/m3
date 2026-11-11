//! Theming for m3 applications: loads `theme.toml`, applies the generated
//! Material stylesheet plus the app's own CSS, and restyles live when the
//! theme changes (file edits, Settings, or the automatic dark schedule).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use m3_core::config::{self as cfg, ThemeConfig};
use m3_theme::Theme;

type Listener = Box<dyn Fn(&ThemeConfig, &Theme)>;

/// The live theme of an m3 application.
pub struct AppStyle {
    provider: gtk::CssProvider,
    app_css: String,
    dir: Option<PathBuf>,
    cfg: RefCell<ThemeConfig>,
    theme: RefCell<Theme>,
    listeners: RefCell<Vec<Listener>>,
    _monitor: RefCell<Option<gio::FileMonitor>>,
}

/// Resolve the theme for `cfg` the way the shell does (wallpaper seed from
/// the extraction cache).
pub fn resolve(cfg: &ThemeConfig) -> Theme {
    let seed = (!cfg.wallpaper.path.as_os_str().is_empty())
        .then(|| m3_theme::wallpaper::cached_seed(&cfg.wallpaper.path))
        .flatten();
    Theme::resolve(cfg, seed, m3_core::time::local_minutes())
}

impl AppStyle {
    /// Install the m3 theme for the default display. `app_css` is appended
    /// after the shared component stylesheet.
    pub fn install(app_css: &str) -> Rc<Self> {
        let provider = gtk::CssProvider::new();
        provider.connect_parsing_error(|_, section, error| {
            tracing::warn!("CSS error at {}: {error}", section.to_str());
        });
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let dir = match cfg::default_dir() {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!("no configuration directory: {e}");
                None
            }
        };
        let config = dir.as_ref().map(|d| cfg::load_or(d, ThemeConfig::default()).0).unwrap_or_default();
        let theme = resolve(&config);
        let me = Rc::new(Self {
            provider,
            app_css: app_css.to_owned(),
            dir,
            cfg: RefCell::new(config),
            theme: RefCell::new(theme),
            listeners: RefCell::new(Vec::new()),
            _monitor: RefCell::new(None),
        });
        me.apply();
        me.watch();
        me
    }

    pub fn config(&self) -> ThemeConfig {
        self.cfg.borrow().clone()
    }

    pub fn theme(&self) -> Theme {
        self.theme.borrow().clone()
    }

    /// Called after every restyle (and not initially).
    pub fn connect_changed(&self, f: impl Fn(&ThemeConfig, &Theme) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    fn apply(&self) {
        let c = self.cfg.borrow();
        let t = self.theme.borrow();
        let mut css = m3_theme::css::gtk_css(&t, &c.font_family, &c.monospace_font_family, false);
        css.push_str(&self.app_css);
        self.provider.load_from_string(&css);
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_interface_color_scheme(if t.dark {
                gtk::InterfaceColorScheme::Dark
            } else {
                gtk::InterfaceColorScheme::Light
            });
            s.set_gtk_enable_animations(!c.reduced_motion);
            s.set_gtk_icon_theme_name(Some(&c.icon_theme));
        }
    }

    fn reload(&self, force: bool) {
        let Some(dir) = &self.dir else { return };
        let (new_cfg, err) = cfg::load_or(dir, self.cfg.borrow().clone());
        if let Some(e) = err {
            tracing::warn!("theme.toml: {e}");
        }
        let new_theme = resolve(&new_cfg);
        let changed = force || new_cfg != *self.cfg.borrow() || new_theme.dark != self.theme.borrow().dark;
        if !changed {
            return;
        }
        *self.cfg.borrow_mut() = new_cfg;
        *self.theme.borrow_mut() = new_theme;
        self.apply();
        let (c, t) = (self.config(), self.theme());
        for l in self.listeners.borrow().iter() {
            l(&c, &t);
        }
    }

    fn watch(self: &Rc<Self>) {
        let Some(dir) = &self.dir else { return };
        match gio::File::for_path(dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            Ok(m) => {
                let weak = Rc::downgrade(self);
                m.connect_changed(move |_, file, other, _| {
                    let named = |f: Option<&gio::File>| {
                        f.and_then(|f| f.basename()).is_some_and(|n| n.to_string_lossy() == "theme.toml")
                    };
                    if !(named(Some(file)) || named(other)) {
                        return;
                    }
                    let weak = weak.clone();
                    // Let atomic saves finish before reading.
                    glib::timeout_add_local_once(Duration::from_millis(120), move || {
                        if let Some(s) = weak.upgrade() {
                            s.reload(true);
                        }
                    });
                });
                *self._monitor.borrow_mut() = Some(m);
            }
            Err(e) => tracing::warn!("cannot watch {}: {e}", dir.display()),
        }
        // The automatic light/dark schedule.
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(60, move || match weak.upgrade() {
            Some(s) => {
                s.reload(false);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }
}
