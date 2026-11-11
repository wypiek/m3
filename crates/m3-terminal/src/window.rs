//! A terminal window: pill tab strip, terminals, search bar.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use m3_core::config::ThemeConfig;
use m3_theme::{Rgb, Theme};
use m3_widgets::AppStyle;
use vte4::prelude::*;

use crate::{links, palette, shell};

const MIN_ZOOM: f64 = 0.5;
const MAX_ZOOM: f64 = 3.0;

thread_local! {
    /// Open windows (each removes itself when destroyed).
    static WINDOWS: RefCell<Vec<Rc<TerminalWindow>>> = const { RefCell::new(Vec::new()) };
}

/// Keyboard shortcuts.
pub fn install_accels(app: &gtk::Application) {
    for (action, keys) in [
        ("win.copy", &["<Ctrl><Shift>c"][..]),
        ("win.paste", &["<Ctrl><Shift>v"]),
        ("win.new-tab", &["<Ctrl><Shift>t"]),
        ("win.close-tab", &["<Ctrl><Shift>w"]),
        ("app.new-window", &["<Ctrl><Shift>n"]),
        ("win.next-tab", &["<Ctrl>Page_Down"]),
        ("win.prev-tab", &["<Ctrl>Page_Up"]),
        ("win.zoom-in", &["<Ctrl>equal", "<Ctrl>plus", "<Ctrl>KP_Add"]),
        ("win.zoom-out", &["<Ctrl>minus", "<Ctrl>KP_Subtract"]),
        ("win.zoom-reset", &["<Ctrl>0", "<Ctrl>KP_0"]),
        ("win.find", &["<Ctrl><Shift>f"]),
    ] {
        app.set_accels_for_action(action, keys);
    }
}

fn icon(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("m3-icon");
    l.add_css_class("small");
    l.set_accessible_role(gtk::AccessibleRole::Img);
    l
}

fn icon_button(name: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&icon(name)));
    b.add_css_class("icon");
    b.set_tooltip_text(Some(tooltip));
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

fn rgba(c: Rgb) -> gdk::RGBA {
    gdk::RGBA::new(f32::from(c.0) / 255.0, f32::from(c.1) / 255.0, f32::from(c.2) / 255.0, 1.0)
}

/// Apply theme colours and font to one terminal.
fn style_terminal(term: &vte4::Terminal, cfg: &ThemeConfig, theme: &Theme) {
    let p = palette::palette(&theme.colors, theme.dark);
    let ansi: Vec<gdk::RGBA> = p.ansi.iter().map(|c| rgba(*c)).collect();
    let refs: Vec<&gdk::RGBA> = ansi.iter().collect();
    term.set_colors(Some(&rgba(p.foreground)), Some(&rgba(p.background)), &refs);
    term.set_color_cursor(Some(&rgba(p.cursor)));
    term.set_color_cursor_foreground(Some(&rgba(p.cursor_foreground)));
    term.set_color_highlight(Some(&rgba(p.selection)));
    term.set_color_highlight_foreground(Some(&rgba(p.selection_foreground)));
    let size = (11.0 * cfg.text_scale).round().max(6.0);
    let font = pango::FontDescription::from_string(&format!("{},monospace {size}", cfg.monospace_font_family));
    term.set_font(Some(&font));
}

type WindowAction = Box<dyn Fn(&Rc<TerminalWindow>)>;

struct Tab {
    term: vte4::Terminal,
    page: gtk::ScrolledWindow,
    pill: gtk::Box,
    label: gtk::Label,
    close: gtk::Button,
}

impl Tab {
    fn title(&self) -> String {
        self.term
            .window_title()
            .map(|t| t.to_string())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Terminal".into())
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.term.current_directory_uri().and_then(|u| shell::dir_from_uri(&u))
    }
}

/// Below this window width tabs are compact (short titles, close button
/// only on the selected tab).
const COMPACT_WIDTH: i32 = 620;

pub struct TerminalWindow {
    window: gtk::ApplicationWindow,
    style: Rc<AppStyle>,
    stack: gtk::Stack,
    strip: gtk::Box,
    strip_scroll: gtk::ScrolledWindow,
    compact: Cell<bool>,
    tabs: RefCell<Vec<Rc<Tab>>>,
    current: RefCell<Option<Rc<Tab>>>,
    search_revealer: gtk::Revealer,
    search_entry: gtk::SearchEntry,
    copy_action: gio::SimpleAction,
    closing: Cell<bool>,
}

