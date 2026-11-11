//! m3 Browser — a WebKitGTK browser with Material 3 Expressive chrome.

mod address;
mod downloads;
mod library;
mod pages;
mod store;
mod ui;
mod window;

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use m3_widgets::AppStyle;

use crate::downloads::Downloads;
use crate::store::Store;
use crate::window::Window;

pub const APP_ID: &str = "org.m3.Browser";

/// State shared by every browser window.
pub struct App {
    pub gtk: gtk::Application,
    pub session: webkit6::NetworkSession,
    pub settings: webkit6::Settings,
    pub store: RefCell<Store>,
    pub downloads: Rc<Downloads>,
    pub style: Rc<AppStyle>,
    pub windows: RefCell<Vec<Rc<Window>>>,
}

fn data_dir() -> PathBuf {
    glib::user_data_dir().join("m3").join("browser")
}

fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("m3").join("browser")
}

impl App {
    fn new(gtk_app: &gtk::Application) -> Rc<Self> {
        let style = AppStyle::install(include_str!("browser.css"));
        let (data, cache) = (data_dir(), cache_dir());
        for d in [&data, &cache] {
            if let Err(e) = std::fs::create_dir_all(d) {
                tracing::warn!("cannot create {}: {e}", d.display());
            }
        }
        // A persistent session: cookies, storage and cache survive restarts.
        let session = webkit6::NetworkSession::new(Some(&data.to_string_lossy()), Some(&cache.to_string_lossy()));
        session.set_itp_enabled(true);
        session.set_tls_errors_policy(webkit6::TLSErrorsPolicy::Fail);
        if let Some(m) = session.website_data_manager() {
            m.set_favicons_enabled(true);
        }
        if let Some(cookies) = session.cookie_manager() {
            cookies.set_persistent_storage(
                &data.join("cookies.sqlite").to_string_lossy(),
                webkit6::CookiePersistentStorage::Sqlite,
            );
            cookies.set_accept_policy(webkit6::CookieAcceptPolicy::NoThirdParty);
        }

        let settings = webkit6::Settings::new();
        settings.set_enable_smooth_scrolling(true);
        settings.set_enable_back_forward_navigation_gestures(true);
        settings.set_enable_fullscreen(true);
        settings.set_enable_developer_extras(true);
        settings.set_javascript_can_open_windows_automatically(false);
        settings.set_user_agent_with_application_details(Some("m3Browser"), Some(env!("CARGO_PKG_VERSION")));

        let app = Rc::new(Self {
            gtk: gtk_app.clone(),
            session,
            settings,
            store: RefCell::new(Store::open(data)),
            downloads: Downloads::new(),
            style,
            windows: RefCell::new(Vec::new()),
        });
        let weak = Rc::downgrade(&app);
        app.session.connect_download_started(move |_, d| {
            if let Some(a) = weak.upgrade() {
                a.downloads.add(d);
            }
        });
        app.install_actions();
        app
    }

    fn install_actions(self: &Rc<Self>) {
        let engine = self.store.borrow().settings.search_engine.clone();
        let a = gio::SimpleAction::new_stateful("engine", Some(glib::VariantTy::STRING), &engine.to_variant());
        let weak = Rc::downgrade(self);
        a.connect_activate(move |a, p| {
            let (Some(app), Some(id)) = (weak.upgrade(), p.and_then(|p| p.get::<String>())) else { return };
            if !address::ENGINES.iter().any(|(e, _, _)| *e == id) {
                return;
            }
            a.set_state(&id.to_variant());
            app.store.borrow_mut().set_engine(&id);
        });
        self.gtk.add_action(&a);

        let accels: &[(&str, &[&str])] = &[
            ("win.new-tab", &["<Control>t"]),
            ("win.new-window", &["<Control>n"]),
            ("win.close-tab", &["<Control>w"]),
            ("win.reload", &["<Control>r", "F5"]),
            ("win.reload-hard", &["<Control><Shift>r", "<Shift>F5"]),
            ("win.back", &["<Alt>Left"]),
            ("win.forward", &["<Alt>Right"]),
            ("win.focus-address", &["<Control>l", "<Alt>d", "F6"]),
            ("win.find", &["<Control>f"]),
            ("win.zoom-in", &["<Control>plus", "<Control>equal", "<Control>KP_Add"]),
            ("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]),
            ("win.zoom-reset", &["<Control>0", "<Control>KP_0"]),
            ("win.next-tab", &["<Control>Tab", "<Control>Page_Down"]),
            ("win.prev-tab", &["<Control><Shift>Tab", "<Control><Shift>ISO_Left_Tab", "<Control>Page_Up"]),
            ("win.bookmark", &["<Control>d"]),
            ("win.library", &["<Control>h", "<Control><Shift>o"]),
            ("win.downloads", &["<Control><Shift>y"]),
            ("win.fullscreen", &["F11"]),
        ];
        for (action, keys) in accels {
            self.gtk.set_accels_for_action(action, keys);
        }
    }

    /// Open a new window; `urls` become tabs (or a new-tab page if empty).
    pub fn new_window(self: &Rc<Self>, urls: &[String]) -> Rc<Window> {
        let w = Window::new(self);
        if urls.is_empty() {
            w.add_tab(None, None, true);
            w.focus_address();
        } else {
            for (i, u) in urls.iter().enumerate() {
                w.add_tab(Some(u), None, i == 0);
            }
        }
        self.windows.borrow_mut().push(w.clone());
        w.present();
        w
    }

    pub fn forget_window(&self, w: &Window) {
        self.windows.borrow_mut().retain(|x| !std::ptr::eq(Rc::as_ptr(x), w));
    }

    /// The most recently focused window, if any.
    pub fn active_window(&self) -> Option<Rc<Window>> {
        let active = self.gtk.active_window();
        let windows = self.windows.borrow();
        active
            .and_then(|a| windows.iter().find(|w| w.gtk_window().upcast_ref::<gtk::Window>() == &a).cloned())
            .or_else(|| windows.last().cloned())
    }

    pub fn engine(&self) -> String {
        self.store.borrow().settings.search_engine.clone()
    }

    /// Bookmarks changed: refresh every window's star and new-tab pages.
    pub fn bookmarks_changed(&self) {
        for w in self.windows.borrow().iter() {
            w.refresh_bookmarks();
        }
    }
}

/// URLs from the command line / desktop file that the browser will open.
fn openable(file: &gio::File) -> Option<String> {
    let uri = file.uri().to_string();
    let scheme = uri.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "file").then_some(uri)
}

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    let gtk_app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    let app: Rc<OnceCell<Rc<App>>> = Rc::default();
    {
        let app = app.clone();
        gtk_app.connect_startup(move |g| {
            let _ = app.set(App::new(g));
        });
    }
    {
        let app = app.clone();
        gtk_app.connect_activate(move |_| {
            if let Some(a) = app.get() {
                a.new_window(&[]);
            }
        });
    }
    gtk_app.connect_open(move |_, files, _| {
        let Some(a) = app.get() else { return };
        let urls: Vec<String> = files.iter().filter_map(openable).collect();
        if urls.is_empty() {
            a.new_window(&[]);
            return;
        }
        match a.active_window() {
            Some(w) => {
                for (i, u) in urls.iter().enumerate() {
                    w.add_tab(Some(u), None, i == 0);
                }
                w.present();
            }
            None => {
                a.new_window(&urls);
            }
        }
    });
    gtk_app.run()
}
