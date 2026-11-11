//! Downloads: saved to the XDG download directory under a unique name and
//! listed in a popover with expressive wavy progress.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{gio, glib};
use m3_widgets::WavyProgress;
use webkit6::prelude::*;

use crate::store;
use crate::ui::{icon, icon_button};

type Listener = Box<dyn Fn(usize)>;

/// The app-wide downloads list. Its widget moves into whichever window's
/// popover is opened.
pub struct Downloads {
    root: gtk::Box,
    list: gtk::Box,
    empty: gtk::Label,
    count: RefCell<usize>,
    current: RefCell<Option<gtk::Popover>>,
    listeners: RefCell<Vec<Listener>>,
}

pub fn download_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(|| glib::home_dir().join("Downloads"))
}

fn human_bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0} kB", n / 1e3)
    } else {
        format!("{n:.0} B")
    }
}

fn parent_window(w: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    w.as_ref().root().and_downcast::<gtk::Window>()
}

impl Downloads {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("m3-downloads");
        let title = gtk::Label::new(Some("Downloads"));
        title.add_css_class("m3-title-medium-emphasized");
        title.set_xalign(0.0);
        root.append(&title);
        let empty = gtk::Label::new(Some("Files you download appear here."));
        empty.add_css_class("m3-secondary-text");
        empty.set_margin_top(8);
        empty.set_margin_bottom(8);
        root.append(&empty);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("m3-group");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_max_content_height(420);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&list));
        root.append(&scroller);
        root.set_size_request(380, -1);
        Rc::new(Self {
            root,
            list,
            empty,
            count: RefCell::new(0),
            current: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
        })
    }

    /// Show the list in `popover` (moving it from any other window).
    pub fn attach(&self, popover: &gtk::Popover) {
        if let Some(old) = self.current.borrow_mut().take()
            && &old != popover
        {
            old.set_child(None::<&gtk::Widget>);
        }
        if self.root.parent().is_none() {
            popover.set_child(Some(&self.root));
        }
        *self.current.borrow_mut() = Some(popover.clone());
    }

    /// Detach from a popover whose window is going away.
    pub fn detach(&self, popover: &gtk::Popover) {
        let mine = self.current.borrow().as_ref() == Some(popover);
        if mine {
            popover.set_child(None::<&gtk::Widget>);
            *self.current.borrow_mut() = None;
        }
    }

    pub fn count(&self) -> usize {
        *self.count.borrow()
    }

    /// Called with the number of downloads whenever one is added.
    pub fn connect_changed(&self, f: impl Fn(usize) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Track a download WebKit has started.
    pub fn add(self: &Rc<Self>, download: &webkit6::Download) {
        download.connect_decide_destination(|d, suggested| {
            let dir = download_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!("cannot create {}: {e}", dir.display());
            }
            let path = store::unique_download_path(&dir, suggested, |p| p.exists());
            d.set_allow_overwrite(false);
            d.set_destination(&path.to_string_lossy());
            true
        });

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("m3-download-row");
        let glyph = icon("draft");
        glyph.add_css_class("m3-download-icon");
        glyph.set_valign(gtk::Align::Center);
        row.append(&glyph);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
        col.set_hexpand(true);
        col.set_valign(gtk::Align::Center);
        let name = gtk::Label::new(Some(
            &download
                .request()
                .and_then(|r| r.uri())
                .map(|u| u.rsplit('/').next().unwrap_or("").to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Download".into()),
        ));
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_max_width_chars(28);
        name.add_css_class("m3-title-small-emphasized");
        let status = gtk::Label::new(Some("Starting…"));
        status.set_xalign(0.0);
        status.add_css_class("m3-body-small");
        status.add_css_class("m3-secondary-text");
        let progress = WavyProgress::new(None);
        progress.set_waving(true);
        col.append(&name);
        col.append(&status);
        col.append(progress.widget());
        row.append(&col);

        let cancel = icon_button("close", "Cancel download");
        let open = icon_button("open_in_new", "Open");
        let folder = icon_button("folder_open", "Show in folder");
        open.set_visible(false);
        folder.set_visible(false);
        for b in [&cancel, &open, &folder] {
            b.set_valign(gtk::Align::Center);
            row.append(b);
        }
        self.list.prepend(&row);
        self.empty.set_visible(false);
        *self.count.borrow_mut() += 1;
        let n = self.count();
        for l in self.listeners.borrow().iter() {
            l(n);
        }

        let d = download.clone();
        cancel.connect_clicked(move |_| d.cancel());

        let dest: Rc<RefCell<Option<PathBuf>>> = Rc::default();
        {
            let (name, dest) = (name.clone(), dest.clone());
            download.connect_created_destination(move |_, path| {
                let p = PathBuf::from(path);
                if let Some(n) = p.file_name() {
                    name.set_text(&n.to_string_lossy());
                }
                *dest.borrow_mut() = Some(p);
            });
        }
        {
            let (status, progress) = (status.clone(), progress.clone());
            download.connect_received_data(move |d, _| {
                let total = d.response().map(|r| r.content_length()).unwrap_or(0);
                let got = d.received_data_length();
                if total > 0 {
                    progress.set_fraction(Some(d.estimated_progress()));
                    status.set_text(&format!("{} of {}", human_bytes(got), human_bytes(total)));
                } else {
                    status.set_text(&human_bytes(got));
                }
            });
        }
        let failed = Rc::new(std::cell::Cell::new(false));
        {
            let (status, failed, progress) = (status.clone(), failed.clone(), progress.clone());
            download.connect_failed(move |_, err| {
                failed.set(true);
                progress.widget().set_visible(false);
                if err.matches(webkit6::DownloadError::CancelledByUser) {
                    status.set_text("Cancelled");
                } else {
                    status.set_text(&format!("Failed: {}", err.message()));
                }
            });
        }
        {
            let (status, progress, cancel, open, folder, glyph) =
                (status.clone(), progress.clone(), cancel.clone(), open.clone(), folder.clone(), glyph.clone());
            let dest2 = dest.clone();
            download.connect_finished(move |d| {
                cancel.set_visible(false);
                progress.widget().set_visible(false);
                if failed.get() {
                    return;
                }
                glyph.set_text("download_done");
                status.set_text(&format!("Done · {}", human_bytes(d.received_data_length())));
                let path = dest2.borrow().clone().or_else(|| d.destination().map(|p| PathBuf::from(p.as_str())));
                if path.is_some() {
                    open.set_visible(true);
                    folder.set_visible(true);
                }
            });
        }
        {
            let dest = dest.clone();
            open.connect_clicked(move |b| {
                let Some(p) = dest.borrow().clone() else { return };
                let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&p)));
                launcher.launch(parent_window(b).as_ref(), gio::Cancellable::NONE, |r| {
                    if let Err(e) = r {
                        tracing::warn!("opening download: {e}");
                    }
                });
            });
        }
        folder.connect_clicked(move |b| {
            let Some(p) = dest.borrow().clone() else { return };
            let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&p)));
            launcher.open_containing_folder(parent_window(b).as_ref(), gio::Cancellable::NONE, |r| {
                if let Err(e) = r {
                    tracing::warn!("showing download: {e}");
                }
            });
        });
    }
}