impl TerminalWindow {
    /// Open a new window with one tab in `cwd` (default: home).
    pub fn open(app: &gtk::Application, style: Rc<AppStyle>, cwd: Option<PathBuf>) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Terminal")
            .default_width(980)
            .default_height(620)
            .build();
        window.add_css_class("m3");
        window.add_css_class("m3-terminal-window");
        window.set_size_request(360, 240);

        // Title bar: a horizontally scrolling strip of pill tabs with the
        // new-tab button after the last tab, then the window controls. The
        // strip takes the space that's there instead of widening the window.
        let strip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        strip.add_css_class("m3-tab-strip");
        strip.set_halign(gtk::Align::Start);
        let new_tab = icon_button("add", "New tab (Ctrl+Shift+T)");
        new_tab.add_css_class("m3-new-tab");
        new_tab.set_action_name(Some("win.new-tab"));
        new_tab.set_valign(gtk::Align::Center);
        // The strip asks for its natural width but can shrink to almost
        // nothing (then it scrolls), so it never widens the window and the
        // new-tab button right after it stays visible.
        let strip_scroll = gtk::ScrolledWindow::new();
        strip_scroll.add_css_class("m3-tab-scroll");
        strip_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        strip_scroll.set_child(Some(&strip));
        strip_scroll.set_propagate_natural_width(true);
        strip_scroll.set_min_content_width(48);
        // The mouse wheel scrolls the tabs sideways.
        let wheel = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::KINETIC,
        );
        {
            let sc = strip_scroll.clone();
            wheel.connect_scroll(move |_, dx, dy| {
                let adj = sc.hadjustment();
                let delta = if dx.abs() > dy.abs() { dx } else { dy };
                adj.set_value(adj.value() + delta * 48.0);
                glib::Propagation::Stop
            });
        }
        strip_scroll.add_controller(wheel);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("m3-terminal-header");
        bar.append(&strip_scroll);
        bar.append(&new_tab);
        bar.append(&spacer);
        bar.append(&gtk::WindowControls::new(gtk::PackType::End));
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&bar));
        window.set_titlebar(Some(&handle));

        // Find bar.
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Find in terminal"));
        search_entry.add_css_class("m3-search");
        search_entry.set_hexpand(true);
        let prev = icon_button("keyboard_arrow_up", "Previous match (Shift+Enter)");
        let next = icon_button("keyboard_arrow_down", "Next match (Enter)");
        let group = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        group.add_css_class("m3-button-group");
        group.append(&prev);
        group.append(&next);
        let close_search = icon_button("close", "Close (Escape)");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bar.add_css_class("m3-terminal-search");
        bar.append(&search_entry);
        bar.append(&group);
        bar.append(&close_search);
        let search_revealer = gtk::Revealer::new();
        search_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        search_revealer.set_child(Some(&bar));

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);
        stack.set_vexpand(true);
        stack.add_css_class("m3-terminal-surface");
        stack.set_overflow(gtk::Overflow::Hidden);
        // A zero-height probe reports the content width (compact tabs).
        let probe = gtk::DrawingArea::new();
        probe.set_hexpand(true);
        probe.set_content_height(0);
        probe.set_can_target(false);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&probe);
        root.append(&search_revealer);
        root.append(&stack);
        window.set_child(Some(&root));

        let copy_action = gio::SimpleAction::new("copy", None);
        let me = Rc::new(Self {
            window: window.clone(),
            style: style.clone(),
            stack,
            strip,
            strip_scroll,
            compact: Cell::new(false),
            tabs: RefCell::new(Vec::new()),
            current: RefCell::new(None),
            search_revealer,
            search_entry: search_entry.clone(),
            copy_action,
            closing: Cell::new(false),
        });
        me.install_actions();
        // Keep the selected tab in view whenever the strip's size changes
        // (new tab, closed tab, window resized).
        let w = Rc::downgrade(&me);
        me.strip_scroll.hadjustment().connect_changed(move |_| {
            if let Some(me) = w.upgrade() {
                if let Some(t) = me.current() {
                    me.reveal_tab(&t);
                }
            }
        });
        let w = Rc::downgrade(&me);
        probe.connect_resize(move |_, width, _| {
            if let Some(me) = w.upgrade() {
                me.set_compact(width < COMPACT_WIDTH);
            }
        });

        // Search wiring.
        let w = Rc::downgrade(&me);
        search_entry.connect_search_changed(move |e| {
            if let Some(me) = w.upgrade() {
                me.apply_search(&e.text());
                me.find(true);
            }
        });
        let w = Rc::downgrade(&me);
        search_entry.connect_activate(move |_| {
            if let Some(me) = w.upgrade() {
                me.find(true);
            }
        });
        let w = Rc::downgrade(&me);
        search_entry.connect_previous_match(move |_| {
            if let Some(me) = w.upgrade() {
                me.find(false);
            }
        });
        let w = Rc::downgrade(&me);
        search_entry.connect_next_match(move |_| {
            if let Some(me) = w.upgrade() {
                me.find(true);
            }
        });
        let w = Rc::downgrade(&me);
        search_entry.connect_stop_search(move |_| {
            if let Some(me) = w.upgrade() {
                me.close_search();
            }
        });
        // Shift+Enter searches backwards.
        let keys = gtk::EventControllerKey::new();
        let w = Rc::downgrade(&me);
        keys.connect_key_pressed(move |_, key, _, state| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && state.contains(gdk::ModifierType::SHIFT_MASK) {
                if let Some(me) = w.upgrade() {
                    me.find(false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        search_entry.add_controller(keys);
        let w = Rc::downgrade(&me);
        prev.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                me.find(false);
            }
        });
        let w = Rc::downgrade(&me);
        next.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                me.find(true);
            }
        });
        let w = Rc::downgrade(&me);
        close_search.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                me.close_search();
            }
        });

        // Live theme.
        let w = Rc::downgrade(&me);
        style.connect_changed(move |cfg, theme| {
            if let Some(me) = w.upgrade() {
                for t in me.tabs.borrow().iter() {
                    style_terminal(&t.term, cfg, theme);
                }
            }
        });

        let keep = me.clone();
        WINDOWS.with(|ws| ws.borrow_mut().push(keep));
        let win = window.clone();
        window.connect_destroy(move |_| {
            WINDOWS.with(|ws| ws.borrow_mut().retain(|w| w.window != win));
        });
        let w = Rc::downgrade(&me);
        window.connect_close_request(move |_| {
            if let Some(me) = w.upgrade() {
                me.closing.set(true);
            }
            glib::Propagation::Proceed
        });

        me.new_tab(cwd);
        window.present();
        me
    }

    fn current(&self) -> Option<Rc<Tab>> {
        self.current.borrow().clone()
    }

    fn install_actions(self: &Rc<Self>) {
        let add = |name: &str, f: WindowAction| {
            let a = gio::SimpleAction::new(name, None);
            let w: Weak<Self> = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(me) = w.upgrade() {
                    f(&me);
                }
            });
            self.window.add_action(&a);
        };
        let w = Rc::downgrade(self);
        self.copy_action.connect_activate(move |_, _| {
            if let Some(t) = w.upgrade().and_then(|me| me.current()) {
                t.term.copy_clipboard_format(vte4::Format::Text);
            }
        });
        self.copy_action.set_enabled(false);
        self.window.add_action(&self.copy_action);
        add(
            "paste",
            Box::new(|me| {
                if let Some(t) = me.current() {
                    t.term.paste_clipboard();
                }
            }),
        );
        add(
            "new-tab",
            Box::new(|me| {
                let cwd = me.current().and_then(|t| t.cwd());
                me.new_tab(cwd);
            }),
        );
        add(
            "close-tab",
            Box::new(|me| {
                if let Some(t) = me.current() {
                    me.close_tab(&t);
                }
            }),
        );
        add("next-tab", Box::new(|me| me.cycle(1)));
        add("prev-tab", Box::new(|me| me.cycle(-1)));
        add("zoom-in", Box::new(|me| me.zoom(Some(1.1))));
        add("zoom-out", Box::new(|me| me.zoom(Some(1.0 / 1.1))));
        add("zoom-reset", Box::new(|me| me.zoom(None)));
        add("find", Box::new(|me| me.open_search()));
    }

    fn new_tab(self: &Rc<Self>, cwd: Option<PathBuf>) {
        let term = vte4::Terminal::new();
        term.add_css_class("m3-terminal");
        term.set_hexpand(true);
        term.set_vexpand(true);
        term.set_scrollback_lines(10_000);
        term.set_cursor_blink_mode(vte4::CursorBlinkMode::On);
        term.set_mouse_autohide(true);
        term.set_allow_hyperlink(true);
        term.set_bold_is_bright(false);
        style_terminal(&term, &self.style.config(), &self.style.theme());
        match vte4::Regex::for_match(
            links::URL_REGEX,
            links::PCRE2_MULTILINE | links::PCRE2_UTF | links::PCRE2_NO_UTF_CHECK | links::PCRE2_UCP,
        ) {
            Ok(re) => {
                let tag = term.match_add_regex(&re, 0);
                term.match_set_cursor_name(tag, "pointer");
            }
            Err(e) => tracing::warn!("URL regex: {e}"),
        }

        let page = gtk::ScrolledWindow::new();
        page.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        page.set_child(Some(&term));
        page.add_css_class("m3-terminal-page");

        // Pill: title + close button.
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pill.add_css_class("m3-tab");
        let label = gtk::Label::new(Some("Terminal"));
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.set_max_width_chars(if self.compact.get() { 10 } else { 22 });
        label.set_width_chars(if self.compact.get() { 4 } else { 8 });
        let main = gtk::Button::new();
        main.add_css_class("m3-tab-main");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.append(&icon("terminal"));
        content.append(&label);
        main.set_child(Some(&content));
        let close = icon_button("close", "Close tab (Ctrl+Shift+W)");
        close.add_css_class("m3-tab-close");
        pill.append(&main);
        pill.append(&close);

        let tab =
            Rc::new(Tab { term: term.clone(), page: page.clone(), pill: pill.clone(), label, close: close.clone() });
        self.stack.add_child(&page);
        self.strip.append(&pill);
        m3_widgets::animate(&pill, "m3-enter");
        self.tabs.borrow_mut().push(tab.clone());

        let w = Rc::downgrade(self);
        let t = Rc::downgrade(&tab);
        main.connect_clicked(move |_| {
            if let (Some(me), Some(t)) = (w.upgrade(), t.upgrade()) {
                me.select(&t);
            }
        });
        let w = Rc::downgrade(self);
        let t = Rc::downgrade(&tab);
        close.connect_clicked(move |_| {
            if let (Some(me), Some(t)) = (w.upgrade(), t.upgrade()) {
                me.close_tab(&t);
            }
        });
        // Middle-click on a tab closes it.
        let middle = gtk::GestureClick::builder().button(2).build();
        let w = Rc::downgrade(self);
        let t = Rc::downgrade(&tab);
        middle.connect_released(move |_, _, _, _| {
            if let (Some(me), Some(t)) = (w.upgrade(), t.upgrade()) {
                me.close_tab(&t);
            }
        });
        pill.add_controller(middle);

        let w = Rc::downgrade(self);
        let t = Rc::downgrade(&tab);
        term.connect_window_title_changed(move |_| {
            if let (Some(me), Some(t)) = (w.upgrade(), t.upgrade()) {
                let title = t.title();
                t.label.set_text(&title);
                t.pill.set_tooltip_text(Some(&title));
                if me.current().is_some_and(|c| Rc::ptr_eq(&c, &t)) {
                    me.window.set_title(Some(&title));
                }
            }
        });
        let w = Rc::downgrade(self);
        let t = Rc::downgrade(&tab);
        term.connect_child_exited(move |_, _status| {
            if let (Some(me), Some(t)) = (w.upgrade(), t.upgrade()) {
                me.close_tab(&t);
            }
        });
        let w = Rc::downgrade(self);
        term.connect_selection_changed(move |term| {
            if let Some(me) = w.upgrade() {
                if me.current().is_some_and(|c| &c.term == term) {
                    me.copy_action.set_enabled(term.has_selection());
                }
            }
        });

        // Ctrl+click opens links.
        let click = gtk::GestureClick::builder().button(1).propagation_phase(gtk::PropagationPhase::Capture).build();
        let w = Rc::downgrade(self);
        click.connect_pressed(move |g, _, x, y| {
            if !g.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return;
            }
            let (Some(me), Some(term)) = (w.upgrade(), g.widget().and_downcast::<vte4::Terminal>()) else { return };
            let Some(uri) = links::link_at(&term, x, y) else { return };
            if !links::is_openable(&uri) {
                tracing::info!("not opening link with unsupported scheme");
                return;
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            gtk::UriLauncher::new(&uri).launch(Some(&me.window), gio::Cancellable::NONE, |r| {
                if let Err(e) = r {
                    tracing::warn!("cannot open link: {e}");
                }
            });
        });
        term.add_controller(click);

        // Context menu.
        let menu_click = gtk::GestureClick::builder().button(3).build();
        menu_click.connect_pressed(move |g, _, x, y| {
            let Some(term) = g.widget().and_downcast::<vte4::Terminal>() else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            let menu = gio::Menu::new();
            let edit = gio::Menu::new();
            edit.append(Some("Copy"), Some("win.copy"));
            edit.append(Some("Paste"), Some("win.paste"));
            menu.append_section(None, &edit);
            let more = gio::Menu::new();
            more.append(Some("New tab"), Some("win.new-tab"));
            more.append(Some("Find…"), Some("win.find"));
            menu.append_section(None, &more);
            let pop = gtk::PopoverMenu::from_model(Some(&menu));
            pop.set_has_arrow(false);
            pop.set_parent(&term);
            pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.connect_closed(|p| {
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            pop.popup();
        });
        term.add_controller(menu_click);

        self.spawn(&term, cwd);
        self.select(&tab);
    }

    fn spawn(&self, term: &vte4::Terminal, cwd: Option<PathBuf>) {
        let sh = shell::user_shell();
        let sh = sh.to_string_lossy().to_string();
        let dir = cwd.unwrap_or_else(glib::home_dir);
        let dir = dir.to_string_lossy().to_string();
        let t = term.clone();
        let shown = sh.clone();
        term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            Some(&dir),
            &[&sh],
            &["TERM=xterm-256color", "COLORTERM=truecolor", "TERM_PROGRAM=m3-terminal"],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gio::Cancellable::NONE,
            move |r| {
                if let Err(e) = r {
                    tracing::error!("cannot start {shown}: {e}");
                    t.feed(format!("m3-terminal: cannot start {shown}: {e}\r\n").as_bytes());
                }
            },
        );
    }

    fn select(&self, tab: &Rc<Tab>) {
        self.stack.set_visible_child(&tab.page);
        for t in self.tabs.borrow().iter() {
            if Rc::ptr_eq(t, tab) {
                t.pill.add_css_class("selected");
            } else {
                t.pill.remove_css_class("selected");
            }
        }
        self.window.set_title(Some(&tab.title()));
        self.copy_action.set_enabled(tab.term.has_selection());
        *self.current.borrow_mut() = Some(tab.clone());
        self.update_close_buttons();
        self.scroll_to(tab);
        if self.search_revealer.reveals_child() {
            self.apply_search(&self.search_entry.text());
        }
        tab.term.grab_focus();
    }

    /// Compact tabs on narrow windows.
    fn set_compact(&self, compact: bool) {
        if self.compact.replace(compact) == compact {
            return;
        }
        for t in self.tabs.borrow().iter() {
            t.label.set_max_width_chars(if compact { 10 } else { 22 });
            t.label.set_width_chars(if compact { 4 } else { 8 });
        }
        self.update_close_buttons();
        if let Some(t) = self.current() {
            self.scroll_to(&t);
        }
    }

    /// When compact, only the selected tab shows its close button.
    fn update_close_buttons(&self) {
        let compact = self.compact.get();
        let current = self.current();
        for t in self.tabs.borrow().iter() {
            let selected = current.as_ref().is_some_and(|c| Rc::ptr_eq(c, t));
            t.close.set_visible(!compact || selected);
        }
    }

    /// Bring a tab into view in the scrolling strip.
    fn reveal_tab(&self, tab: &Rc<Tab>) {
        reveal(&self.strip_scroll, &self.strip, &tab.pill);
    }

    /// Same, again once the strip has been laid out (tab switches).
    fn scroll_to(&self, tab: &Rc<Tab>) {
        self.reveal_tab(tab);
        let (sc, strip, pill) = (self.strip_scroll.clone(), self.strip.clone(), tab.pill.clone());
        glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || reveal(&sc, &strip, &pill));
    }

    fn close_tab(&self, tab: &Rc<Tab>) {
        let index = self.tabs.borrow().iter().position(|t| Rc::ptr_eq(t, tab));
        let Some(index) = index else { return };
        self.tabs.borrow_mut().remove(index);
        self.strip.remove(&tab.pill);
        self.stack.remove(&tab.page);
        let next = {
            let tabs = self.tabs.borrow();
            tabs.get(index.min(tabs.len().saturating_sub(1))).cloned()
        };
        match next {
            Some(n) => {
                if self.current().is_some_and(|c| Rc::ptr_eq(&c, tab)) {
                    self.select(&n);
                }
            }
            None => {
                *self.current.borrow_mut() = None;
                if !self.closing.get() {
                    self.window.close();
                }
            }
        }
    }

    fn cycle(&self, step: isize) {
        let tabs = self.tabs.borrow().clone();
        let Some(cur) = self.current() else { return };
        let Some(i) = tabs.iter().position(|t| Rc::ptr_eq(t, &cur)) else { return };
        let n = tabs.len() as isize;
        let j = (i as isize + step).rem_euclid(n) as usize;
        if let Some(t) = tabs.get(j) {
            self.select(t);
        }
    }

    fn zoom(&self, factor: Option<f64>) {
        if let Some(t) = self.current() {
            let s = factor.map(|f| (t.term.font_scale() * f).clamp(MIN_ZOOM, MAX_ZOOM)).unwrap_or(1.0);
            t.term.set_font_scale(s);
        }
    }

    fn open_search(&self) {
        self.search_revealer.set_reveal_child(true);
        self.search_entry.grab_focus();
        self.apply_search(&self.search_entry.text());
    }

    fn close_search(&self) {
        self.search_revealer.set_reveal_child(false);
        if let Some(t) = self.current() {
            t.term.search_set_regex(None, 0);
            t.term.grab_focus();
        }
    }

    /// Set the (literal, case-insensitive) search on the current terminal.
    fn apply_search(&self, text: &str) {
        let Some(t) = self.current() else { return };
        if text.is_empty() {
            t.term.search_set_regex(None, 0);
            self.search_entry.remove_css_class("error");
            return;
        }
        let flags = links::PCRE2_CASELESS | links::PCRE2_MULTILINE | links::PCRE2_UTF | links::PCRE2_NO_UTF_CHECK;
        match vte4::Regex::for_search(&links::escape_regex(text), flags) {
            Ok(re) => {
                t.term.search_set_regex(Some(&re), 0);
                t.term.search_set_wrap_around(true);
            }
            Err(e) => tracing::warn!("search regex: {e}"),
        }
    }

    fn find(&self, forward: bool) {
        let Some(t) = self.current() else { return };
        if self.search_entry.text().is_empty() {
            return;
        }
        let found = if forward { t.term.search_find_next() } else { t.term.search_find_previous() };
        if found {
            self.search_entry.remove_css_class("error");
        } else {
            self.search_entry.add_css_class("error");
        }
    }
}

/// Scroll `strip_scroll` so `pill` (a child of `strip`) is fully visible.
fn reveal(strip_scroll: &gtk::ScrolledWindow, strip: &gtk::Box, pill: &gtk::Box) {
    let Some(r) = pill.compute_bounds(strip) else { return };
    let adj = strip_scroll.hadjustment();
    let (x, w) = (f64::from(r.x()), f64::from(r.width()));
    let (v, page) = (adj.value(), adj.page_size());
    if page <= 0.0 {
        return;
    }
    if x < v {
        adj.set_value(x - 8.0);
    } else if x + w > v + page {
        adj.set_value(x + w - page + 8.0);
    }
}
