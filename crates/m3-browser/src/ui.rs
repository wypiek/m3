//! Small widget helpers and M3 dialogs.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use m3_widgets::{Shape, ShapeIcon};

/// A Material Symbols glyph by ligature name.
pub fn icon(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("m3-icon");
    l.set_accessible_role(gtk::AccessibleRole::Img);
    l
}

/// An icon-only button with tooltip and accessible label.
pub fn icon_button(name: &str, label: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&icon(name)));
    b.add_css_class("icon");
    b.set_tooltip_text(Some(label));
    b.update_property(&[gtk::accessible::Property::Label(label)]);
    b
}

type DoneSlot = Rc<RefCell<Option<Box<dyn FnOnce(bool)>>>>;

/// A modal M3 dialog: an expressive shape icon, a headline, body text and
/// two buttons. `done(true)` for the confirm button; `done(false)` for the
/// cancel button, Escape or closing the dialog. Called exactly once.
pub fn confirm(
    parent: &gtk::Window,
    glyph: &str,
    headline: &str,
    body: &str,
    (cancel_label, confirm_label): (&str, &str),
    done: impl FnOnce(bool) + 'static,
) {
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .decorated(false)
        .resizable(false)
        .default_width(400)
        .build();
    dialog.add_css_class("m3");
    dialog.add_css_class("m3-dialog-window");
    let done: DoneSlot = Rc::new(RefCell::new(Some(Box::new(done))));
    let finish = {
        let done = done.clone();
        let dialog = dialog.clone();
        move |ok: bool| {
            if let Some(f) = done.borrow_mut().take() {
                f(ok);
            }
            dialog.close();
        }
    };

    let col = gtk::Box::new(gtk::Orientation::Vertical, 16);
    col.add_css_class("m3-dialog");
    col.add_css_class("m3-enter");
    let shape = ShapeIcon::new(Shape::Cookie(9), glyph, 56, true);
    shape.widget().add_css_class("secondary");
    col.append(shape.widget());
    let h = gtk::Label::new(Some(headline));
    h.add_css_class("m3-headline-small-emphasized");
    h.set_wrap(true);
    h.set_justify(gtk::Justification::Center);
    h.set_max_width_chars(30);
    col.append(&h);
    let b = gtk::Label::new(Some(body));
    b.add_css_class("m3-body-medium");
    b.add_css_class("m3-secondary-text");
    b.set_wrap(true);
    b.set_justify(gtk::Justification::Center);
    b.set_max_width_chars(42);
    col.append(&b);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(8);
    let cancel = gtk::Button::with_label(cancel_label);
    cancel.add_css_class("tonal");
    let ok = gtk::Button::with_label(confirm_label);
    ok.add_css_class("filled");
    row.append(&cancel);
    row.append(&ok);
    col.append(&row);
    dialog.set_child(Some(&col));

    let f = finish.clone();
    cancel.connect_clicked(move |_| f(false));
    let f = finish.clone();
    ok.connect_clicked(move |_| f(true));
    let keys = gtk::EventControllerKey::new();
    let f = finish.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            f(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    dialog.add_controller(keys);
    dialog.connect_close_request(move |_| {
        if let Some(f) = done.borrow_mut().take() {
            f(false);
        }
        glib::Propagation::Proceed
    });
    // Deny is the safe default: it gets the initial focus.
    cancel.grab_focus();
    dialog.present();
}
