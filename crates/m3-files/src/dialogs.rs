//! Material 3 dialogs: rename/new folder, confirmations, properties.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::util;

/// A modal M3 dialog: returns (window, content box, button row).
pub fn dialog(parent: &gtk::Window, title: &str, body: Option<&str>) -> (gtk::Window, gtk::Box, gtk::Box) {
    let w = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .decorated(false)
        .resizable(false)
        .default_width(440)
        .title(title)
        .build();
    w.add_css_class("m3");
    w.add_css_class("m3-files-dialog-window");
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 16);
    outer.add_css_class("m3-dialog");
    outer.add_css_class("m3-enter");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("m3-headline-small-emphasized");
    t.set_xalign(0.0);
    t.set_wrap(true);
    outer.append(&t);
    if let Some(b) = body {
        let l = gtk::Label::new(Some(b));
        l.add_css_class("m3-body-medium");
        l.add_css_class("m3-secondary-text");
        l.set_xalign(0.0);
        l.set_wrap(true);
        l.set_max_width_chars(48);
        outer.append(&l);
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.append(&content);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(8);
    outer.append(&buttons);
    w.set_child(Some(&outer));
    let keys = gtk::EventControllerKey::new();
    let ww = w.clone();
    keys.connect_key_pressed(move |_, k, _, _| {
        if k == gtk::gdk::Key::Escape {
            ww.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    w.add_controller(keys);
    (w, content, buttons)
}

pub fn button(label: &str, class: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    if !class.is_empty() {
        b.add_css_class(class);
    }
    b
}

/// Ask for a name (rename / new folder). `exists(name)` rejects taken
/// names; `on_ok` gets the validated name. A rename to the same name is a
/// no-op; with `accept_initial` (new folder) the suggestion can be taken as is.
#[allow(clippy::too_many_arguments)]
pub fn ask_name(
    parent: &gtk::Window,
    title: &str,
    action: &str,
    initial: &str,
    select_stem: bool,
    accept_initial: bool,
    exists: impl Fn(&str) -> bool + 'static,
    on_ok: impl Fn(String) + 'static,
) {
    let (w, content, buttons) = dialog(parent, title, None);
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);
    content.append(&entry);
    let err = gtk::Label::new(None);
    err.add_css_class("m3-files-error");
    err.set_xalign(0.0);
    err.set_visible(false);
    content.append(&err);
    let cancel = button("Cancel", "text");
    let ok = button(action, "filled");
    buttons.append(&cancel);
    buttons.append(&ok);
    w.set_default_widget(Some(&ok));
    let initial = initial.to_owned();
    let check = {
        let (entry, err, ok) = (entry.clone(), err.clone(), ok.clone());
        let initial = initial.clone();
        Rc::new(move || -> Option<String> {
            let name = entry.text().to_string();
            let problem = match util::validate_name(&name) {
                Err(e) => Some(e.to_owned()),
                Ok(()) if (name != initial || accept_initial) && exists(&name) => {
                    Some(format!("“{name}” already exists here."))
                }
                Ok(()) => None,
            };
            err.set_visible(problem.is_some() && !name.is_empty());
            err.set_text(problem.as_deref().unwrap_or(""));
            ok.set_sensitive(problem.is_none());
            problem.is_none().then_some(name)
        })
    };
    let c = check.clone();
    entry.connect_changed(move |_| {
        c();
    });
    check();
    let ww = w.clone();
    cancel.connect_clicked(move |_| ww.close());
    let ww = w.clone();
    let on_ok = Rc::new(on_ok);
    let submit = {
        let check = check.clone();
        let on_ok = on_ok.clone();
        let initial = initial.clone();
        move || {
            if let Some(name) = check() {
                ww.close();
                if name != initial || accept_initial {
                    on_ok(name);
                }
            }
        }
    };
    let s = Rc::new(submit);
    let s2 = s.clone();
    ok.connect_clicked(move |_| s2());
    entry.connect_activate(move |_| s());
    w.present();
    entry.grab_focus();
    // Select the name without its extension, like other file managers.
    let stem_len = if select_stem { util::split_ext(&initial).0.chars().count() } else { initial.chars().count() };
    entry.select_region(0, stem_len as i32);
}

/// A yes/no confirmation.
pub fn confirm(parent: &gtk::Window, title: &str, body: &str, action: &str, on_yes: impl Fn() + 'static) {
    let (w, _, buttons) = dialog(parent, title, Some(body));
    let cancel = button("Cancel", "text");
    let yes = button(action, "danger");
    buttons.append(&cancel);
    buttons.append(&yes);
    let ww = w.clone();
    cancel.connect_clicked(move |_| ww.close());
    let ww = w.clone();
    yes.connect_clicked(move |_| {
        ww.close();
        on_yes();
    });
    w.present();
    cancel.grab_focus();
}

fn row(list: &gtk::Box, title: &str, value: &str) -> gtk::Label {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    r.add_css_class("m3-files-prop-row");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("m3-secondary-text");
    t.set_xalign(0.0);
    t.set_width_chars(10);
    let v = gtk::Label::new(Some(value));
    v.set_xalign(0.0);
    v.set_hexpand(true);
    v.set_wrap(true);
    v.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    v.set_selectable(true);
    r.append(&t);
    r.append(&v);
    list.append(&r);
    v
}

fn fmt_time(info: &gio::FileInfo, attr: &str) -> String {
    let secs = info.attribute_uint64(attr);
    if secs == 0 {
        return "Unknown".into();
    }
    glib::DateTime::from_unix_local(secs as i64)
        .ok()
        .and_then(|d| d.format("%-d %B %Y, %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Unknown".into())
}

/// Recursive size of a folder (blocking; run off the main thread).
fn tree_size(file: &gio::File) -> (u64, u64) {
    let mut total = 0u64;
    let mut count = 0u64;
    let mut stack = vec![file.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(en) = dir.enumerate_children(
            "standard::name,standard::type,standard::size",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        ) else {
            continue;
        };
        while let Ok(Some(i)) = en.next_file(gio::Cancellable::NONE) {
            count += 1;
            if i.file_type() == gio::FileType::Directory {
                stack.push(dir.child(i.name()));
            } else {
                total += i.size().max(0) as u64;
            }
        }
    }
    (total, count)
}

/// The properties window for `file`.
pub fn properties(parent: &gtk::Window, file: &gio::File) {
    let info = match file.query_info(
        "standard::*,time::*,unix::mode,unix::uid,owner::user,access::*",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    ) {
        Ok(i) => i,
        Err(e) => {
            let (w, _, b) = dialog(parent, "Properties", Some(&format!("Can't read this item: {e}")));
            let ok = button("Close", "filled");
            let ww = w.clone();
            ok.connect_clicked(move |_| ww.close());
            b.append(&ok);
            w.present();
            return;
        }
    };
    let name = info.display_name().to_string();
    let (w, content, buttons) = dialog(parent, &name, None);
    let icon = gtk::Image::from_gicon(&info.icon().unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast()));
    icon.set_pixel_size(64);
    icon.set_halign(gtk::Align::Start);
    content.prepend(&icon);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("m3-group");
    content.append(&list);
    let ct = info.content_type().map(|c| c.to_string()).unwrap_or_default();
    let desc = gio::content_type_get_description(&ct).to_string();
    row(&list, "Type", &format!("{desc} ({ct})"));
    let is_dir = info.file_type() == gio::FileType::Directory;
    let size = row(&list, if is_dir { "Contents" } else { "Size" }, &util::format_size(info.size().max(0) as u64));
    if is_dir {
        size.set_text("Calculating…");
        let f = file.clone();
        glib::spawn_future_local(async move {
            let res = gio::spawn_blocking(move || tree_size(&f)).await;
            match res {
                Ok((bytes, n)) => {
                    size.set_text(&format!("{n} item{} — {}", if n == 1 { "" } else { "s" }, util::format_size(bytes)))
                }
                Err(_) => size.set_text("Unknown"),
            }
        });
    }
    let loc = file.parent().map(|p| p.parse_name().to_string()).unwrap_or_else(|| "—".into());
    row(&list, "Location", &loc);
    row(&list, "Modified", &fmt_time(&info, "time::modified"));
    row(&list, "Accessed", &fmt_time(&info, "time::access"));
    let mode = info.attribute_uint32("unix::mode");
    let owner = info.attribute_string("owner::user").map(|s| s.to_string()).unwrap_or_default();
    row(&list, "Permissions", &format!("{} (owner {owner})", mode_string(mode)));

    // "Executable" for regular files the user owns.
    let mine = info.attribute_uint32("unix::uid") == rustix_uid();
    if !is_dir && mine && info.file_type() == gio::FileType::Regular {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        r.add_css_class("m3-files-prop-row");
        let l = gtk::Label::new(Some("Allow running as a program"));
        l.set_hexpand(true);
        l.set_xalign(0.0);
        let sw = gtk::Switch::new();
        sw.set_valign(gtk::Align::Center);
        sw.set_active(mode & 0o100 != 0);
        sw.update_property(&[gtk::accessible::Property::Label("Allow running as a program")]);
        let f = file.clone();
        sw.connect_active_notify(move |s| {
            // Executable for everyone who may read it.
            let new = if s.is_active() { mode | ((mode & 0o444) >> 2) } else { mode & !0o111 };
            if let Err(e) =
                f.set_attribute_uint32("unix::mode", new, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            {
                tracing::warn!("chmod failed: {e}");
            }
        });
        r.append(&l);
        r.append(&sw);
        list.append(&r);
    }
    let close = button("Close", "filled");
    let ww = w.clone();
    close.connect_clicked(move |_| ww.close());
    buttons.append(&close);
    w.present();
    // Focus the button, not the first selectable label.
    close.grab_focus();
}

/// `rwxr-xr-x` for a unix mode.
pub fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let m = (mode >> shift) & 7;
        s.push(if m & 4 != 0 { 'r' } else { '-' });
        s.push(if m & 2 != 0 { 'w' } else { '-' });
        s.push(if m & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Our real uid, from /proc (no libc binding needed).
fn rustix_uid() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|v| v.split_whitespace().next().and_then(|n| n.parse().ok()))
        })
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o640), "rw-r-----");
    }
}
