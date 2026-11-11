//! m3 Settings.
//!
//! Every control writes through `m3_core::config` (validated, atomic,
//! comment-preserving) and the compositor/shell pick changes up live. System
//! pages talk to real services through `m3-services`.

mod layout;
mod pages;
mod store;
mod ui;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{gdk, gio, glib, prelude::*};
use m3_core::config::{
    self as cfg, CompositorConfig, Keybindings, LockConfig, NotificationsConfig, PanelsConfig, ThemeConfig,
    WorkspacesConfig,
};
use m3_services::{
    BatteryState, BluetoothState, NetworkState, PowerProfiles, Services,
    audio::{Audio, AudioState},
};
use store::Store;

#[derive(Default, Clone)]
pub struct Sys {
    pub network: Option<NetworkState>,
    pub bluetooth: Option<BluetoothState>,
    pub battery: Option<BatteryState>,
    pub profiles: Option<PowerProfiles>,
    pub brightness: Option<f64>,
    pub audio: Option<AudioState>,
    pub dnd: bool,
}

type SysListener = Box<dyn Fn(&Sys)>;

pub struct Ctx {
    pub dir: PathBuf,
    pub window: gtk::ApplicationWindow,
    pub compositor: Rc<Store<CompositorConfig>>,
    pub theme: Rc<Store<ThemeConfig>>,
    pub panels: Rc<Store<PanelsConfig>>,
    pub workspaces: Rc<Store<WorkspacesConfig>>,
    pub keys: Rc<Store<Keybindings>>,
    pub notifications: Rc<Store<NotificationsConfig>>,
    pub lock: Rc<Store<LockConfig>>,
    pub widgets: Rc<Store<m3_core::config::WidgetsConfig>>,
    pub services: Services,
    pub audio: Audio,
    pub sys: RefCell<Sys>,
    listeners: RefCell<Vec<SysListener>>,
    css: gtk::CssProvider,
    toast: gtk::Revealer,
    toast_label: gtk::Label,
}

impl Ctx {
    /// Run `f` now and whenever system state changes.
    pub fn on_sys(&self, f: impl Fn(&Sys) + 'static) {
        f(&self.sys.borrow());
        self.listeners.borrow_mut().push(Box::new(f));
    }

    fn sys_changed(&self) {
        let s = self.sys.borrow().clone();
        for l in self.listeners.borrow().iter() {
            l(&s);
        }
    }

    pub fn toast(&self, msg: &str) {
        self.toast_label.set_text(msg);
        self.toast.set_reveal_child(true);
        let r = self.toast.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || r.set_reveal_child(false));
    }

    /// Restyle this window after appearance changes.
    pub fn restyle(&self) {
        let t = self.theme.get();
        let seed = (!t.wallpaper.path.as_os_str().is_empty())
            .then(|| m3_theme::wallpaper::cached_seed(&t.wallpaper.path))
            .flatten();
        let theme = m3_theme::Theme::resolve(&t, seed, m3_core::time::local_minutes());
        let mut css = m3_theme::css::gtk_css(&theme, &t.font_family, &t.monospace_font_family, false);
        css.push_str(include_str!("settings.css"));
        self.css.load_from_string(&css);
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_interface_color_scheme(if theme.dark {
                gtk::InterfaceColorScheme::Dark
            } else {
                gtk::InterfaceColorScheme::Light
            });
        }
    }

    pub fn theme_preview(&self) -> m3_theme::Theme {
        let t = self.theme.get();
        let seed = (!t.wallpaper.path.as_os_str().is_empty())
            .then(|| m3_theme::wallpaper::cached_seed(&t.wallpaper.path))
            .flatten();
        m3_theme::Theme::resolve(&t, seed, m3_core::time::local_minutes())
    }
}

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    let app = gtk::Application::builder().application_id("org.m3.Settings").build();
    app.connect_activate(build);
    // `m3-settings --page appearance` opens a specific page.
    let args: Vec<String> = std::env::args().collect();
    let page = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).cloned();
    START_PAGE.with(|p| *p.borrow_mut() = page);
    app.run_with_args::<&str>(&[])
}

