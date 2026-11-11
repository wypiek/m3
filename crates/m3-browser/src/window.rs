//! A browser window: the tab strip, toolbar, find bar and tab pages.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use m3_widgets::{LoadingIndicator, WavyProgress};
use webkit6::prelude::*;

use crate::App;
use crate::address::{self, display_url, host_of};
use crate::pages::{self, ErrorKind, NewTabPage};
use crate::store::is_recordable;
use crate::ui::{icon, icon_button};

type WinAction = Box<dyn Fn(&Rc<Window>)>;

const ZOOM_STEPS: [f64; 13] = [0.3, 0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

/// One tab: its web view, new-tab page and strip button.
pub struct Tab {
    pub view: webkit6::WebView,
    page: gtk::Stack,
    ntp: NewTabPage,
    button: gtk::Box,
    title: gtk::Label,
    favicon: gtk::Image,
    favicon_stack: gtk::Stack,
    /// Showing the new-tab page (no web content yet).
    blank: Cell<bool>,
}

impl Tab {
    fn page_title(&self) -> String {
        if self.blank.get() {
            return "New tab".into();
        }
        let t = self.view.title().map(|t| t.to_string()).unwrap_or_default();
        if !t.trim().is_empty() {
            return t;
        }
        self.view.uri().and_then(|u| host_of(&u)).unwrap_or_else(|| "Untitled".into())
    }

    fn uri(&self) -> String {
        if self.blank.get() { String::new() } else { self.view.uri().map(|u| u.to_string()).unwrap_or_default() }
    }
}

pub struct Window {
    app: Weak<App>,
    win: gtk::ApplicationWindow,
    header: gtk::HeaderBar,
    tab_strip: gtk::Box,
    pages: gtk::Stack,
    chrome: gtk::Box,
    back: gtk::Button,
    forward: gtk::Button,
    reload: gtk::Button,
    reload_icon: gtk::Label,
    address: gtk::Entry,
    star: gtk::ToggleButton,
    downloads_button: gtk::MenuButton,
    progress: WavyProgress,
    progress_revealer: gtk::Revealer,
    find_revealer: gtk::Revealer,
    find_entry: gtk::SearchEntry,
    find_label: gtk::Label,
    suggest: gtk::Popover,
    suggest_list: gtk::ListBox,
    tabs: RefCell<Vec<Rc<Tab>>>,
    current: RefCell<Option<Rc<Tab>>>,
    updating: Cell<bool>,
    video_fullscreen: Cell<bool>,
}

/// Whether keyboard focus is in `w` or one of its children.
fn focused(w: &impl IsA<gtk::Widget>) -> bool {
    w.as_ref().state_flags().contains(gtk::StateFlags::FOCUS_WITHIN)
}

fn find_options() -> u32 {
    (webkit6::FindOptions::CASE_INSENSITIVE | webkit6::FindOptions::WRAP_AROUND).bits()
}

impl Window {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let win = gtk::ApplicationWindow::builder()
            .application(&app.gtk)
            .title("m3 Browser")
            .default_width(1280)
            .default_height(840)
            .build();
        win.add_css_class("m3");
        win.add_css_class("m3-browser-window");

        // Header: the tab strip.
        let header = gtk::HeaderBar::new();
        header.add_css_class("m3-browser-header");
        let strip_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        strip_box.set_hexpand(true);
        let tab_strip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tab_strip.add_css_class("m3-tabstrip");
        let strip_scroll = gtk::ScrolledWindow::new();
        strip_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        strip_scroll.set_propagate_natural_width(true);
        strip_scroll.set_child(Some(&tab_strip));
        strip_scroll.set_hexpand(false);
        strip_box.append(&strip_scroll);
        let new_tab = icon_button("add", "New tab (Ctrl+T)");
        new_tab.add_css_class("m3-new-tab");
        new_tab.set_action_name(Some("win.new-tab"));
        new_tab.set_valign(gtk::Align::Center);
        strip_box.append(&new_tab);
        // Left-aligned tab strip (the title slot would centre it).
        header.set_title_widget(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
        header.pack_start(&strip_box);
        win.set_titlebar(Some(&header));

        // Toolbar.
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("m3-browser-toolbar");
        let nav = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        nav.add_css_class("m3-button-group");
        nav.add_css_class("m3-nav-group");
        let back = icon_button("arrow_back", "Back (Alt+Left)");
        back.set_action_name(Some("win.back"));
        let forward = icon_button("arrow_forward", "Forward (Alt+Right)");
        forward.set_action_name(Some("win.forward"));
        let reload = gtk::Button::new();
        let reload_icon = icon("refresh");
        reload.set_child(Some(&reload_icon));
        reload.add_css_class("icon");
        reload.set_tooltip_text(Some("Reload (Ctrl+R)"));
        reload.update_property(&[gtk::accessible::Property::Label("Reload")]);
        nav.append(&back);
        nav.append(&forward);
        nav.append(&reload);
        toolbar.append(&nav);

        let address = gtk::Entry::new();
        address.add_css_class("m3-address");
        address.set_hexpand(true);
        address.set_placeholder_text(Some("Search or enter address"));
        address.set_input_purpose(gtk::InputPurpose::Url);
        address.update_property(&[gtk::accessible::Property::Label("Address and search")]);
        toolbar.append(&address);

        let star = gtk::ToggleButton::new();
        star.set_child(Some(&icon("star")));
        star.add_css_class("icon");
        star.add_css_class("m3-star");
        star.set_tooltip_text(Some("Bookmark this page (Ctrl+D)"));
        star.update_property(&[gtk::accessible::Property::Label("Bookmark this page")]);
        toolbar.append(&star);

        let downloads_button = gtk::MenuButton::new();
        downloads_button.set_child(Some(&icon("download")));
        downloads_button.add_css_class("icon");
        downloads_button.set_tooltip_text(Some("Downloads"));
        downloads_button.set_visible(app.downloads.count() > 0);
        let dl_pop = gtk::Popover::new();
        dl_pop.add_css_class("m3-downloads-popover");
        downloads_button.set_popover(Some(&dl_pop));
        toolbar.append(&downloads_button);

        let menu_button = gtk::MenuButton::new();
        menu_button.set_child(Some(&icon("more_vert")));
        menu_button.add_css_class("icon");
        menu_button.set_tooltip_text(Some("Menu"));
        menu_button.set_menu_model(Some(&main_menu()));
        toolbar.append(&menu_button);

        // Expressive wavy progress while loading.
        let progress = WavyProgress::new(Some(0.0));
        progress.widget().add_css_class("m3-browser-progress");
        let progress_revealer = gtk::Revealer::new();
        progress_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        progress_revealer.set_transition_duration(200);
        progress_revealer.set_child(Some(progress.widget()));

        // Find bar.
        let find_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        find_bar.add_css_class("m3-find-bar");
        let find_entry = gtk::SearchEntry::new();
        find_entry.set_placeholder_text(Some("Find in page"));
        find_entry.set_hexpand(true);
        let find_label = gtk::Label::new(None);
        find_label.add_css_class("m3-label-large");
        find_label.add_css_class("m3-secondary-text");
        let find_prev = icon_button("keyboard_arrow_up", "Previous match (Shift+Enter)");
        let find_next = icon_button("keyboard_arrow_down", "Next match (Enter)");
        let find_close = icon_button("close", "Close find bar (Escape)");
        for w in [
            find_entry.upcast_ref::<gtk::Widget>(),
            find_label.upcast_ref(),
            find_prev.upcast_ref(),
            find_next.upcast_ref(),
            find_close.upcast_ref(),
        ] {
            find_bar.append(w);
        }
        let find_revealer = gtk::Revealer::new();
        find_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        find_revealer.set_child(Some(&find_bar));

        let chrome = gtk::Box::new(gtk::Orientation::Vertical, 0);
        chrome.add_css_class("m3-browser-chrome");
        chrome.append(&toolbar);
        chrome.append(&find_revealer);

        let pages = gtk::Stack::new();
        pages.set_vexpand(true);
        pages.set_hexpand(true);
        pages.set_transition_type(gtk::StackTransitionType::Crossfade);
        pages.set_transition_duration(120);
        let pane = gtk::Overlay::new();
        pane.add_css_class("m3-browser-pane");
        pane.set_overflow(gtk::Overflow::Hidden);
        pane.set_child(Some(&pages));
        progress_revealer.set_valign(gtk::Align::Start);
        progress_revealer.set_can_target(false);
        pane.add_overlay(&progress_revealer);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&chrome);
        body.append(&pane);
        win.set_child(Some(&body));

        // Address suggestions.
        let suggest_list = gtk::ListBox::new();
        suggest_list.add_css_class("m3-suggestions");
        suggest_list.set_selection_mode(gtk::SelectionMode::Single);
        suggest_list.set_can_focus(false);
        suggest_list.set_activate_on_single_click(true);
        let suggest = gtk::Popover::new();
        suggest.add_css_class("m3-suggest-popover");
        suggest.set_has_arrow(false);
        suggest.set_autohide(false);
        suggest.set_can_focus(false);
        suggest.set_position(gtk::PositionType::Bottom);
        suggest.set_halign(gtk::Align::Start);
        suggest.set_child(Some(&suggest_list));
        suggest.set_parent(&address);

        let w = Rc::new(Self {
            app: Rc::downgrade(app),
            win,
            header,
            tab_strip,
            pages,
            chrome,
            back,
            forward,
            reload,
            reload_icon,
            address,
            star,
            downloads_button,
            progress,
            progress_revealer,
            find_revealer,
            find_entry,
            find_label,
            suggest,
            suggest_list,
            tabs: RefCell::new(Vec::new()),
            current: RefCell::new(None),
            updating: Cell::new(false),
            video_fullscreen: Cell::new(false),
        });
        w.install_actions();
        w.wire_toolbar();
        w.wire_find(&find_prev, &find_next, &find_close);
        {
            let weak = Rc::downgrade(&w);
            app.downloads.connect_changed(move |n| {
                if let Some(w) = weak.upgrade() {
                    w.downloads_button.set_visible(n > 0);
                    // Show the list when a download starts in this window.
                    if w.win.is_active() {
                        w.show_downloads();
                    }
                }
            });
            let weak = Rc::downgrade(&w);
            dl_pop.connect_show(move |p| {
                if let Some(a) = weak.upgrade().and_then(|w| w.app.upgrade()) {
                    a.downloads.attach(p);
                }
            });
        }
        {
            let weak = Rc::downgrade(&w);
            w.win.connect_destroy(move |_| {
                let Some(w) = weak.upgrade() else { return };
                w.suggest.unparent();
                if let Some(a) = w.app.upgrade() {
                    if let Some(p) = w.downloads_button.popover() {
                        a.downloads.detach(&p);
                    }
                    a.forget_window(&w);
                }
            });
        }
        w
    }

    pub fn gtk_window(&self) -> &gtk::ApplicationWindow {
        &self.win
    }

    pub fn present(&self) {
        self.win.present();
    }

    pub fn current(&self) -> Option<Rc<Tab>> {
        self.current.borrow().clone()
    }

    fn app(&self) -> Option<Rc<App>> {
        self.app.upgrade()
    }

    // ---- actions ---------------------------------------------------------

    fn install_actions(self: &Rc<Self>) {
        let add = |name: &str, f: WinAction| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(w) = weak.upgrade() {
                    f(&w);
                }
            });
            self.win.add_action(&a);
        };
        add(
            "new-tab",
            Box::new(|w| {
                w.add_tab(None, None, true);
                w.focus_address();
            }),
        );
        add(
            "new-window",
            Box::new(|w| {
                if let Some(a) = w.app() {
                    a.new_window(&[]);
                }
            }),
        );
        add(
            "close-tab",
            Box::new(|w| {
                if let Some(t) = w.current() {
                    w.close_tab(&t);
                }
            }),
        );
        add("reload", Box::new(|w| w.with_view(|v| v.reload())));
        add("reload-hard", Box::new(|w| w.with_view(|v| v.reload_bypass_cache())));
        add(
            "stop",
            Box::new(|w| {
                if w.suggest.is_visible() {
                    w.hide_suggestions();
                } else if w.find_revealer.reveals_child() {
                    w.close_find();
                } else {
                    w.with_view(|v| {
                        if v.is_loading() {
                            v.stop_loading();
                        }
                    });
                }
            }),
        );
        add("back", Box::new(|w| w.with_view(|v| v.go_back())));
        add("forward", Box::new(|w| w.with_view(|v| v.go_forward())));
        add("focus-address", Box::new(|w| w.focus_address()));
        add("find", Box::new(|w| w.open_find()));
        add("zoom-in", Box::new(|w| w.zoom(1)));
        add("zoom-out", Box::new(|w| w.zoom(-1)));
        add("zoom-reset", Box::new(|w| w.zoom(0)));
        add("next-tab", Box::new(|w| w.cycle_tab(1)));
        add("prev-tab", Box::new(|w| w.cycle_tab(-1)));
        add("bookmark", Box::new(|w| w.star.set_active(!w.star.is_active())));
        add("library", Box::new(crate::library::show));
        add("downloads", Box::new(|w| w.show_downloads()));
        add("about", Box::new(|w| crate::library::about(w.win.upcast_ref())));
        add(
            "fullscreen",
            Box::new(|w| {
                if w.win.is_fullscreen() {
                    w.win.unfullscreen();
                } else {
                    w.win.fullscreen();
                }
            }),
        );
    }

    fn with_view(&self, f: impl FnOnce(&webkit6::WebView)) {
        if let Some(t) = self.current()
            && !t.blank.get()
        {
            f(&t.view);
        }
    }

    fn show_downloads(&self) {
        self.downloads_button.set_visible(true);
        self.downloads_button.popup();
    }

    // ---- tabs ------------------------------------------------------------

    /// Add a tab. `url = None` shows the new-tab page. `related` makes the
    /// view share a web process with its opener (window.open).
    pub fn add_tab(
        self: &Rc<Self>,
        url: Option<&str>,
        related: Option<&webkit6::WebView>,
        select: bool,
    ) -> Option<Rc<Tab>> {
        let app = self.app()?;
        let view = match related {
            Some(r) => webkit6::WebView::builder().related_view(r).settings(&app.settings).build(),
            None => webkit6::WebView::builder().network_session(&app.session).settings(&app.settings).build(),
        };
        view.set_vexpand(true);
        view.set_hexpand(true);

        let weak = Rc::downgrade(self);
        let tab_cell: Rc<RefCell<Weak<Tab>>> = Rc::default();
        let go: Rc<dyn Fn(&str)> = {
            let (weak, tab_cell) = (weak.clone(), tab_cell.clone());
            Rc::new(move |text: &str| {
                let (Some(w), Some(t)) = (weak.upgrade(), tab_cell.borrow().upgrade()) else { return };
                if let Some(url) = w.app().and_then(|a| address::resolve(text, &a.engine())) {
                    w.load_in(&t, &url);
                }
            })
        };
        let ntp = NewTabPage::new(go);
        let page = gtk::Stack::new();
        page.add_named(&ntp.root, Some("ntp"));
        page.add_named(&view, Some("web"));

        // Strip button: favicon (or a loading indicator) + title + close.
        let button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        button.add_css_class("m3-tab");
        button.add_css_class("m3-enter");
        let main = gtk::Button::new();
        main.add_css_class("m3-tab-main");
        let inner = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let favicon_stack = gtk::Stack::new();
        favicon_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        let favicon = gtk::Image::new();
        favicon.set_pixel_size(16);
        let fallback = icon("public");
        fallback.add_css_class("m3-tab-glyph");
        favicon_stack.add_named(&fallback, Some("glyph"));
        favicon_stack.add_named(&favicon, Some("favicon"));
        favicon_stack.add_named(LoadingIndicator::new(18).widget(), Some("loading"));
        favicon_stack.set_visible_child_name("glyph");
        inner.append(&favicon_stack);
        let title = gtk::Label::new(Some("New tab"));
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_width_chars(10);
        title.set_max_width_chars(18);
        title.set_xalign(0.0);
        title.add_css_class("m3-tab-title");
        inner.append(&title);
        main.set_child(Some(&inner));
        let close = icon_button("close", "Close tab (Ctrl+W)");
        close.add_css_class("m3-tab-close");
        close.set_valign(gtk::Align::Center);
        button.append(&main);
        button.append(&close);

        let tab = Rc::new(Tab { view, page, ntp, button, title, favicon, favicon_stack, blank: Cell::new(true) });
        *tab_cell.borrow_mut() = Rc::downgrade(&tab);
        tab.ntp.set_bookmarks(&app.store.borrow().bookmarks, self.opener(&tab));

        {
            let (wk, t) = (weak.clone(), Rc::downgrade(&tab));
            main.connect_clicked(move |_| {
                if let (Some(w), Some(t)) = (wk.upgrade(), t.upgrade()) {
                    w.select_tab(&t);
                }
            });
            let (wk, t) = (weak.clone(), Rc::downgrade(&tab));
            close.connect_clicked(move |_| {
                if let (Some(w), Some(t)) = (wk.upgrade(), t.upgrade()) {
                    w.close_tab(&t);
                }
            });
            // Middle click closes.
            let middle = gtk::GestureClick::builder().button(2).build();
            let (wk, t) = (weak.clone(), Rc::downgrade(&tab));
            middle.connect_released(move |g, _, _, _| {
                g.set_state(gtk::EventSequenceState::Claimed);
                if let (Some(w), Some(t)) = (wk.upgrade(), t.upgrade()) {
                    w.close_tab(&t);
                }
            });
            tab.button.add_controller(middle);
        }

        self.wire_view(&tab);
        // Insert after the current tab (like most browsers do for openers).
        let index = {
            let tabs = self.tabs.borrow();
            match (related, self.current()) {
                (Some(_), Some(cur)) => tabs.iter().position(|t| Rc::ptr_eq(t, &cur)).map(|i| i + 1),
                _ => None,
            }
            .unwrap_or(tabs.len())
        };
        let before = self.tabs.borrow().get(index.wrapping_sub(1)).map(|t| t.button.clone());
        match before {
            Some(prev) if index > 0 => self.tab_strip.insert_child_after(&tab.button, Some(&prev)),
            _ => self.tab_strip.append(&tab.button),
        }
        let len = self.tabs.borrow().len();
        self.tabs.borrow_mut().insert(index.min(len), tab.clone());
        self.pages.add_child(&tab.page);

        if let Some(u) = url {
            self.load_in(&tab, u);
        } else if related.is_some() {
            // WebKit loads the opener's request into this view itself.
            tab.blank.set(false);
            tab.page.set_visible_child_name("web");
        }
        if select || self.current().is_none() {
            self.select_tab(&tab);
        }
        Some(tab)
    }

    fn opener(self: &Rc<Self>, tab: &Rc<Tab>) -> Rc<dyn Fn(&str)> {
        let (weak, t) = (Rc::downgrade(self), Rc::downgrade(tab));
        Rc::new(move |url: &str| {
            if let (Some(w), Some(t)) = (weak.upgrade(), t.upgrade()) {
                w.load_in(&t, url);
            }
        })
    }

    pub fn select_tab(self: &Rc<Self>, tab: &Rc<Tab>) {
        if let Some(old) = self.current.borrow_mut().replace(tab.clone()) {
            old.button.remove_css_class("selected");
        }
        tab.button.add_css_class("selected");
        self.pages.set_visible_child(&tab.page);
        self.hide_suggestions();
        if self.find_revealer.reveals_child() {
            self.close_find();
        }
        self.sync_chrome();
        if tab.blank.get() {
            tab.ntp.search.grab_focus();
        } else {
            tab.view.grab_focus();
        }
    }

    pub fn close_tab(self: &Rc<Self>, tab: &Rc<Tab>) {
        let index = self.tabs.borrow().iter().position(|t| Rc::ptr_eq(t, tab));
        let Some(index) = index else { return };
        self.tabs.borrow_mut().remove(index);
        self.tab_strip.remove(&tab.button);
        self.pages.remove(&tab.page);
        if tab.view.is_loading() {
            tab.view.stop_loading();
        }
        let next = {
            let tabs = self.tabs.borrow();
            tabs.get(index).or_else(|| tabs.last()).cloned()
        };
        let was_current = self.current().is_some_and(|c| Rc::ptr_eq(&c, tab));
        if was_current {
            *self.current.borrow_mut() = None;
        }
        match next {
            None => self.win.close(),
            Some(n) if was_current => self.select_tab(&n),
            Some(_) => {}
        }
    }

    fn cycle_tab(self: &Rc<Self>, step: isize) {
        let next = {
            let tabs = self.tabs.borrow();
            let Some(cur) = self.current() else { return };
            let Some(i) = tabs.iter().position(|t| Rc::ptr_eq(t, &cur)) else { return };
            let n = tabs.len() as isize;
            tabs.get(((i as isize + step).rem_euclid(n)) as usize).cloned()
        };
        if let Some(t) = next {
            self.select_tab(&t);
        }
    }

    /// Load `url` (already resolved) in `tab`.
    pub fn load_in(self: &Rc<Self>, tab: &Rc<Tab>, url: &str) {
        tab.blank.set(false);
        tab.page.set_visible_child_name("web");
        tab.view.load_uri(url);
        if self.current().is_some_and(|c| Rc::ptr_eq(&c, tab)) {
            self.address.set_text(&display_url(url));
            tab.view.grab_focus();
        }
        self.update_tab_label(tab);
    }

    pub fn refresh_bookmarks(self: &Rc<Self>) {
        let Some(app) = self.app() else { return };
        let tabs = self.tabs.borrow().clone();
        for t in &tabs {
            t.ntp.set_bookmarks(&app.store.borrow().bookmarks, self.opener(t));
        }
        self.sync_star();
    }

    // ---- web view signals --------------------------------------------------

    fn wire_view(self: &Rc<Self>, tab: &Rc<Tab>) {
        let v = &tab.view;
        let weak = Rc::downgrade(self);
        let tw = Rc::downgrade(tab);
        macro_rules! with {
            (|$w:ident, $t:ident| $body:expr) => {{
                let (weak, tw) = (weak.clone(), tw.clone());
                move || {
                    if let (Some($w), Some($t)) = (weak.upgrade(), tw.upgrade()) {
                        $body
                    }
                }
            }};
        }

        let f = with!(|w, t| {
            w.update_tab_label(&t);
            if w.is_current(&t) {
                w.sync_chrome();
            }
            if let Some(a) = w.app() {
                let (u, title) = (t.uri(), t.page_title());
                a.store.borrow_mut().retitle(&u, &title);
            }
        });
        v.connect_title_notify(move |_| f());
        let f = with!(|w, t| {
            if w.is_current(&t) && !focused(&w.address) && !w.suggest.is_visible() {
                w.address.set_text(&display_url(&t.uri()));
            }
            if w.is_current(&t) {
                w.sync_star();
            }
        });
        v.connect_uri_notify(move |_| f());
        let f = with!(|_w, t| {
            match t.view.favicon() {
                Some(tex) => {
                    t.favicon.set_paintable(Some(&tex));
                    if !t.view.is_loading() {
                        t.favicon_stack.set_visible_child_name("favicon");
                    }
                }
                None => t.favicon.set_paintable(None::<&gdk::Paintable>),
            }
        });
        v.connect_favicon_notify(move |_| f());
        let f = with!(|w, t| {
            let loading = t.view.is_loading();
            t.favicon_stack.set_visible_child_name(if loading {
                "loading"
            } else if t.favicon.paintable().is_some() {
                "favicon"
            } else {
                "glyph"
            });
            if w.is_current(&t) {
                w.sync_loading();
            }
        });
        v.connect_is_loading_notify(move |_| f());
        let f = with!(|w, t| {
            if w.is_current(&t) {
                w.progress.set_fraction(Some(t.view.estimated_load_progress()));
            }
        });
        v.connect_estimated_load_progress_notify(move |_| f());
        {
            let (weak, tw) = (weak.clone(), tw.clone());
            v.connect_load_changed(move |view, ev| {
                let (Some(w), Some(t)) = (weak.upgrade(), tw.upgrade()) else { return };
                if ev == webkit6::LoadEvent::Committed
                    && let (Some(a), Some(u)) = (w.app(), view.uri())
                    && is_recordable(&u)
                {
                    a.store.borrow_mut().record_visit(&u, &t.page_title());
                }
                if w.is_current(&t) {
                    w.sync_chrome();
                }
            });
        }
        // Errors.
        {
            let weak = weak.clone();
            v.connect_load_failed(move |view, _, uri, err| {
                if err.matches(webkit6::NetworkError::Cancelled)
                    || err.matches(webkit6::PolicyError::FrameLoadInterruptedByPolicyChange)
                {
                    return false;
                }
                let Some(a) = weak.upgrade().and_then(|w| w.app()) else { return false };
                let host = host_of(uri).unwrap_or_else(|| uri.to_owned());
                let (headline, detail) = if err.matches(webkit6::NetworkError::Transport) {
                    (
                        format!("Can't reach {host}"),
                        format!("Check your connection, or try again later. ({})", err.message()),
                    )
                } else if err.matches(webkit6::NetworkError::UnknownProtocol) {
                    ("This address isn't supported".to_owned(), err.message().to_owned())
                } else {
                    (format!("Couldn't open {host}"), err.message().to_owned())
                };
                let html = pages::error_page(&a.style.theme(), ErrorKind::Load, &headline, &detail, uri);
                view.load_alternate_html(&html, uri, None);
                true
            });
        }
        {
            let weak = weak.clone();
            v.connect_load_failed_with_tls_errors(move |view, uri, _cert, flags| {
                let Some(a) = weak.upgrade().and_then(|w| w.app()) else { return false };
                let html = pages::error_page(
                    &a.style.theme(),
                    ErrorKind::Tls,
                    "Your connection isn't private",
                    &pages::tls_reason(flags),
                    uri,
                );
                view.load_alternate_html(&html, uri, None);
                true
            });
        }
        // New windows open as tabs next to their opener.
        {
            let weak = weak.clone();
            v.connect_create(move |view, _| {
                let w = weak.upgrade()?;
                let t = w.add_tab(None, Some(view), true)?;
                Some(t.view.clone().upcast())
            });
        }
        {
            let (weak, tw) = (weak.clone(), tw.clone());
            v.connect_close(move |_| {
                if let (Some(w), Some(t)) = (weak.upgrade(), tw.upgrade()) {
                    w.close_tab(&t);
                }
            });
        }
        // Middle/Ctrl-click opens a background tab; unsupported responses download;
        // other schemes (mailto:, …) go to the system — only on a user gesture.
        {
            let weak = weak.clone();
            v.connect_decide_policy(move |_, decision, kind| {
                let Some(w) = weak.upgrade() else { return false };
                match kind {
                    webkit6::PolicyDecisionType::NavigationAction => {
                        let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
                            return false;
                        };
                        let Some(action) = nav.navigation_action() else { return false };
                        let Some(uri) = action.request().and_then(|r| r.uri()).map(|u| u.to_string()) else {
                            return false;
                        };
                        let scheme = uri.split(':').next().unwrap_or("").to_ascii_lowercase();
                        let web = matches!(scheme.as_str(), "http" | "https" | "file" | "about" | "data" | "blob");
                        if !web {
                            decision.ignore();
                            if action.is_user_gesture() {
                                let launcher = gtk::UriLauncher::new(&uri);
                                launcher.launch(Some(&w.win), gio::Cancellable::NONE, |r| {
                                    if let Err(e) = r {
                                        tracing::info!("no handler: {e}");
                                    }
                                });
                            }
                            return true;
                        }
                        let ctrl = action.modifiers() & gdk::ModifierType::CONTROL_MASK.bits() != 0;
                        if (action.mouse_button() == 2 || ctrl) && action.is_user_gesture() {
                            decision.ignore();
                            w.add_tab(Some(&uri), None, false);
                            return true;
                        }
                        false
                    }
                    webkit6::PolicyDecisionType::Response => {
                        let Some(r) = decision.downcast_ref::<webkit6::ResponsePolicyDecision>() else { return false };
                        if !r.is_mime_type_supported() {
                            decision.download();
                            return true;
                        }
                        false
                    }
                    _ => false,
                }
            });
        }
        // Permission requests: always ask; deny on dismiss.
        {
            let weak = weak.clone();
            v.connect_permission_request(move |view, req| {
                let Some(w) = weak.upgrade() else {
                    req.deny();
                    return true;
                };
                let site = view.uri().and_then(|u| host_of(&u)).unwrap_or_else(|| "This site".into());
                let (glyph, what) = describe_permission(req);
                let req = req.clone();
                crate::ui::confirm(
                    w.win.upcast_ref(),
                    glyph,
                    &format!("Allow {site} to {what}?"),
                    "You can change your mind by reloading the page; m3 Browser asks again every time.",
                    ("Deny", "Allow"),
                    move |ok| if ok { req.allow() } else { req.deny() },
                );
                true
            });
        }
        // HTML5 fullscreen (videos): hide the chrome.
        {
            let wk = weak.clone();
            v.connect_enter_fullscreen(move |_| {
                if let Some(w) = wk.upgrade() {
                    w.video_fullscreen.set(true);
                    w.set_chrome_visible(false);
                    w.win.fullscreen();
                }
                false
            });
            let wk = weak.clone();
            v.connect_leave_fullscreen(move |_| {
                if let Some(w) = wk.upgrade() {
                    w.video_fullscreen.set(false);
                    w.set_chrome_visible(true);
                    w.win.unfullscreen();
                }
                false
            });
        }
        // Find results.
        if let Some(fc) = v.find_controller() {
            let (wk, twk) = (weak.clone(), tw.clone());
            fc.connect_counted_matches(move |_, n| {
                if let (Some(w), Some(t)) = (wk.upgrade(), twk.upgrade())
                    && w.is_current(&t)
                {
                    w.find_label.remove_css_class("error");
                    w.find_label.set_text(&match n {
                        0 => "No matches".to_owned(),
                        1 => "1 match".to_owned(),
                        n => format!("{n} matches"),
                    });
                }
            });
            let (wk, twk) = (weak.clone(), tw.clone());
            fc.connect_failed_to_find_text(move |_| {
                if let (Some(w), Some(t)) = (wk.upgrade(), twk.upgrade())
                    && w.is_current(&t)
                {
                    w.find_label.set_text("No matches");
                    w.find_label.add_css_class("error");
                }
            });
        }
    }

    fn is_current(&self, tab: &Rc<Tab>) -> bool {
        self.current().is_some_and(|c| Rc::ptr_eq(&c, tab))
    }

    fn set_chrome_visible(&self, visible: bool) {
        self.header.set_visible(visible);
        self.chrome.set_visible(visible);
    }

    fn update_tab_label(&self, tab: &Tab) {
        let t = tab.page_title();
        tab.title.set_text(&t);
        tab.button.set_tooltip_text(Some(&t));
        if self.current().is_some_and(|c| std::ptr::eq(Rc::as_ptr(&c), tab)) {
            self.win.set_title(Some(&format!("{t} — m3 Browser")));
        }
    }

    // ---- chrome state --------------------------------------------------------

    fn sync_chrome(&self) {
        let Some(t) = self.current() else { return };
        self.update_tab_label(&t);
        let blank = t.blank.get();
        self.back.set_sensitive(!blank && t.view.can_go_back());
        self.forward.set_sensitive(!blank && t.view.can_go_forward());
        self.reload.set_sensitive(!blank);
        if !focused(&self.address) {
            self.address.set_text(&display_url(&t.uri()));
        }
        self.sync_star();
        self.sync_loading();
    }

    fn sync_loading(&self) {
        let Some(t) = self.current() else { return };
        let loading = !t.blank.get() && t.view.is_loading();
        self.progress_revealer.set_reveal_child(loading);
        self.progress.set_waving(loading);
        if loading {
            self.progress.set_fraction(Some(t.view.estimated_load_progress()));
        }
        self.reload_icon.set_text(if loading { "close" } else { "refresh" });
        let tip = if loading { "Stop (Escape)" } else { "Reload (Ctrl+R)" };
        self.reload.set_tooltip_text(Some(tip));
    }

    fn sync_star(&self) {
        let Some(t) = self.current() else { return };
        let uri = t.uri();
        let on = self.app().is_some_and(|a| a.store.borrow().is_bookmarked(&uri));
        self.updating.set(true);
        self.star.set_sensitive(is_recordable(&uri));
        self.star.set_active(on);
        if let Some(l) = self.star.child().and_downcast::<gtk::Label>() {
            if on { l.add_css_class("filled") } else { l.remove_css_class("filled") }
        }
        self.updating.set(false);
    }

    pub fn focus_address(&self) {
        if let Some(t) = self.current()
            && t.blank.get()
        {
            t.ntp.search.grab_focus();
            return;
        }
        self.address.grab_focus();
        self.address.select_region(0, -1);
    }

    fn zoom(&self, dir: i32) {
        self.with_view(|v| {
            let cur = v.zoom_level();
            let next = match dir {
                0 => 1.0,
                d if d > 0 => ZOOM_STEPS.iter().copied().find(|z| *z > cur + 0.001).unwrap_or(3.0),
                _ => ZOOM_STEPS.iter().rev().copied().find(|z| *z < cur - 0.001).unwrap_or(0.3),
            };
            v.set_zoom_level(next);
        });
    }

    // ---- toolbar ---------------------------------------------------------------

    fn wire_toolbar(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.reload.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.with_view(|v| if v.is_loading() { v.stop_loading() } else { v.reload() });
            }
        });
        let weak = Rc::downgrade(self);
        self.star.connect_toggled(move |s| {
            let Some(w) = weak.upgrade() else { return };
            if w.updating.get() {
                return;
            }
            let (Some(a), Some(t)) = (w.app(), w.current()) else { return };
            let uri = t.uri();
            if s.is_active() {
                a.store.borrow_mut().add_bookmark(&uri, &t.page_title());
            } else {
                a.store.borrow_mut().remove_bookmark(&uri);
            }
            a.bookmarks_changed();
        });

        // Address field: navigate on Enter, suggestions while typing.
        let weak = Rc::downgrade(self);
        self.address.connect_activate(move |e| {
            let Some(w) = weak.upgrade() else { return };
            let chosen =
                w.suggest_list.selected_row().filter(|_| w.suggest.is_visible()).map(|r| r.widget_name().to_string());
            w.hide_suggestions();
            let url = match chosen {
                Some(u) if !u.is_empty() => Some(u),
                _ => w.app().and_then(|a| address::resolve(&e.text(), &a.engine())),
            };
            if let (Some(url), Some(t)) = (url, w.current()) {
                w.load_in(&t, &url);
            }
        });
        let weak = Rc::downgrade(self);
        self.address.connect_changed(move |e| {
            let Some(w) = weak.upgrade() else { return };
            if focused(e) {
                w.update_suggestions(&e.text());
            }
        });
        // A GtkEntry's focus lives on its inner text widget: track it with a
        // focus controller (which sees focus anywhere inside the entry).
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_enter(move |_| {
            let Some(w) = weak.upgrade() else { return };
            // Select everything once the click that focused us is done.
            let e = w.address.clone();
            glib::idle_add_local_once(move || e.select_region(0, -1));
        });
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            let Some(w) = weak.upgrade() else { return };
            w.hide_suggestions();
            if let Some(t) = w.current() {
                w.address.set_text(&display_url(&t.uri()));
            }
        });
        self.address.add_controller(focus);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(w) = weak.upgrade() else { return glib::Propagation::Proceed };
            if key == gdk::Key::Escape {
                w.hide_suggestions();
                if let Some(t) = w.current() {
                    w.address.set_text(&display_url(&t.uri()));
                    if !t.blank.get() {
                        t.view.grab_focus();
                    }
                }
                return glib::Propagation::Stop;
            }
            if !w.suggest.is_visible() {
                return glib::Propagation::Proceed;
            }
            let step = match key {
                gdk::Key::Down => 1,
                gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            let n = {
                let mut n = 0;
                while w.suggest_list.row_at_index(n).is_some() {
                    n += 1;
                }
                n
            };
            if n == 0 {
                return glib::Propagation::Proceed;
            }
            let cur = w.suggest_list.selected_row().map(|r| r.index()).unwrap_or(-1);
            let next = (cur + step).clamp(0, n - 1);
            w.suggest_list.select_row(w.suggest_list.row_at_index(next).as_ref());
            glib::Propagation::Stop
        });
        self.address.add_controller(keys);
        let weak = Rc::downgrade(self);
        self.suggest_list.connect_row_activated(move |_, row| {
            let Some(w) = weak.upgrade() else { return };
            let url = row.widget_name().to_string();
            w.hide_suggestions();
            if let Some(t) = w.current()
                && !url.is_empty()
            {
                w.load_in(&t, &url);
            }
        });
    }

    fn hide_suggestions(&self) {
        if self.suggest.is_visible() {
            self.suggest.popdown();
        }
    }

    fn update_suggestions(&self, text: &str) {
        let Some(app) = self.app() else { return };
        while let Some(c) = self.suggest_list.first_child() {
            self.suggest_list.remove(&c);
        }
        if text.trim().is_empty() {
            self.hide_suggestions();
            return;
        }
        let engine = app.engine();
        // First row: what Enter will do.
        if let Some(target) = address::resolve(text, &engine) {
            let searching = target.starts_with(address::engine_prefix(&engine)) && !text.contains("://");
            let label = if searching {
                let name = address::ENGINES.iter().find(|e| e.0 == engine).map(|e| e.1).unwrap_or("the web");
                format!("Search {name} for “{}”", text.trim())
            } else {
                format!("Go to {}", display_url(&target))
            };
            self.suggest_list.append(&suggestion_row(
                if searching { "search" } else { "arrow_outward" },
                &label,
                "",
                &target,
            ));
        }
        for (url, title, bookmark) in app.store.borrow().suggest(text, 7) {
            let t = if title.is_empty() { url.clone() } else { title };
            self.suggest_list.append(&suggestion_row(
                if bookmark { "star" } else { "history" },
                &t,
                &display_url(&url),
                &url,
            ));
        }
        self.suggest_list.select_row(self.suggest_list.row_at_index(0).as_ref());
        let width = self.address.width().max(320);
        self.suggest.set_size_request(width, -1);
        if !self.suggest.is_visible() {
            self.suggest.popup();
        }
    }

    // ---- find ------------------------------------------------------------------

    fn wire_find(self: &Rc<Self>, prev: &gtk::Button, next: &gtk::Button, close: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.find_entry.connect_search_changed(move |e| {
            let Some(w) = weak.upgrade() else { return };
            let text = e.text().to_string();
            w.with_view(|v| {
                let Some(fc) = v.find_controller() else { return };
                if text.is_empty() {
                    fc.search_finish();
                    w.find_label.set_text("");
                } else {
                    fc.count_matches(&text, find_options(), 1000);
                    fc.search(&text, find_options(), 1000);
                }
            });
        });
        let step = |weak: Weak<Self>, back: bool| {
            move || {
                if let Some(w) = weak.upgrade() {
                    w.with_view(|v| {
                        if let Some(fc) = v.find_controller() {
                            if back { fc.search_previous() } else { fc.search_next() }
                        }
                    });
                }
            }
        };
        let f = step(Rc::downgrade(self), false);
        self.find_entry.connect_activate(move |_| f());
        let f = step(Rc::downgrade(self), false);
        next.connect_clicked(move |_| f());
        let f = step(Rc::downgrade(self), true);
        prev.connect_clicked(move |_| f());
        let keys = gtk::EventControllerKey::new();
        let f = step(Rc::downgrade(self), true);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter if mods.contains(gdk::ModifierType::SHIFT_MASK) => f(),
                gdk::Key::Escape => {
                    if let Some(w) = weak.upgrade() {
                        w.close_find();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.find_entry.add_controller(keys);
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.close_find();
            }
        });
    }

    fn open_find(&self) {
        if self.current().is_none_or(|t| t.blank.get()) {
            return;
        }
        self.find_revealer.set_reveal_child(true);
        self.find_entry.grab_focus();
        self.find_entry.select_region(0, -1);
    }

    fn close_find(&self) {
        self.find_revealer.set_reveal_child(false);
        self.find_label.set_text("");
        self.with_view(|v| {
            if let Some(fc) = v.find_controller() {
                fc.search_finish();
            }
        });
        if let Some(t) = self.current() {
            t.view.grab_focus();
        }
    }

    /// Tabs for the library window to open into.
    pub fn open_in_new_tab(self: &Rc<Self>, url: &str) {
        self.add_tab(Some(url), None, true);
        self.present();
    }

    pub fn window(&self) -> &gtk::ApplicationWindow {
        &self.win
    }

    pub fn app_rc(&self) -> Option<Rc<App>> {
        self.app()
    }
}

