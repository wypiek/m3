//! Notification popups and the notification centre.
//!
//! The D-Bus server lives in m3-services; this module presents what it
//! receives. Popups respect urgency and Do Not Disturb (critical
//! notifications always show); history survives restarts.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_services::{CloseReason, Command, Notification, Services, notifications::Urgency};

use crate::icons;

const MAX_HISTORY: usize = 100;

type CountCallback = Box<dyn Fn(usize)>;

/// Handler for an action on one of the shell's own notifications.
type ActionHandler = Rc<dyn Fn(&str)>;

pub struct Notifications {
    services: Services,
    popup_window: gtk::Window,
    popups: gtk::Box,
    popup_widgets: RefCell<HashMap<u32, gtk::Widget>>,
    /// Action handlers for the shell's own notifications (screenshots,
    /// battery…); other notifications' actions go to their app over D-Bus.
    internal: RefCell<HashMap<u32, ActionHandler>>,
    center_window: gtk::Window,
    center_revealer: gtk::Revealer,
    center_list: gtk::Box,
    empty: gtk::Label,
    dnd_switch: gtk::Switch,
    history: RefCell<Vec<Notification>>,
    dnd: RefCell<bool>,
    on_count: RefCell<Option<CountCallback>>,
}

fn history_path() -> Option<PathBuf> {
    m3_core::paths::state_dir().map(|d| d.join("notifications.json"))
}

fn relative_time(ts: i64) -> String {
    let now = chrono::Utc::now().timestamp();
    let d = (now - ts).max(0);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => chrono::DateTime::from_timestamp(ts, 0)
            .map(|t| t.with_timezone(&chrono::Local).format("%-d %b").to_string())
            .unwrap_or_default(),
    }
}

fn notification_icon(n: &Notification, size: i32) -> gtk::Widget {
    if let Some(img) = &n.image {
        let format = if img.has_alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
        let bytes = glib::Bytes::from(&img.data);
        let tex = gdk::MemoryTexture::new(img.width, img.height, format, &bytes, img.rowstride as usize);
        let pic = gtk::Image::from_paintable(Some(&tex));
        pic.set_pixel_size(size);
        return pic.upcast();
    }
    let path = n
        .image_path
        .clone()
        .or_else(|| (n.app_icon.starts_with('/') || n.app_icon.starts_with("file://")).then(|| n.app_icon.clone()));
    if let Some(p) = path {
        let file = if p.starts_with("file://") { gio::File::for_uri(&p) } else { gio::File::for_path(&p) };
        let img = gtk::Image::from_gicon(&gio::FileIcon::new(&file));
        img.set_pixel_size(size);
        return img.upcast();
    }
    if !n.app_icon.is_empty() {
        return icons::app_icon(Some(gio::ThemedIcon::new(&n.app_icon).upcast()), size);
    }
    let icon = n
        .desktop_entry
        .as_ref()
        .and_then(|d| gio_unix::DesktopAppInfo::new(&format!("{d}.desktop")))
        .and_then(|a| gio::prelude::AppInfoExt::icon(&a));
    match icon {
        Some(i) => icons::app_icon(Some(i), size),
        // No icon: the bell on an expressive shape.
        None => {
            let s = m3_widgets::ShapeIcon::new(m3_widgets::Shape::Cookie(9), "notifications", size + 8, true);
            s.widget().add_css_class("tertiary");
            s.widget().clone().upcast()
        }
    }
}