thread_local! {
    static START_PAGE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn build(app: &gtk::Application) {
    if let Some(w) = app.active_window() {
        w.present();
        return;
    }
    let dir = match cfg::default_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("{e}");
            return;
        }
    };
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Settings")
        .default_width(1100)
        .default_height(760)
        .build();
    window.add_css_class("m3");
    window.add_css_class("m3-settings-window");
    // A flat header merged with the window; the title lives in the side bar.
    let header = gtk::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.set_titlebar(Some(&header));

    let css = gtk::CssProvider::new();
    if let Some(d) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&d, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let toast = gtk::Revealer::new();
    toast.set_transition_type(gtk::RevealerTransitionType::SlideUp);
    toast.set_halign(gtk::Align::Center);
    toast.set_valign(gtk::Align::End);
    toast.set_margin_bottom(24);
    let toast_label = gtk::Label::new(None);
    toast_label.set_wrap(true);
    let sb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    sb.add_css_class("m3-snackbar");
    sb.append(&toast_label);
    toast.set_child(Some(&sb));

    let err_toast: Rc<RefCell<Option<std::rc::Weak<Ctx>>>> = Rc::new(RefCell::new(None));
    let on_error: Rc<dyn Fn(String)> = {
        let e = err_toast.clone();
        Rc::new(move |msg: String| {
            tracing::warn!("{msg}");
            if let Some(ctx) = e.borrow().as_ref().and_then(|w| w.upgrade()) {
                ctx.toast(&format!("Not saved: {msg}"));
            }
        })
    };
    let (services, events) = Services::start();
    let (audio, audio_events) = Audio::start();
    let ctx = Rc::new(Ctx {
        compositor: Store::load(&dir, true, on_error.clone()),
        theme: Store::load(&dir, true, on_error.clone()),
        panels: Store::load(&dir, false, on_error.clone()),
        workspaces: Store::load(&dir, true, on_error.clone()),
        keys: Store::load(&dir, true, on_error.clone()),
        notifications: Store::load(&dir, false, on_error.clone()),
        lock: Store::load(&dir, false, on_error.clone()),
        widgets: Store::load(&dir, false, on_error.clone()),
        dir,
        window: window.clone(),
        services,
        audio,
        sys: RefCell::new(Sys::default()),
        listeners: RefCell::new(Vec::new()),
        css,
        toast,
        toast_label,
    });
    *err_toast.borrow_mut() = Some(Rc::downgrade(&ctx));
    ctx.restyle();

    // Service events.
    {
        let c = Rc::downgrade(&ctx);
        glib::spawn_future_local(async move {
            while let Ok(e) = events.recv().await {
                let Some(ctx) = c.upgrade() else { break };
                {
                    let mut s = ctx.sys.borrow_mut();
                    use m3_services::Event as E;
                    match e {
                        E::Network(n) => s.network = Some(n),
                        E::Bluetooth(b) => s.bluetooth = Some(b),
                        E::Battery(b) => s.battery = Some(b),
                        E::PowerProfiles(p) => s.profiles = Some(p),
                        E::Brightness(b) => s.brightness = b,
                        E::DoNotDisturb(d) => s.dnd = d,
                        E::Error(msg) => {
                            drop(s);
                            ctx.toast(&msg);
                            continue;
                        }
                        // Settings is not the notification UI.
                        _ => continue,
                    }
                }
                ctx.sys_changed();
            }
        });
        let c = Rc::downgrade(&ctx);
        glib::spawn_future_local(async move {
            while let Ok(a) = audio_events.recv().await {
                let Some(ctx) = c.upgrade() else { break };
                ctx.sys.borrow_mut().audio = Some(a);
                ctx.sys_changed();
            }
        });
    }

    // Navigation (side bar or rail) + pages.
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(150);
    stack.set_hexpand(true);
    let nav = gtk::ListBox::new();
    nav.add_css_class("m3-navigation-drawer");
    nav.set_selection_mode(gtk::SelectionMode::Single);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search settings"));
    search.add_css_class("m3-search");

    // Each item: the icon in an indicator pill, the label beside it (side
    // bar) or under it (rail).
    let mut nav_items: Vec<(gtk::Box, gtk::Label)> = Vec::new();
    for p in pages::ALL {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("m3-nav-item");
        row.set_tooltip_text(Some(p.title));
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let indicator = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        indicator.add_css_class("m3-nav-indicator");
        indicator.set_halign(gtk::Align::Center);
        let icon = ui::icon(p.icon);
        // Centred in the indicator, but never expanding the row (that would
        // push the label away from the icon).
        icon.set_halign(gtk::Align::Center);
        icon.set_hexpand(true);
        indicator.set_hexpand(false);
        indicator.set_halign(gtk::Align::Start);
        indicator.append(&icon);
        b.append(&indicator);
        let l = gtk::Label::new(Some(p.title));
        l.set_xalign(0.0);
        l.add_css_class("m3-nav-label");
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.append(&l);
        row.set_child(Some(&b));
        row.set_widget_name(p.id);
        nav.append(&row);
        nav_items.push((b, l));
    }
    let built: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let stack = stack.clone();
        let ctx = ctx.clone();
        let built = built.clone();
        nav.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let id = row.widget_name().to_string();
            let Some(p) = pages::ALL.iter().find(|p| p.id == id) else { return };
            // Pages are built lazily the first time they are opened.
            if !built.borrow().contains(&p.id) {
                stack.add_named(&(p.build)(&ctx), Some(p.id));
                built.borrow_mut().push(p.id);
            }
            stack.set_visible_child_name(p.id);
            if let Some(page) = stack.visible_child() {
                m3_widgets::animate(&page, "m3-enter-up");
            }
        });
    }
    {
        let nav2 = nav.clone();
        search.connect_search_changed(move |s| {
            let q = s.text().to_lowercase();
            let mut i = 0;
            while let Some(row) = nav2.row_at_index(i) {
                let id = row.widget_name().to_string();
                let visible = pages::ALL.iter().find(|p| p.id == id).is_some_and(|p| {
                    q.is_empty() || p.title.to_lowercase().contains(&q) || p.keywords.iter().any(|k| k.contains(&q))
                });
                row.set_visible(visible);
                i += 1;
            }
        });
    }

    let side = gtk::Box::new(gtk::Orientation::Vertical, 12);
    side.add_css_class("m3-settings-side");
    let side_title = gtk::Label::new(Some("Settings"));
    side_title.add_css_class("m3-headline-medium-emphasized");
    side_title.add_css_class("m3-settings-title");
    side_title.set_xalign(0.0);
    side.append(&side_title);
    // In the rail, search lives in a popover behind a search button.
    let search_button = gtk::MenuButton::new();
    search_button.add_css_class("m3-rail-search");
    search_button.set_child(Some(&ui::icon("search")));
    search_button.set_tooltip_text(Some("Search settings"));
    search_button.set_halign(gtk::Align::Center);
    let search_pop = gtk::Popover::new();
    search_pop.set_position(gtk::PositionType::Right);
    search_button.set_popover(Some(&search_pop));
    {
        let s = search.clone();
        search_pop.connect_show(move |_| {
            s.grab_focus();
        });
    }
    side.append(&search_button);
    side.append(&search);
    let nav_scroll = gtk::ScrolledWindow::new();
    nav_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    nav_scroll.set_child(Some(&nav));
    nav_scroll.set_vexpand(true);
    side.append(&nav_scroll);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.append(&side);
    // Pages sit on a large rounded pane, like M3 Expressive's list-detail layouts.
    let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pane.add_css_class("m3-settings-pane");
    pane.set_overflow(gtk::Overflow::Hidden);
    pane.set_hexpand(true);
    pane.append(&stack);
    body.append(&pane);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&body));
    overlay.add_overlay(&ctx.toast);
    // Resize sensor: picks wide / medium / narrow from the window width.
    let sensor = gtk::DrawingArea::new();
    sensor.set_can_target(false);
    overlay.add_overlay(&sensor);
    window.set_child(Some(&overlay));
    window.set_size_request(360, 480);

    let apply_mode = {
        let (window, side, side_title, search, search_button, search_pop) = (
            window.clone(),
            side.clone(),
            side_title.clone(),
            search.clone(),
            search_button.clone(),
            search_pop.clone(),
        );
        move |mode: layout::Mode| {
            for m in [layout::Mode::Wide, layout::Mode::Medium, layout::Mode::Narrow] {
                window.remove_css_class(m.css_class());
            }
            window.add_css_class(mode.css_class());
            let rail = mode != layout::Mode::Wide;
            side_title.set_visible(!rail);
            search_button.set_visible(rail);
            side.set_size_request(if rail { 88 } else { 280 }, -1);
            // Move the search field between the side bar and the popover.
            let in_side = search.parent().is_some_and(|p| p == *side.upcast_ref::<gtk::Widget>());
            if rail && in_side {
                side.remove(&search);
                search_pop.set_child(Some(&search));
            } else if !rail && !in_side {
                search_pop.set_child(None::<&gtk::Widget>);
                side.insert_child_after(&search, Some(&search_button));
            }
            for (b, l) in &nav_items {
                if let Some(ind) = b.first_child() {
                    ind.set_halign(if rail { gtk::Align::Center } else { gtk::Align::Start });
                }
                l.set_hexpand(!rail);
                b.set_orientation(if rail { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal });
                b.set_spacing(if rail { 4 } else { 12 });
                l.set_xalign(if rail { 0.5 } else { 0.0 });
                l.set_max_width_chars(if rail { 9 } else { -1 });
            }
            layout::set_mode(mode);
        }
    };
    apply_mode(layout::Mode::Wide);
    sensor.connect_resize(move |_, w, _| {
        let mode = layout::mode_for_width(w);
        if mode != layout::mode() {
            apply_mode(mode);
        }
    });

    let start = START_PAGE.with(|p| p.borrow().clone()).unwrap_or_else(|| "appearance".into());
    let index = pages::ALL.iter().position(|p| p.id == start).unwrap_or(0);
    nav.select_row(nav.row_at_index(index as i32).as_ref());

    // Keep the context alive with the window.
    let keep = ctx.clone();
    window.connect_destroy(move |_| {
        let _ = &keep;
    });
    window.present();
    let _ = gio::Application::default();
}
