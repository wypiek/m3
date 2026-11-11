//! The application launcher.
//!
//! An overlay layer surface with exclusive keyboard focus: a search field,
//! category filter chips, recently used apps, the full app grid and matching
//! recent files. Typing filters immediately; arrow keys move through results;
//! Enter launches the selection; Escape or a click outside closes.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::{
    apps::{self, App, CATEGORIES, Usage},
    icons,
};

type LaunchCallback = Rc<dyn Fn(&str)>;

pub struct Launcher {
    window: gtk::Window,
    revealer: gtk::Revealer,
    search: gtk::SearchEntry,
    chips: gtk::Box,
    recent_label: gtk::Label,
    recent: gtk::FlowBox,
    history: gtk::Box,
    grid: gtk::FlowBox,
    files_label: gtk::Label,
    files: gtk::ListBox,
    empty: gtk::Label,
    apps: RefCell<Vec<App>>,
    usage: RefCell<Usage>,
    category: RefCell<Option<String>>,
    on_launch: RefCell<Option<LaunchCallback>>,
}

impl Launcher {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("m3-launcher"));
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-launcher-window");

        // Scrim: clicking it closes the launcher.
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("m3-launcher-scrim");

        let sheet = gtk::Box::new(gtk::Orientation::Vertical, 16);
        sheet.add_css_class("m3-launcher");
        sheet.set_halign(gtk::Align::Center);
        sheet.set_valign(gtk::Align::Center);
        sheet.set_size_request(760, -1);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search apps and files"));
        search.add_css_class("m3-search");
        search.update_property(&[gtk::accessible::Property::Label("Search applications")]);
        sheet.append(&search);

        let history = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        history.add_css_class("m3-history");
        sheet.append(&history);

        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        chips.add_css_class("m3-category-chips");
        sheet.append(&chips);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let recent_label = section_label("Recent");
        content.append(&recent_label);
        let recent = flow();
        recent.set_max_children_per_line(6);
        content.append(&recent);
        content.append(&section_label("All apps"));
        let grid = flow();
        content.append(&grid);
        let empty = gtk::Label::new(Some("No matching apps"));
        empty.add_css_class("m3-secondary-text");
        empty.add_css_class("m3-body-large");
        empty.set_margin_top(24);
        content.append(&empty);
        let files_label = section_label("Recent files");
        content.append(&files_label);
        let files = gtk::ListBox::new();
        files.add_css_class("m3-list");
        files.set_selection_mode(gtk::SelectionMode::None);
        content.append(&files);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_height(420);
        scroller.set_max_content_height(560);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&content));
        sheet.append(&scroller);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        revealer.set_transition_duration(300);
        revealer.set_child(Some(&sheet));
        revealer.set_halign(gtk::Align::Center);
        revealer.set_valign(gtk::Align::Center);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&scrim));
        overlay.add_overlay(&revealer);
        window.set_child(Some(&overlay));

        let launcher = Rc::new(Self {
            window,
            revealer,
            search,
            chips,
            recent_label,
            recent,
            history,
            grid,
            files_label,
            files,
            empty,
            apps: RefCell::new(Vec::new()),
            usage: RefCell::new(Usage::load()),
            category: RefCell::new(None),
            on_launch: RefCell::new(None),
        });
        launcher.connect_signals(&scrim);
        launcher
    }

    pub fn set_on_launch(&self, f: Rc<dyn Fn(&str)>) {
        *self.on_launch.borrow_mut() = Some(f);
    }

    fn connect_signals(self: &Rc<Self>, scrim: &gtk::Box) {
        let weak = Rc::downgrade(self);
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            if let Some(l) = weak.upgrade() {
                l.hide();
            }
        });
        scrim.add_controller(click);

        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(l) = weak.upgrade() {
                l.refilter();
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_activate(move |_| {
            if let Some(l) = weak.upgrade() {
                l.launch_first();
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_stop_search(move |_| {
            if let Some(l) = weak.upgrade() {
                l.hide();
            }
        });
        // Down arrow moves from the search field into the results.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(l) = weak.upgrade() else { return glib::Propagation::Proceed };
            match key {
                gdk::Key::Escape => {
                    l.hide();
                    glib::Propagation::Stop
                }
                gdk::Key::Down if l.search.has_focus() => {
                    if let Some(c) = l.grid.child_at_index(0).filter(|c| c.is_visible()) {
                        c.grab_focus();
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(keys);
        // Typing anywhere goes to the search field.
        self.search.set_key_capture_widget(Some(&self.window));

        for flow in [&self.grid, &self.recent] {
            let weak = Rc::downgrade(self);
            flow.connect_child_activated(move |_, child| {
                if let Some(l) = weak.upgrade() {
                    let id = child.widget_name();
                    l.launch_id(id.as_str());
                }
            });
        }
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.is_visible() { self.hide() } else { self.show() }
    }

    pub fn show(self: &Rc<Self>) {
        *self.apps.borrow_mut() = apps::all_apps();
        *self.category.borrow_mut() = None;
        self.search.set_text("");
        self.rebuild();
        if let Some(sheet) = self.revealer.child() {
            m3_widgets::animate(&sheet, "m3-enter-up");
        }
        self.window.present();
        self.revealer.set_reveal_child(true);
        self.search.grab_focus();
    }

    pub fn hide(&self) {
        if !self.window.is_visible() {
            return;
        }
        self.revealer.set_reveal_child(false);
        let window = self.window.clone();
        // Let the exit transition play before unmapping.
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(self.revealer.transition_duration() as u64),
            move || {
                window.set_visible(false);
            },
        );
    }

    fn rebuild(self: &Rc<Self>) {
        // Category chips (only categories that have apps).
        clear(&self.chips);
        let apps = self.apps.borrow();
        let add_chip = |id: Option<&str>, label: &str| {
            let chip = gtk::ToggleButton::with_label(label);
            chip.add_css_class("m3-chip");
            chip.set_active(self.category.borrow().as_deref() == id);
            let id = id.map(str::to_owned);
            let weak = Rc::downgrade(self);
            chip.connect_clicked(move |_| {
                if let Some(l) = weak.upgrade() {
                    *l.category.borrow_mut() = id.clone();
                    l.rebuild();
                }
            });
            self.chips.append(&chip);
        };
        add_chip(None, "All");
        for (id, label, _) in CATEGORIES {
            if apps.iter().any(|a| a.in_category(id)) {
                add_chip(Some(id), label);
            }
        }

        clear_flow(&self.grid);
        for a in apps.iter() {
            self.grid.append(&app_tile(a, 56));
        }
        clear_flow(&self.recent);
        let usage = self.usage.borrow();
        for id in usage.recent.iter().take(6) {
            if let Some(a) = apps.iter().find(|a| &a.id == id) {
                self.recent.append(&app_tile(a, 48));
            }
        }
        drop(usage);
        drop(apps);
        self.rebuild_history();
        self.refilter();
    }

    fn rebuild_history(self: &Rc<Self>) {
        clear(&self.history);
        for q in self.usage.borrow().searches.iter().take(5) {
            let chip = gtk::Button::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(&icons::symbol_with("history", &["small"]));
            row.append(&gtk::Label::new(Some(q)));
            chip.set_child(Some(&row));
            chip.add_css_class("m3-chip");
            chip.add_css_class("assist");
            let q = q.clone();
            let weak = Rc::downgrade(self);
            chip.connect_clicked(move |_| {
                if let Some(l) = weak.upgrade() {
                    l.search.set_text(&q);
                    l.search.set_position(-1);
                }
            });
            self.history.append(&chip);
        }
    }

    fn refilter(self: &Rc<Self>) {
        let query = self.search.text().trim().to_lowercase();
        let searching = !query.is_empty();
        let category = self.category.borrow().clone();
        let apps = self.apps.borrow();
        let usage = self.usage.borrow();

        // Score and order the grid: best matches (and most used) first.
        let mut visible = 0;
        let mut i = 0;
        while let Some(child) = self.grid.child_at_index(i) {
            let id = child.widget_name();
            let app = apps.iter().find(|a| a.id == id.as_str());
            let score = app.map(|a| a.score(&query)).unwrap_or(0);
            let in_cat = category.as_deref().is_none_or(|c| app.is_some_and(|a| a.in_category(c)));
            let show = score > 0 && in_cat;
            child.set_visible(show);
            if show {
                visible += 1;
            }
            i += 1;
        }
        let launches = usage.launches.clone();
        let names: std::collections::HashMap<String, (u32, String)> =
            apps.iter().map(|a| (a.id.clone(), (a.score(&query), a.name.to_lowercase()))).collect();
        self.grid.set_sort_func(move |a, b| {
            let (sa, na) = names.get(a.widget_name().as_str()).cloned().unwrap_or_default();
            let (sb, nb) = names.get(b.widget_name().as_str()).cloned().unwrap_or_default();
            let ua = launches.get(a.widget_name().as_str()).copied().unwrap_or(0);
            let ub = launches.get(b.widget_name().as_str()).copied().unwrap_or(0);
            // Relevance, then how often it's used, then alphabetical.
            sb.cmp(&sa).then(ub.cmp(&ua)).then(na.cmp(&nb)).into()
        });

        let show_recent = !searching && category.is_none() && self.recent.child_at_index(0).is_some();
        self.recent.set_visible(show_recent);
        self.recent_label.set_visible(show_recent);
        self.history.set_visible(!searching && self.history.first_child().is_some());
        self.empty.set_visible(visible == 0);

        // Recent files that match (GTK's recently-used list).
        clear_list(&self.files);
        let mut files = 0;
        if searching {
            for info in gtk::RecentManager::default().items() {
                if files >= 6 {
                    break;
                }
                let name = info.display_name().to_string();
                if !name.to_lowercase().contains(&query) || !info.exists() {
                    continue;
                }
                files += 1;
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                row.append(&icons::app_icon(
                    Some(info.gicon().unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast())),
                    24,
                ));
                let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
                let l = gtk::Label::new(Some(&name));
                l.set_xalign(0.0);
                col.append(&l);
                let uri = info.uri().to_string();
                let p = gtk::Label::new(Some(&gio::File::for_uri(&uri).parse_name()));
                p.set_xalign(0.0);
                p.add_css_class("m3-body-small");
                p.add_css_class("m3-secondary-text");
                p.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                col.append(&p);
                row.append(&col);
                let button = gtk::Button::new();
                button.set_child(Some(&row));
                button.add_css_class("flat");
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    let ctx = gdk::Display::default().map(|d| d.app_launch_context());
                    if let Err(e) = gio::AppInfo::launch_default_for_uri(&uri, ctx.as_ref()) {
                        tracing::warn!("cannot open {uri}: {e}");
                    }
                    if let Some(l) = weak.upgrade() {
                        l.hide();
                    }
                });
                self.files.append(&button);
            }
        }
        self.files_label.set_visible(files > 0);
        self.files.set_visible(files > 0);
        drop(usage);
        drop(apps);
        self.grid.invalidate_sort();
    }

    fn first_visible(&self) -> Option<String> {
        // FlowBox sorts lazily; pick the best-scoring visible child directly.
        let query = self.search.text().trim().to_lowercase();
        let category = self.category.borrow().clone();
        let apps = self.apps.borrow();
        let usage = self.usage.borrow();
        apps.iter()
            .filter(|a| a.score(&query) > 0 && category.as_deref().is_none_or(|c| a.in_category(c)))
            .max_by_key(|a| (a.score(&query), usage.launches.get(&a.id).copied().unwrap_or(0)))
            .map(|a| a.id.clone())
    }

    fn launch_first(self: &Rc<Self>) {
        if let Some(id) = self.first_visible() {
            self.launch_id(&id);
        }
    }

    fn launch_id(self: &Rc<Self>, id: &str) {
        let app = self.apps.borrow().iter().find(|a| a.id == id).cloned();
        let Some(app) = app else { return };
        app.launch();
        {
            let mut u = self.usage.borrow_mut();
            u.record_search(&self.search.text());
            u.record_launch(id);
        }
        if let Some(f) = self.on_launch.borrow().clone() {
            f(id);
        }
        self.hide();
    }

    /// Record a launch that happened elsewhere (panel).
    pub fn note_launch(&self, id: &str) {
        self.usage.borrow_mut().record_launch(id);
    }
}

fn section_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("m3-title-small-emphasized");
    l.add_css_class("m3-section-label");
    l
}

fn flow() -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_homogeneous(true);
    f.set_max_children_per_line(6);
    f.set_min_children_per_line(4);
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_activate_on_single_click(true);
    f.set_row_spacing(4);
    f.set_column_spacing(4);
    f
}

fn app_tile(a: &App, size: i32) -> gtk::FlowBoxChild {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.add_css_class("m3-app-tile");
    b.append(&icons::app_icon(a.icon.clone(), size));
    let l = gtk::Label::new(Some(&a.name));
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(12);
    l.set_justify(gtk::Justification::Center);
    l.add_css_class("m3-body-small");
    b.append(&l);
    let child = gtk::FlowBoxChild::new();
    child.set_child(Some(&b));
    child.set_widget_name(&a.id);
    let tip = if a.description.is_empty() { a.name.clone() } else { format!("{}\n{}", a.name, a.description) };
    child.set_tooltip_text(Some(&tip));
    child.update_property(&[
        gtk::accessible::Property::Label(&a.name),
        gtk::accessible::Property::Description(&a.description),
    ]);
    child
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn clear_flow(f: &gtk::FlowBox) {
    f.remove_all();
}

fn clear_list(l: &gtk::ListBox) {
    l.remove_all();
}
