//! The navigation drawer: places, bookmarks, devices (mounts and volumes).

use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::util::{self, Bookmark};
use crate::window::{Location, icon};

/// What the window should do when a sidebar row is chosen.
pub enum Target {
    Go(Location),
    /// Mount a volume, then open its root.
    Mount(gio::Volume),
}

type Navigate = Rc<dyn Fn(Target)>;
type Notify = Rc<dyn Fn(&str)>;

pub struct Sidebar {
    pub widget: gtk::ScrolledWindow,
    list: gtk::ListBox,
    navigate: Navigate,
    notify: Notify,
    monitor: gio::VolumeMonitor,
    rows: std::cell::RefCell<Vec<(gtk::ListBoxRow, Location)>>,
}

pub fn bookmarks_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gtk-3.0").join("bookmarks"))
}

pub fn load_bookmarks() -> Vec<Bookmark> {
    bookmarks_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| util::parse_bookmarks(&t))
        .unwrap_or_default()
}

pub fn save_bookmarks(b: &[Bookmark]) -> Result<(), String> {
    let p = bookmarks_path().ok_or("no configuration directory")?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("m3-tmp");
    std::fs::write(&tmp, util::serialize_bookmarks(b)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

fn section(text: &str) -> gtk::ListBoxRow {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("m3-label-large-emphasized");
    l.add_css_class("m3-files-section");
    l.set_xalign(0.0);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&l));
    row.set_selectable(false);
    row.set_activatable(false);
    row.add_css_class("m3-files-section-row");
    row
}