impl Notifications {
    pub fn new(app: &gtk::Application, services: Services) -> Rc<Self> {
        // Popups: top-right, never steal keyboard focus.
        let popup_window = gtk::Window::builder().application(app).decorated(false).build();
        popup_window.init_layer_shell();
        popup_window.set_layer(Layer::Overlay);
        popup_window.set_namespace(Some("m3-notifications"));
        popup_window.set_anchor(Edge::Top, true);
        popup_window.set_anchor(Edge::Right, true);
        // Top right; layer-shell keeps it clear of a top panel (exclusive zone).
        popup_window.set_margin(Edge::Top, 12);
        popup_window.set_margin(Edge::Right, 12);
        popup_window.set_keyboard_mode(KeyboardMode::None);
        popup_window.add_css_class("m3-layer");
        popup_window.add_css_class("m3-transparent");
        let popups = gtk::Box::new(gtk::Orientation::Vertical, 8);
        // Never 0 px (layer-shell protocol error) even with no popups.
        popups.set_size_request(380, 1);
        popup_window.set_child(Some(&popups));

        // Centre.
        let center_window = gtk::Window::builder().application(app).decorated(false).build();
        center_window.init_layer_shell();
        center_window.set_layer(Layer::Top);
        center_window.set_namespace(Some("m3-notification-center"));
        center_window.set_keyboard_mode(KeyboardMode::OnDemand);
        center_window.add_css_class("m3-layer");
        center_window.add_css_class("m3-transparent");
        let sheet = gtk::Box::new(gtk::Orientation::Vertical, 12);
        sheet.add_css_class("m3-notification-center");
        sheet.set_size_request(400, -1);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Notifications"));
        title.add_css_class("m3-headline-small-emphasized");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        header.append(&title);
        let clear = gtk::Button::with_label("Clear all");
        clear.add_css_class("tonal");
        header.append(&clear);
        sheet.append(&header);
        let dnd_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        dnd_row.add_css_class("m3-status-row");
        dnd_row.append(&icons::symbol_with("do_not_disturb_on", &["small"]));
        let dl = gtk::Label::new(Some("Do Not Disturb"));
        dl.set_hexpand(true);
        dl.set_xalign(0.0);
        dnd_row.append(&dl);
        let dnd_switch = gtk::Switch::new();
        dnd_switch.set_valign(gtk::Align::Center);
        dnd_switch.update_property(&[gtk::accessible::Property::Label("Do Not Disturb")]);
        dnd_row.append(&dnd_switch);
        sheet.append(&dnd_row);
        let center_list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let empty = gtk::Label::new(Some("No notifications"));
        empty.add_css_class("m3-secondary-text");
        empty.set_margin_top(24);
        empty.set_margin_bottom(24);
        sheet.append(&empty);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_max_content_height(600);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&center_list));
        sheet.append(&scroller);
        let center_revealer = gtk::Revealer::new();
        center_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        center_revealer.set_child(Some(&sheet));
        center_revealer.set_size_request(400, 1);
        center_window.set_child(Some(&center_revealer));

        let history: Vec<Notification> = history_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();

        let n = Rc::new(Self {
            services,
            popup_window,
            popups,
            popup_widgets: RefCell::new(HashMap::new()),
            internal: RefCell::new(HashMap::new()),
            center_window,
            center_revealer,
            center_list,
            empty,
            dnd_switch,
            history: RefCell::new(history),
            dnd: RefCell::new(false),
            on_count: RefCell::new(None),
        });
        let me = Rc::downgrade(&n);
        clear.connect_clicked(move |_| {
            if let Some(n) = me.upgrade() {
                n.clear_all();
            }
        });
        let me = Rc::downgrade(&n);
        n.dnd_switch.connect_active_notify(move |s| {
            if let Some(n) = me.upgrade() {
                if *n.dnd.borrow() != s.is_active() {
                    n.services.send(Command::SetDoNotDisturb(s.is_active()));
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        let me = Rc::downgrade(&n);
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Escape {
                if let Some(n) = me.upgrade() {
                    n.hide_center();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        n.center_window.add_controller(keys);
        if let Some(sheet) = n.center_revealer.child() {
            let me = Rc::downgrade(&n);
            crate::placement::dismiss_on_outside_click(&n.center_window, &sheet, move || {
                if let Some(n) = me.upgrade() {
                    n.hide_center();
                }
            });
        }
        crate::placement::anchor_sheet(&n.center_window, &n.center_revealer, crate::placement::panel_position());
        n.rebuild_center();
        n
    }

    pub fn set_on_count(&self, f: Box<dyn Fn(usize)>) {
        f(self.history.borrow().len());
        *self.on_count.borrow_mut() = Some(f);
    }

    fn changed(&self) {
        if let Some(path) = history_path() {
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            // Transient notifications are not persisted.
            let history = self.history.borrow();
            let persist: Vec<&Notification> = history.iter().filter(|n| !n.transient).collect();
            if let Ok(bytes) = serde_json::to_vec(&persist) {
                let _ = m3_core::config::atomic_write(&path, &bytes);
            }
        }
        if let Some(f) = self.on_count.borrow().as_ref() {
            f(self.history.borrow().len());
        }
    }

    pub fn set_dnd(&self, on: bool) {
        *self.dnd.borrow_mut() = on;
        if self.dnd_switch.is_active() != on {
            self.dnd_switch.set_active(on);
        }
    }

    /// Show a notification from the shell itself; `on_action` gets the
    /// action key (`"default"` for a click on the card).
    pub fn add_internal(self: &Rc<Self>, n: Notification, on_action: impl Fn(&str) + 'static) {
        self.internal.borrow_mut().insert(n.id, Rc::new(on_action));
        self.add(n, true);
    }

    fn invoke(&self, id: u32, key: &str) {
        let handler = self.internal.borrow().get(&id).cloned();
        match handler {
            Some(h) => h(key),
            None => self.services.send(Command::InvokeAction { id, key: key.to_owned() }),
        }
    }

    pub fn add(self: &Rc<Self>, n: Notification, allow_popup: bool) {
        {
            let mut h = self.history.borrow_mut();
            h.retain(|x| x.id != n.id);
            h.insert(0, n.clone());
            h.truncate(MAX_HISTORY);
        }
        self.changed();
        self.rebuild_center();
        let show_popup = allow_popup && (!*self.dnd.borrow() || n.urgency == Urgency::Critical);
        if show_popup && !self.center_window.is_visible() {
            self.show_popup(&n);
        }
    }

    fn show_popup(self: &Rc<Self>, n: &Notification) {
        if let Some(old) = self.popup_widgets.borrow_mut().remove(&n.id) {
            self.popups.remove(&old);
        }
        let card = self.card(n, true);
        m3_widgets::animate(&card, "m3-enter");
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
        revealer.set_transition_duration(250);
        revealer.set_child(Some(&card));
        self.popups.prepend(&revealer);
        // Keep at most three popups on screen.
        while self.popup_widgets.borrow().len() >= 3 {
            let oldest = self.popups.last_child();
            match oldest {
                Some(w) => {
                    self.popups.remove(&w);
                    self.popup_widgets.borrow_mut().retain(|_, v| v != &w);
                }
                None => break,
            }
        }
        self.popup_widgets.borrow_mut().insert(n.id, revealer.clone().upcast());
        self.popup_window.present();
        revealer.set_reveal_child(true);

        let timeout = match (n.urgency, n.expire_timeout) {
            (Urgency::Critical, _) => None,
            (_, 0) => None,
            (_, t) if t > 0 => Some(t as u64),
            (Urgency::Low, _) => Some(4000),
            _ => Some(6000),
        };
        if let Some(ms) = timeout {
            let me = Rc::downgrade(self);
            let id = n.id;
            glib::timeout_add_local_once(Duration::from_millis(ms), move || {
                if let Some(s) = me.upgrade() {
                    s.dismiss_popup(id);
                    if s.history.borrow().iter().any(|x| x.id == id && x.transient) {
                        s.remove(id, CloseReason::Expired);
                    }
                }
            });
        }
    }

    fn dismiss_popup(&self, id: u32) {
        let w = self.popup_widgets.borrow_mut().remove(&id);
        if let Some(w) = w.and_then(|w| w.downcast::<gtk::Revealer>().ok()) {
            w.set_reveal_child(false);
            let popups = self.popups.clone();
            let win = self.popup_window.clone();
            glib::timeout_add_local_once(Duration::from_millis(260), move || {
                popups.remove(&w);
                if popups.first_child().is_none() {
                    win.set_visible(false);
                }
            });
        }
    }

    /// Remove from history (user dismissed, app closed it, or expired).
    pub fn remove(self: &Rc<Self>, id: u32, reason: CloseReason) {
        self.dismiss_popup(id);
        self.internal.borrow_mut().remove(&id);
        let existed = {
            let mut h = self.history.borrow_mut();
            let before = h.len();
            h.retain(|x| x.id != id);
            before != h.len()
        };
        if existed {
            self.services.send(Command::CloseNotification { id, reason });
            self.changed();
            self.rebuild_center();
        }
    }

    pub fn clear_all(self: &Rc<Self>) {
        let ids: Vec<u32> = self.history.borrow().iter().map(|n| n.id).collect();
        for id in ids {
            self.dismiss_popup(id);
            self.services.send(Command::CloseNotification { id, reason: CloseReason::Dismissed });
        }
        self.history.borrow_mut().clear();
        self.changed();
        self.rebuild_center();
    }

    fn card(self: &Rc<Self>, n: &Notification, popup: bool) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        card.add_css_class("m3-notification");
        if popup {
            card.add_css_class("popup");
        }
        if n.urgency == Urgency::Critical {
            card.add_css_class("critical");
        }
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        top.append(&notification_icon(n, 32));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        let meta = gtk::Label::new(Some(&format!(
            "{} · {}",
            if n.app_name.is_empty() { "Notification" } else { &n.app_name },
            relative_time(n.timestamp)
        )));
        meta.set_xalign(0.0);
        meta.add_css_class("m3-label-medium");
        meta.add_css_class("m3-secondary-text");
        text.append(&meta);
        let summary = gtk::Label::new(Some(&n.summary));
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        // Without a width cap a wrapping label asks for its whole text width
        // and the popup grows instead of wrapping.
        summary.set_max_width_chars(36);
        summary.add_css_class("m3-title-small-emphasized");
        text.append(&summary);
        if !n.body.is_empty() {
            let body = gtk::Label::new(Some(&strip_markup(&n.body)));
            body.set_xalign(0.0);
            body.set_wrap(true);
            body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            body.set_lines(if popup { 4 } else { 8 });
            body.set_ellipsize(gtk::pango::EllipsizeMode::End);
            body.set_max_width_chars(36);
            body.add_css_class("m3-body-medium");
            text.append(&body);
        }
        top.append(&text);
        let close = icons::icon_button("close", "Dismiss");
        close.set_valign(gtk::Align::Start);
        let me = Rc::downgrade(self);
        let id = n.id;
        close.connect_clicked(move |_| {
            if let Some(s) = me.upgrade() {
                s.remove(id, CloseReason::Dismissed);
            }
        });
        top.append(&close);
        card.append(&top);

        let visible_actions: Vec<&(String, String)> = n.actions.iter().filter(|(k, _)| k != "default").collect();
        if !visible_actions.is_empty() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_halign(gtk::Align::End);
            for (key, label) in visible_actions {
                let b = gtk::Button::with_label(label);
                b.add_css_class("tonal");
                let me = Rc::downgrade(self);
                let key = key.clone();
                let resident = n.resident;
                b.connect_clicked(move |_| {
                    if let Some(s) = me.upgrade() {
                        s.invoke(id, &key);
                        if !resident {
                            s.remove(id, CloseReason::Dismissed);
                        }
                    }
                });
                row.append(&b);
            }
            card.append(&row);
        }
        // Clicking the card runs the default action, if any.
        if n.actions.iter().any(|(k, _)| k == "default") {
            let click = gtk::GestureClick::new();
            let me = Rc::downgrade(self);
            click.connect_released(move |_, _, _, _| {
                if let Some(s) = me.upgrade() {
                    s.invoke(id, "default");
                    s.remove(id, CloseReason::Dismissed);
                }
            });
            card.add_controller(click);
        }
        card
    }

    fn rebuild_center(self: &Rc<Self>) {
        while let Some(c) = self.center_list.first_child() {
            self.center_list.remove(&c);
        }
        let history = self.history.borrow().clone();
        self.empty.set_visible(history.is_empty());
        // Group by application, most recent group first.
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<Notification>> = HashMap::new();
        for n in history {
            let key = if n.app_name.is_empty() { "Other".to_owned() } else { n.app_name.clone() };
            if !order.contains(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(n);
        }
        for key in order {
            let items = groups.remove(&key).unwrap_or_default();
            let group = gtk::Box::new(gtk::Orientation::Vertical, 6);
            group.add_css_class("m3-notification-group");
            if items.len() > 1 {
                let l = gtk::Label::new(Some(&format!("{key} · {}", items.len())));
                l.set_xalign(0.0);
                l.add_css_class("m3-label-large-emphasized");
                l.add_css_class("m3-group-label");
                group.append(&l);
            }
            // Cards of one app form a grouped list (2px gaps, inner corners small).
            let cards = gtk::Box::new(gtk::Orientation::Vertical, 2);
            cards.add_css_class("m3-group");
            for n in &items {
                cards.append(&self.card(n, false));
            }
            group.append(&cards);
            self.center_list.append(&group);
        }
    }

    pub fn toggle_center(self: &Rc<Self>) {
        if self.center_window.is_visible() { self.hide_center() } else { self.show_center() }
    }

    /// Hide on-screen popups (they stay in the history).
    pub fn hide_popups(&self) {
        let ids: Vec<u32> = self.popup_widgets.borrow().keys().copied().collect();
        for id in ids {
            self.dismiss_popup(id);
        }
    }

    pub fn show_center(self: &Rc<Self>) {
        // Opening the centre clears popups.
        self.hide_popups();
        self.rebuild_center();
        let pos = crate::placement::panel_position();
        crate::placement::anchor_sheet(&self.center_window, &self.center_revealer, pos);
        if let Some(sheet) = self.center_revealer.child() {
            sheet.remove_css_class("m3-enter");
            sheet.remove_css_class("m3-enter-up");
            m3_widgets::animate(&sheet, crate::placement::enter_class(pos));
        }
        self.center_window.set_keyboard_mode(KeyboardMode::Exclusive);
        self.center_window.present();
        self.center_revealer.set_reveal_child(true);
    }

    pub fn hide_center(&self) {
        if !self.center_window.is_visible() {
            return;
        }
        self.center_window.set_keyboard_mode(KeyboardMode::None);
        self.center_revealer.set_reveal_child(false);
        let w = self.center_window.clone();
        glib::timeout_add_local_once(Duration::from_millis(260), move || w.set_visible(false));
    }
}

/// Notification bodies may contain a small markup subset; show them as
/// plain text (hyperlink text kept, tags dropped).
fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    #[test]
    fn markup_is_stripped() {
        assert_eq!(super::strip_markup("<b>Hi</b> &amp; <a href=\"x\">link</a>"), "Hi & link");
    }
}
