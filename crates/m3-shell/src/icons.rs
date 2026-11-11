//! Material Symbols icons and application icons.

use gtk::{gio, prelude::*};

/// A Material Symbols Rounded glyph by ligature name (e.g. `"wifi"`).
pub fn symbol(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("m3-icon");
    l.set_accessible_role(gtk::AccessibleRole::Img);
    l
}

pub fn symbol_with(name: &str, classes: &[&str]) -> gtk::Label {
    let l = symbol(name);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// An icon-only button with an accessible label and tooltip.
pub fn icon_button(name: &str, label: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&symbol(name)));
    b.add_css_class("icon");
    b.set_tooltip_text(Some(label));
    b.update_property(&[gtk::accessible::Property::Label(label)]);
    b
}

/// Image for an application icon, falling back to a generic glyph.
pub fn app_icon(icon: Option<gio::Icon>, size: i32) -> gtk::Widget {
    match icon {
        Some(icon) => {
            let img = gtk::Image::from_gicon(&icon);
            img.set_pixel_size(size);
            img.upcast()
        }
        None => {
            let l = symbol("apps");
            l.add_css_class("filled");
            l.upcast()
        }
    }
}
