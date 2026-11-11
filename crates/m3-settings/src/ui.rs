//! Small building blocks for settings pages (cards, rows, controls).

use gtk::prelude::*;

pub fn icon(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("m3-icon");
    l.set_accessible_role(gtk::AccessibleRole::Img);
    l
}

/// A scrollable page with a large title; returns (page, content box).
pub fn page(title: &str, subtitle: &str) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.add_css_class("m3-settings-page");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("m3-display-small-emphasized");
    t.add_css_class("m3-page-title");
    t.set_xalign(0.0);
    content.append(&t);
    if !subtitle.is_empty() {
        let s = gtk::Label::new(Some(subtitle));
        s.add_css_class("m3-body-large");
        s.add_css_class("m3-secondary-text");
        s.set_xalign(0.0);
        s.set_wrap(true);
        content.append(&s);
    }
    let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
    clamp.set_halign(gtk::Align::Center);
    clamp.set_size_request(720, -1);
    clamp.append(&content);
    let sw = gtk::ScrolledWindow::new();
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_child(Some(&clamp));
    sw.set_vexpand(true);
    (sw, content)
}

/// A titled card; append rows to the returned list box.
pub fn card(parent: &gtk::Box, title: &str) -> gtk::Box {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 8);
    if !title.is_empty() {
        let t = gtk::Label::new(Some(title));
        t.add_css_class("m3-title-small-emphasized");
        t.add_css_class("m3-card-title");
        t.set_xalign(0.0);
        wrap.append(&t);
    }
    // A grouped list: each row is its own rounded segment.
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("m3-group");
    list.add_css_class("m3-settings-card");
    wrap.append(&list);
    parent.append(&wrap);
    list
}

/// A row: title (+ subtitle) on the left, `control` on the right.
pub fn row(
    list: &gtk::Box,
    title: &str,
    subtitle: &str,
    control: &(impl IsA<gtk::Widget> + IsA<gtk::Accessible>),
) -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    r.add_css_class("m3-settings-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    t.set_wrap(true);
    t.add_css_class("m3-body-large");
    t.add_css_class("m3-row-title");
    text.append(&t);
    if !subtitle.is_empty() {
        let s = gtk::Label::new(Some(subtitle));
        s.set_xalign(0.0);
        s.set_wrap(true);
        s.add_css_class("m3-body-medium");
        s.add_css_class("m3-secondary-text");
        text.append(&s);
    }
    r.append(&text);
    control.set_valign(gtk::Align::Center);
    control.update_property(&[gtk::accessible::Property::Label(title)]);
    r.append(control);
    list.append(&r);
    r
}

pub fn switch(list: &gtk::Box, title: &str, subtitle: &str, active: bool, f: impl Fn(bool) + 'static) -> gtk::Switch {
    let s = gtk::Switch::new();
    s.set_active(active);
    s.connect_active_notify(move |s| f(s.is_active()));
    row(list, title, subtitle, &s);
    s
}

/// Slider with its current value shown next to it.
pub fn slider(
    list: &gtk::Box,
    title: &str,
    subtitle: &str,
    (min, max, step): (f64, f64, f64),
    value: f64,
    fmt: fn(f64) -> String,
    f: impl Fn(f64) + 'static,
) -> gtk::Scale {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    s.set_value(value);
    s.set_draw_value(false);
    s.set_size_request(220, -1);
    let label = gtk::Label::new(Some(&fmt(value)));
    label.set_width_chars(6);
    label.add_css_class("m3-label-large");
    let l2 = label.clone();
    s.connect_value_changed(move |s| {
        l2.set_text(&fmt(s.value()));
        f(s.value());
    });
    b.append(&s);
    b.append(&label);
    row(list, title, subtitle, &b);
    s
}

pub fn dropdown(
    list: &gtk::Box,
    title: &str,
    subtitle: &str,
    items: &[&str],
    selected: usize,
    f: impl Fn(usize) + 'static,
) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_selected(selected.min(items.len().saturating_sub(1)) as u32);
    d.connect_selected_notify(move |d| f(d.selected() as usize));
    row(list, title, subtitle, &d);
    d
}

pub fn spin(
    list: &gtk::Box,
    title: &str,
    subtitle: &str,
    (min, max, step): (f64, f64, f64),
    value: f64,
    f: impl Fn(f64) + 'static,
) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, step);
    s.set_value(value);
    s.connect_value_changed(move |s| f(s.value()));
    row(list, title, subtitle, &s);
    s
}

/// Segmented button group; returns the buttons.
pub fn segmented(
    list: &gtk::Box,
    title: &str,
    labels: &[&str],
    selected: usize,
    f: impl Fn(usize) + 'static,
) -> Vec<gtk::ToggleButton> {
    // A connected button group.
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    b.add_css_class("m3-button-group");
    let f = std::rc::Rc::new(f);
    let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        let t = gtk::ToggleButton::with_label(l);
        if let Some(first) = buttons.first() {
            t.set_group(Some(first));
        }
        t.set_active(i == selected);
        let f = f.clone();
        t.connect_toggled(move |t| {
            if t.is_active() {
                f(i);
            }
        });
        b.append(&t);
        buttons.push(t);
    }
    row(list, title, "", &b);
    buttons
}

pub fn info(list: &gtk::Box, title: &str, value: &str) -> gtk::Label {
    let v = gtk::Label::new(Some(value));
    v.add_css_class("m3-secondary-text");
    v.set_selectable(true);
    v.set_wrap(true);
    // Without a width hint GTK wraps a label at its minimum width.
    v.set_max_width_chars(40);
    v.set_justify(gtk::Justification::Right);
    v.set_xalign(1.0);
    row(list, title, "", &v);
    v
}

pub fn note(parent: &gtk::Box, text: &str) {
    let l = gtk::Label::new(Some(text));
    l.set_wrap(true);
    l.set_xalign(0.0);
    l.add_css_class("m3-body-medium");
    l.add_css_class("m3-secondary-text");
    l.add_css_class("m3-note");
    parent.append(&l);
}

/// Duration choices for idle timeouts (seconds, 0 = never).
pub const TIMEOUTS: &[(u32, &str)] = &[
    (0, "Never"),
    (60, "1 minute"),
    (120, "2 minutes"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (900, "15 minutes"),
    (1800, "30 minutes"),
    (3600, "1 hour"),
];

pub fn timeout_index(v: u32) -> usize {
    TIMEOUTS
        .iter()
        .position(|(s, _)| *s == v)
        .unwrap_or_else(|| TIMEOUTS.iter().rposition(|(s, _)| *s <= v).unwrap_or(0))
}

pub fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

pub fn times(v: f64) -> String {
    format!("{v:.2}×")
}

pub fn plain(v: f64) -> String {
    format!("{v:.0}")
}
