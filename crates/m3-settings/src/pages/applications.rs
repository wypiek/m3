//! Applications: default apps and installed apps.

use std::rc::Rc;

use gtk::{gio, prelude::*};

use crate::{Ctx, ui};

const DEFAULTS: &[(&str, &[&str])] = &[
    ("Web browser", &["x-scheme-handler/http", "x-scheme-handler/https", "text/html"]),
    ("Email", &["x-scheme-handler/mailto"]),
    ("Files", &["inode/directory"]),
    ("Music", &["audio/mpeg", "audio/flac", "audio/ogg"]),
    ("Video", &["video/mp4", "video/x-matroska", "video/webm"]),
    ("Photos", &["image/png", "image/jpeg", "image/webp"]),
    ("Text editor", &["text/plain"]),
    ("Terminal", &["x-scheme-handler/terminal"]),
];

pub fn build(_ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Applications", "Default apps and everything installed.");
    let card = ui::card(&content, "Default apps");
    for (label, types) in DEFAULTS {
        let apps = gio::AppInfo::all_for_type(types[0]);
        if apps.is_empty() {
            continue;
        }
        let names: Vec<String> = apps.iter().map(|a| a.display_name().to_string()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let current = gio::AppInfo::default_for_type(types[0], false);
        let sel = current.and_then(|c| apps.iter().position(|a| a.equal(&c))).unwrap_or(0);
        let types: Vec<String> = types.iter().map(|t| (*t).to_owned()).collect();
        ui::dropdown(&card, label, "", &refs, sel, move |i| {
            for t in &types {
                if let Err(e) = apps[i].set_as_default_for_type(t) {
                    tracing::warn!("cannot set default for {t}: {e}");
                }
            }
        });
    }
    let card = ui::card(&content, "Installed apps");
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Filter"));
    card.append(&search);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    let mut all: Vec<gio::AppInfo> = gio::AppInfo::all().into_iter().filter(|a| a.should_show()).collect();
    all.sort_by_key(|a| a.display_name().to_lowercase());
    for a in &all {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        r.add_css_class("m3-settings-row");
        let img = match a.icon() {
            Some(i) => gtk::Image::from_gicon(&i),
            None => gtk::Image::from_icon_name("application-x-executable"),
        };
        img.set_pixel_size(32);
        r.append(&img);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col.set_hexpand(true);
        let n = gtk::Label::new(Some(&a.display_name()));
        n.set_xalign(0.0);
        col.append(&n);
        if let Some(d) = a.description() {
            let d = gtk::Label::new(Some(&d));
            d.set_xalign(0.0);
            d.set_ellipsize(gtk::pango::EllipsizeMode::End);
            d.add_css_class("m3-secondary-text");
            d.add_css_class("m3-body-small");
            col.append(&d);
        }
        r.append(&col);
        let open = gtk::Button::with_label("Open");
        open.add_css_class("text");
        let app = a.clone();
        open.connect_clicked(move |_| {
            let ctx = gtk::gdk::Display::default().map(|d| d.app_launch_context());
            let _ = app.launch(&[], ctx.as_ref());
        });
        r.append(&open);
        r.set_widget_name(&a.display_name().to_lowercase());
        list.append(&r);
    }
    {
        let l = list.clone();
        search.connect_search_changed(move |s| {
            let q = s.text().to_lowercase();
            l.set_filter_func(move |row| row.child().is_some_and(|c| c.widget_name().contains(q.as_str())));
        });
    }
    card.append(&list);
    page.upcast()
}
