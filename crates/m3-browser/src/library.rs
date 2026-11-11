//! The bookmarks & history window and the About dialog.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use m3_widgets::{Shape, ShapeIcon};

use crate::address::display_url;
use crate::ui::{icon, icon_button};
use crate::window::Window;

/// A slot holding the rebuild closure so its rows can call it again.
type SelfRef = Rc<std::cell::RefCell<Option<Rc<dyn Fn()>>>>;

fn relative(ts: u64) -> String {
    let d = crate::store::now().saturating_sub(ts);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

fn row(glyph: &str, title: &str, url: &str, meta: &str) -> (gtk::Box, gtk::Button, gtk::Button) {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    r.add_css_class("m3-library-row");
    let open = gtk::Button::new();
    open.add_css_class("m3-library-open");
    open.set_hexpand(true);
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    let g = icon(glyph);
    g.add_css_class("m3-library-glyph");
    inner.append(&g);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(Some(if title.is_empty() { url } else { title }));
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.add_css_class("m3-body-large");
    let u = gtk::Label::new(Some(&if meta.is_empty() {
        display_url(url)
    } else {
        format!("{} · {meta}", display_url(url))
    }));
    u.set_xalign(0.0);
    u.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    u.add_css_class("m3-body-small");
    u.add_css_class("m3-secondary-text");
    col.append(&t);
    col.append(&u);
    inner.append(&col);
    open.set_child(Some(&inner));
    open.set_tooltip_text(Some(url));
    let remove = icon_button("delete", "Remove");
    remove.set_valign(gtk::Align::Center);
    r.append(&open);
    r.append(&remove);
    (r, open, remove)
}

/// Show bookmarks and history; clicking an entry opens it in `w`.
pub fn show(w: &Rc<Window>) {
    let Some(app) = w.app_rc() else { return };
    let dialog = gtk::Window::builder()
        .transient_for(w.window())
        .title("Bookmarks and history")
        .default_width(640)
        .default_height(620)
        .build();
    dialog.add_css_class("m3");
    dialog.add_css_class("m3-library-window");
    let header = gtk::HeaderBar::new();
    let switch = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    switch.add_css_class("m3-button-group");
    let b_bm = gtk::ToggleButton::with_label("Bookmarks");
    let b_hist = gtk::ToggleButton::with_label("History");
    b_hist.set_group(Some(&b_bm));
    switch.append(&b_bm);
    switch.append(&b_hist);
    header.set_title_widget(Some(&switch));
    dialog.set_titlebar(Some(&header));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    let bm_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    bm_list.add_css_class("m3-group");
    let hist_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    hist_list.add_css_class("m3-group");
    let clear = gtk::Button::with_label("Clear history");
    clear.add_css_class("tonal");
    clear.set_halign(gtk::Align::End);

    let page = |list: &gtk::Box, extra: Option<&gtk::Button>| {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
        col.add_css_class("m3-library-page");
        if let Some(e) = extra {
            col.append(e);
        }
        col.append(list);
        let sw = gtk::ScrolledWindow::new();
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sw.set_vexpand(true);
        sw.set_child(Some(&col));
        sw
    };
    stack.add_named(&page(&bm_list, None), Some("bookmarks"));
    stack.add_named(&page(&hist_list, Some(&clear)), Some("history"));
    dialog.set_child(Some(&stack));

    let rebuild: Rc<dyn Fn()> = {
        let (app, w, bm_list, hist_list, dlg) =
            (app.clone(), Rc::downgrade(w), bm_list.clone(), hist_list.clone(), dialog.clone());
        let me: SelfRef = Rc::default();
        let me2 = me.clone();
        let f: Rc<dyn Fn()> = Rc::new(move || {
            for l in [&bm_list, &hist_list] {
                while let Some(c) = l.first_child() {
                    l.remove(&c);
                }
            }
            let store = app.store.borrow();
            let empty = |l: &gtk::Box, text: &str| {
                let e = gtk::Label::new(Some(text));
                e.add_css_class("m3-secondary-text");
                e.set_margin_top(32);
                l.append(&e);
            };
            if store.bookmarks.is_empty() {
                empty(&bm_list, "No bookmarks yet — use the star in the toolbar.");
            }
            for b in store.bookmarks.iter().rev() {
                let (r, open, remove) = row("star", &b.title, &b.url, "");
                let (w, url, d) = (w.clone(), b.url.clone(), dlg.clone());
                open.connect_clicked(move |_| {
                    if let Some(w) = w.upgrade() {
                        w.open_in_new_tab(&url);
                        d.close();
                    }
                });
                let (a, url, me) = (app.clone(), b.url.clone(), me2.clone());
                remove.connect_clicked(move |_| {
                    a.store.borrow_mut().remove_bookmark(&url);
                    a.bookmarks_changed();
                    let f = me.borrow().clone();
                    if let Some(f) = f {
                        glib::idle_add_local_once(move || f());
                    }
                });
                bm_list.append(&r);
            }
            if store.history.is_empty() {
                empty(&hist_list, "Pages you visit appear here.");
            }
            for v in store.history.iter().take(300) {
                let meta = format!("{} · {} visit{}", relative(v.last), v.visits, if v.visits == 1 { "" } else { "s" });
                let (r, open, remove) = row("history", &v.title, &v.url, &meta);
                let (w, url, d) = (w.clone(), v.url.clone(), dlg.clone());
                open.connect_clicked(move |_| {
                    if let Some(w) = w.upgrade() {
                        w.open_in_new_tab(&url);
                        d.close();
                    }
                });
                let (a, url, me) = (app.clone(), v.url.clone(), me2.clone());
                remove.connect_clicked(move |_| {
                    a.store.borrow_mut().remove_visit(&url);
                    let f = me.borrow().clone();
                    if let Some(f) = f {
                        glib::idle_add_local_once(move || f());
                    }
                });
                hist_list.append(&r);
            }
        });
        *me.borrow_mut() = Some(f.clone());
        // Break the self-reference when the window goes away.
        dialog.connect_destroy(move |_| {
            me.borrow_mut().take();
        });
        f
    };
    rebuild();

    {
        let (app, rebuild, dialog2) = (app.clone(), rebuild.clone(), dialog.clone());
        clear.connect_clicked(move |_| {
            let (app, rebuild) = (app.clone(), rebuild.clone());
            crate::ui::confirm(
                dialog2.upcast_ref(),
                "history",
                "Clear all history?",
                "Every page in your history is removed. Bookmarks, cookies and downloads are kept.",
                ("Cancel", "Clear"),
                move |ok| {
                    if ok {
                        app.store.borrow_mut().clear_history();
                        rebuild();
                    }
                },
            );
        });
    }
    let s = stack.clone();
    b_bm.connect_toggled(move |b| {
        if b.is_active() {
            s.set_visible_child_name("bookmarks");
        }
    });
    let s = stack.clone();
    b_hist.connect_toggled(move |b| {
        if b.is_active() {
            s.set_visible_child_name("history");
        }
    });
    b_bm.set_active(true);
    dialog.present();
}

/// The About dialog.
pub fn about(parent: &gtk::Window) {
    let dialog =
        gtk::Window::builder().transient_for(parent).modal(true).title("About m3 Browser").resizable(false).build();
    dialog.add_css_class("m3");
    dialog.add_css_class("m3-dialog-window");
    let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
    col.add_css_class("m3-dialog");
    col.add_css_class("m3-enter");
    let logo = ShapeIcon::new(Shape::Sunny(8), "language", 96, true);
    logo.widget().add_css_class("primary");
    m3_widgets::morph_on_interaction(logo.widget(), logo.shape(), Shape::Sunny(8), Shape::Cookie(9), Shape::Clover(4));
    col.append(logo.widget());
    let name = gtk::Label::new(Some("m3 Browser"));
    name.add_css_class("m3-headline-medium-emphasized");
    col.append(&name);
    let version = gtk::Label::new(Some(&format!(
        "Version {} · WebKitGTK {}.{}.{}",
        env!("CARGO_PKG_VERSION"),
        webkit6::functions::major_version(),
        webkit6::functions::minor_version(),
        webkit6::functions::micro_version()
    )));
    version.add_css_class("m3-secondary-text");
    col.append(&version);
    let blurb = gtk::Label::new(Some(
        "The web browser of the m3 desktop. Bookmarks and history stay on this computer.\nLicensed under the GNU GPL, version 3 or later.",
    ));
    blurb.set_justify(gtk::Justification::Center);
    blurb.set_wrap(true);
    blurb.set_max_width_chars(44);
    blurb.add_css_class("m3-body-medium");
    col.append(&blurb);
    let close = gtk::Button::with_label("Close");
    close.add_css_class("filled");
    close.set_halign(gtk::Align::Center);
    close.set_margin_top(8);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    col.append(&close);
    dialog.set_child(Some(&col));
    dialog.present();
}
