//! The overview: every workspace side by side with its windows, a search
//! field that filters windows, and drag-and-drop to move windows between
//! workspaces.
//!
//! Windows are shown as cards with the app icon and title; live window
//! thumbnails need a capture protocol the compositor does not offer yet.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_protocol::{Action, WindowInfo, WorkspaceInfo};

use crate::{
    apps::{self, App},
    icons, ipc,
};

pub struct Overview {
    window: gtk::Window,
    revealer: gtk::Revealer,
    search: gtk::SearchEntry,
    columns: gtk::Box,
    state: RefCell<(Vec<WorkspaceInfo>, Vec<WindowInfo>, Vec<App>)>,
}

impl Overview {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("m3-overview"));
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-transparent");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 24);
        root.add_css_class("m3-overview");
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search windows"));
        search.add_css_class("m3-search");
        search.set_halign(gtk::Align::Center);
        search.set_size_request(480, -1);
        root.append(&search);
        let columns = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        columns.set_halign(gtk::Align::Center);
        columns.set_homogeneous(true);
        columns.set_vexpand(true);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        scroller.set_child(Some(&columns));
        scroller.set_vexpand(true);
        root.append(&scroller);
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        revealer.set_transition_duration(200);
        revealer.set_child(Some(&root));
        window.set_child(Some(&revealer));

        let ov = Rc::new(Self {
            window,
            revealer,
            search,
            columns,
            state: RefCell::new((Vec::new(), Vec::new(), Vec::new())),
        });
        let me = Rc::downgrade(&ov);
        ov.search.connect_search_changed(move |_| {
            if let Some(o) = me.upgrade() {
                o.rebuild();
            }
        });
        let me = Rc::downgrade(&ov);
        ov.search.connect_activate(move |_| {
            // Enter focuses the first matching window.
            if let Some(o) = me.upgrade() {
                let q = o.search.text().to_lowercase();
                let first = o.state.borrow().1.iter().find(|w| matches(w, &q)).map(|w| w.id);
                if let Some(id) = first {
                    ipc::action(Action::FocusWindow { id });
                    o.hide();
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        let me = Rc::downgrade(&ov);
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Escape {
                if let Some(o) = me.upgrade() {
                    o.hide();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        ov.window.add_controller(keys);
        ov.search.set_key_capture_widget(Some(&ov.window));
        // Clicking empty space closes.
        let click = gtk::GestureClick::new();
        let me = Rc::downgrade(&ov);
        click.connect_released(move |g, _, _, _| {
            if let Some(o) = me.upgrade() {
                if g.widget().is_some_and(|w| w == o.columns.clone().upcast::<gtk::Widget>()) {
                    o.hide();
                }
            }
        });
        ov.columns.add_controller(click);
        ov
    }

    pub fn update(self: &Rc<Self>, ws: &[WorkspaceInfo], wins: &[WindowInfo], apps: &[App]) {
        *self.state.borrow_mut() = (ws.to_vec(), wins.to_vec(), apps.to_vec());
        if self.window.is_visible() {
            self.rebuild();
        }
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(c) = self.columns.first_child() {
            self.columns.remove(&c);
        }
        let q = self.search.text().to_lowercase();
        let (ws, wins, apps) = self.state.borrow().clone();
        for w in &ws {
            let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
            col.add_css_class("m3-overview-workspace");
            if w.active {
                col.add_css_class("active");
            }
            col.set_size_request(260, 320);
            let head = gtk::Button::new();
            head.add_css_class("m3-overview-head");
            let hl = gtk::Label::new(Some(&format!("Workspace {}", w.name)));
            hl.add_css_class("m3-title-medium-emphasized");
            head.set_child(Some(&hl));
            let (index, me) = (w.index, Rc::downgrade(self));
            head.connect_clicked(move |_| {
                ipc::action(Action::SwitchWorkspace { index });
                if let Some(o) = me.upgrade() {
                    o.hide();
                }
            });
            col.append(&head);
            let mut any = false;
            for win in wins.iter().filter(|x| x.workspace == w.index && matches(x, &q)) {
                any = true;
                col.append(&self.card(win, &apps));
            }
            if !any {
                let l = gtk::Label::new(Some(if q.is_empty() { "Empty" } else { "No matches" }));
                l.add_css_class("m3-secondary-text");
                l.set_vexpand(true);
                col.append(&l);
            }
            // Drop a window here to move it to this workspace.
            let drop = gtk::DropTarget::new(glib::Type::U64, gdk::DragAction::MOVE);
            let col2 = col.clone();
            drop.connect_enter(move |_, _, _| {
                col2.add_css_class("drop-target");
                gdk::DragAction::MOVE
            });
            let col2 = col.clone();
            drop.connect_leave(move |_| col2.remove_css_class("drop-target"));
            let index = w.index;
            drop.connect_drop(move |_, v, _, _| {
                if let Ok(id) = v.get::<u64>() {
                    ipc::action(Action::MoveWindowToWorkspace { id, index });
                    return true;
                }
                false
            });
            col.add_controller(drop);
            self.columns.append(&col);
        }
    }

    fn card(self: &Rc<Self>, w: &WindowInfo, apps: &[App]) -> gtk::Widget {
        let app = w.app_id.as_deref().and_then(|a| apps::app_for_window(apps, a));
        let b = gtk::Button::new();
        b.add_css_class("m3-overview-window");
        if w.focused {
            b.add_css_class("focused");
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.append(&icons::app_icon(app.as_ref().and_then(|a| a.icon.clone()), 40));
        let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let name = gtk::Label::new(Some(
            &app.as_ref().map(|a| a.name.clone()).or_else(|| w.app_id.clone()).unwrap_or_else(|| "Window".into()),
        ));
        name.set_xalign(0.0);
        name.add_css_class("m3-label-large");
        let title = gtk::Label::new(Some(w.title.as_deref().unwrap_or("")));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(24);
        title.add_css_class("m3-body-small");
        title.add_css_class("m3-secondary-text");
        col.append(&name);
        col.append(&title);
        if w.minimized {
            let m = gtk::Label::new(Some("Minimized"));
            m.set_xalign(0.0);
            m.add_css_class("m3-label-small");
            col.append(&m);
        }
        row.append(&col);
        b.set_child(Some(&row));
        b.set_tooltip_text(w.title.as_deref());
        let (id, me) = (w.id, Rc::downgrade(self));
        b.connect_clicked(move |_| {
            ipc::action(Action::FocusWindow { id });
            if let Some(o) = me.upgrade() {
                o.hide();
            }
        });
        // Middle click closes the window.
        let mid = gtk::GestureClick::builder().button(2).build();
        mid.connect_released(move |_, _, _, _| ipc::action(Action::CloseWindowById { id }));
        b.add_controller(mid);
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&id.to_value())));
        b.add_controller(drag);
        b.upcast()
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.window.is_visible() { self.hide() } else { self.show() }
    }

    pub fn show(self: &Rc<Self>) {
        self.search.set_text("");
        self.rebuild();
        if let Some(root) = self.revealer.child() {
            m3_widgets::animate(&root, "m3-enter");
        }
        self.window.present();
        self.revealer.set_reveal_child(true);
        self.search.grab_focus();
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
        let w = self.window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(210), move || w.set_visible(false));
    }
}

fn matches(w: &WindowInfo, q: &str) -> bool {
    q.is_empty()
        || w.title.as_deref().is_some_and(|t| t.to_lowercase().contains(q))
        || w.app_id.as_deref().is_some_and(|a| a.to_lowercase().contains(q))
}