fn suggestion_row(glyph: &str, title: &str, subtitle: &str, url: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_widget_name(url);
    // Clicking a suggestion must not take focus from the address field.
    row.set_focusable(false);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let g = icon(glyph);
    g.add_css_class("small");
    b.append(&g);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.add_css_class("m3-body-large");
    col.append(&t);
    if !subtitle.is_empty() && subtitle != title {
        let s = gtk::Label::new(Some(subtitle));
        s.set_xalign(0.0);
        s.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        s.add_css_class("m3-body-small");
        s.add_css_class("m3-secondary-text");
        col.append(&s);
    }
    b.append(&col);
    row.set_child(Some(&b));
    row
}

/// `(glyph, "to …" phrase)` for a permission request.
fn describe_permission(req: &webkit6::PermissionRequest) -> (&'static str, String) {
    if req.is::<webkit6::GeolocationPermissionRequest>() {
        return ("location_on", "know your location".into());
    }
    if req.is::<webkit6::NotificationPermissionRequest>() {
        return ("notifications", "show notifications".into());
    }
    if let Some(m) = req.downcast_ref::<webkit6::UserMediaPermissionRequest>() {
        let (audio, video) = (m.is_for_audio_device(), m.is_for_video_device());
        if webkit6::functions::user_media_permission_is_for_display_device(m) {
            return ("screen_share", "see your screen".into());
        }
        return match (audio, video) {
            (true, true) => ("videocam", "use your camera and microphone".into()),
            (false, true) => ("videocam", "use your camera".into()),
            _ => ("mic", "use your microphone".into()),
        };
    }
    if req.is::<webkit6::ClipboardPermissionRequest>() {
        return ("content_paste", "see text and images you copied".into());
    }
    if req.is::<webkit6::DeviceInfoPermissionRequest>() {
        return ("devices", "see which cameras and microphones you have".into());
    }
    if let Some(m) = req.downcast_ref::<webkit6::MediaKeySystemPermissionRequest>() {
        let name = webkit6::functions::media_key_system_permission_get_name(m).map(|n| n.to_string());
        return ("lock", format!("play protected content{}", name.map(|n| format!(" ({n})")).unwrap_or_default()));
    }
    if req.is::<webkit6::PointerLockPermissionRequest>() {
        return ("mouse", "hide and lock your pointer".into());
    }
    if let Some(d) = req.downcast_ref::<webkit6::WebsiteDataAccessPermissionRequest>() {
        let other = d.requesting_domain().map(|s| s.to_string()).unwrap_or_else(|| "another site".into());
        return ("cookie", format!("let {other} use its cookies here"));
    }
    ("shield", "use an additional feature".into())
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("New tab"), Some("win.new-tab"));
    s1.append(Some("New window"), Some("win.new-window"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some("Find in page…"), Some("win.find"));
    s2.append(Some("Zoom in"), Some("win.zoom-in"));
    s2.append(Some("Zoom out"), Some("win.zoom-out"));
    s2.append(Some("Actual size"), Some("win.zoom-reset"));
    s2.append(Some("Fullscreen"), Some("win.fullscreen"));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append(Some("Bookmarks and history"), Some("win.library"));
    s3.append(Some("Downloads"), Some("win.downloads"));
    let engines = gio::Menu::new();
    for (id, label, _) in address::ENGINES {
        engines.append(Some(label), Some(&format!("app.engine::{id}")));
    }
    s3.append_submenu(Some("Search engine"), &engines);
    menu.append_section(None, &s3);
    let s4 = gio::Menu::new();
    s4.append(Some("About m3 Browser"), Some("win.about"));
    menu.append_section(None, &s4);
    menu
}