impl Sidebar {
    pub fn new(navigate: Navigate, notify: Notify) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.add_css_class("m3-navigation-drawer");
        list.add_css_class("m3-files-sidebar");
        // The current place is marked with a class, not the list selection
        // (which would follow keyboard focus).
        list.set_selection_mode(gtk::SelectionMode::None);
        let widget = gtk::ScrolledWindow::new();
        widget.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        widget.set_child(Some(&list));
        widget.set_size_request(248, -1);
        let me = Rc::new(Self {
            widget,
            list,
            navigate,
            notify,
            monitor: gio::VolumeMonitor::get(),
            rows: Default::default(),
        });
        {
            let weak = Rc::downgrade(&me);
            me.list.connect_row_activated(move |_, row| {
                if let Some(s) = weak.upgrade() {
                    s.activate(row);
                }
            });
        }
        let weak = Rc::downgrade(&me);
        let refresh = move || {
            if let Some(s) = weak.upgrade() {
                s.rebuild();
            }
        };
        let r = refresh.clone();
        me.monitor.connect_mount_added(move |_, _| r());
        let r = refresh.clone();
        me.monitor.connect_mount_removed(move |_, _| r());
        let r = refresh.clone();
        me.monitor.connect_mount_changed(move |_, _| r());
        let r = refresh.clone();
        me.monitor.connect_volume_added(move |_, _| r());
        me.monitor.connect_volume_removed(move |_, _| refresh());
        me.rebuild();
        me
    }

    fn activate(&self, row: &gtk::ListBoxRow) {
        if let Some((_, loc)) = self.rows.borrow().iter().find(|(r, _)| r == row) {
            (self.navigate)(Target::Go(loc.clone()));
            return;
        }
        // Unmounted volumes carry their volume in the row's name lookup.
        let name = row.widget_name().to_string();
        if let Some(v) =
            self.monitor.volumes().into_iter().find(|v| v.uuid().map(|u| u.to_string()) == Some(name.clone()))
        {
            (self.navigate)(Target::Mount(v));
        }
    }

    fn place(&self, symbol: &str, label: &str, loc: Location, trailing: Option<gtk::Widget>) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("m3-nav-item");
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.append(&icon(symbol));
        let l = gtk::Label::new(Some(label));
        l.set_xalign(0.0);
        l.set_hexpand(true);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.append(&l);
        if let Some(t) = trailing {
            b.append(&t);
        }
        row.set_child(Some(&b));
        row.set_tooltip_text(Some(&loc.describe()));
        self.list.append(&row);
        self.rows.borrow_mut().push((row.clone(), loc));
        row
    }

    /// Rebuild all rows (places, bookmarks, devices).
    pub fn rebuild(self: &Rc<Self>) {
        let selected = self.selected_location();
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.rows.borrow_mut().clear();
        let home = glib::home_dir();
        self.place("home", "Home", Location::Folder(gio::File::for_path(&home)), None);
        self.place("schedule", "Recent", Location::Recent, None);
        for (dir, sym, name) in [
            (glib::UserDirectory::Desktop, "desktop_windows", "Desktop"),
            (glib::UserDirectory::Documents, "description", "Documents"),
            (glib::UserDirectory::Downloads, "download", "Downloads"),
            (glib::UserDirectory::Music, "music_note", "Music"),
            (glib::UserDirectory::Pictures, "image", "Pictures"),
            (glib::UserDirectory::Videos, "movie", "Videos"),
        ] {
            if let Some(p) = glib::user_special_dir(dir).filter(|p| p.is_dir() && *p != home) {
                self.place(sym, name, Location::Folder(gio::File::for_path(p)), None);
            }
        }
        let n = crate::trash::count();
        let badge = gtk::Label::new(Some(&n.to_string()));
        badge.add_css_class("m3-files-count");
        badge.set_valign(gtk::Align::Center);
        badge.set_visible(n > 0);
        self.place(if n > 0 { "delete" } else { "delete_outline" }, "Trash", Location::Trash, Some(badge.upcast()));

        // Bookmarks, removable with a right click.
        let marks = load_bookmarks();
        if !marks.is_empty() {
            self.list.append(&section("Bookmarks"));
        }
        for bm in marks {
            let f = gio::File::for_uri(&bm.uri);
            let label = bm
                .label
                .clone()
                .or_else(|| f.basename().map(|b| b.to_string_lossy().into_owned()))
                .unwrap_or_else(|| bm.uri.clone());
            let row = self.place("bookmark", &label, Location::Folder(f), None);
            let click = gtk::GestureClick::new();
            click.set_button(3);
            let me = Rc::downgrade(self);
            let uri = bm.uri.clone();
            let r = row.clone();
            click.connect_pressed(move |_, _, _, _| {
                let pop = gtk::Popover::new();
                pop.add_css_class("menu");
                let b = gtk::Button::new();
                b.add_css_class("m3-menu-item");
                b.add_css_class("flat");
                let bx = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                bx.append(&icon("bookmark_remove"));
                bx.append(&gtk::Label::new(Some("Remove bookmark")));
                b.set_child(Some(&bx));
                pop.set_child(Some(&b));
                pop.set_parent(&r);
                let (me, uri, p2) = (me.clone(), uri.clone(), pop.clone());
                b.connect_clicked(move |_| {
                    p2.popdown();
                    let marks: Vec<Bookmark> = load_bookmarks().into_iter().filter(|m| m.uri != uri).collect();
                    if let Some(s) = me.upgrade() {
                        match save_bookmarks(&marks) {
                            Ok(()) => s.rebuild(),
                            Err(e) => (s.notify)(&format!("Can't save bookmarks: {e}")),
                        }
                    }
                });
                pop.connect_closed(|p| p.unparent());
                pop.popup();
            });
            row.add_controller(click);
        }

        // Devices.
        let mounts = self.monitor.mounts();
        let volumes: Vec<gio::Volume> =
            self.monitor.volumes().into_iter().filter(|v| v.get_mount().is_none() && v.can_mount()).collect();
        if !mounts.is_empty() || !volumes.is_empty() {
            self.list.append(&section("Devices"));
        }
        for m in mounts.into_iter().filter(|m| !m.is_shadowed()) {
            let root = m.root();
            let name = m.name().to_string();
            let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            // Usage, if the filesystem reports it.
            if let Ok(fi) = root.query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE) {
                let (size, free) = (fi.attribute_uint64("filesystem::size"), fi.attribute_uint64("filesystem::free"));
                if size > 0 {
                    let bar = gtk::ProgressBar::new();
                    bar.add_css_class("m3-files-usage");
                    bar.set_fraction(1.0 - free as f64 / size as f64);
                    bar.set_valign(gtk::Align::Center);
                    bar.set_size_request(36, -1);
                    bar.set_tooltip_text(Some(&format!(
                        "{} free of {}",
                        util::format_size(free),
                        util::format_size(size)
                    )));
                    trailing.append(&bar);
                }
            }
            if m.can_unmount() || m.can_eject() {
                let eject = gtk::Button::new();
                eject.set_child(Some(&icon("eject")));
                eject.add_css_class("icon");
                eject.add_css_class("m3-files-eject");
                let what = if m.can_eject() { "Eject" } else { "Unmount" };
                eject.set_tooltip_text(Some(&format!("{what} {name}")));
                let me = Rc::downgrade(self);
                let m2 = m.clone();
                eject.connect_clicked(move |b| {
                    let op = gtk::MountOperation::new(b.root().and_downcast_ref::<gtk::Window>());
                    let me = me.clone();
                    let m2 = m2.clone();
                    let name = m2.name().to_string();
                    glib::spawn_future_local(async move {
                        let r = if m2.can_eject() {
                            m2.eject_with_operation_future(gio::MountUnmountFlags::NONE, Some(&op)).await
                        } else {
                            m2.unmount_with_operation_future(gio::MountUnmountFlags::NONE, Some(&op)).await
                        };
                        if let (Err(e), Some(s)) = (r, me.upgrade()) {
                            (s.notify)(&format!("Can't remove “{name}”: {}", e.message()));
                        }
                    });
                });
                trailing.append(&eject);
            }
            self.place(
                if m.can_eject() { "usb" } else { "hard_drive" },
                &name,
                Location::Folder(root),
                Some(trailing.upcast()),
            );
        }
        for v in volumes {
            let row = gtk::ListBoxRow::new();
            row.add_css_class("m3-nav-item");
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.append(&icon("hard_drive"));
            let l = gtk::Label::new(Some(&v.name()));
            l.set_xalign(0.0);
            l.set_hexpand(true);
            l.add_css_class("m3-secondary-text");
            b.append(&l);
            row.set_child(Some(&b));
            row.set_tooltip_text(Some("Not mounted — click to mount"));
            row.set_widget_name(&v.uuid().map(|u| u.to_string()).unwrap_or_default());
            self.list.append(&row);
        }
        if let Some(loc) = selected {
            self.select(&loc);
        }
    }

    fn selected_location(&self) -> Option<Location> {
        self.rows.borrow().iter().find(|(r, _)| r.has_css_class("current")).map(|(_, l)| l.clone())
    }

    /// Highlight the row for `loc` (or none).
    pub fn select(&self, loc: &Location) {
        let rows = self.rows.borrow();
        let mut found = false;
        for (r, l) in rows.iter() {
            if !found && l.same_place(loc) {
                r.add_css_class("current");
                found = true;
            } else {
                r.remove_css_class("current");
            }
        }
    }
}
