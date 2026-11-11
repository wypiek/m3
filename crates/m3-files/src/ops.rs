//! File operations: copy, move, trash, delete, restore, empty trash.
//!
//! Each operation runs on a worker thread with blocking GIO calls (GFile is
//! thread-safe) and reports into shared state that the UI polls ten times a
//! second. Conflicts pause the worker until the user answers.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::{dialogs, trash, util};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Replace,
    Skip,
    KeepBoth,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum Job {
    Copy { sources: Vec<gio::File>, dest: gio::File },
    Move { sources: Vec<gio::File>, dest: gio::File },
    Trash(Vec<gio::File>),
    Delete(Vec<gio::File>),
    Restore(Vec<trash::TrashItem>),
    EmptyTrash,
}

impl Job {
    fn title(&self) -> String {
        let n = |v: usize| if v == 1 { "1 item".to_owned() } else { format!("{v} items") };
        match self {
            Job::Copy { sources, .. } => format!("Copying {}", n(sources.len())),
            Job::Move { sources, .. } => format!("Moving {}", n(sources.len())),
            Job::Trash(f) => format!("Moving {} to the trash", n(f.len())),
            Job::Delete(f) => format!("Deleting {}", n(f.len())),
            Job::Restore(f) => format!("Restoring {}", n(f.len())),
            Job::EmptyTrash => "Emptying the trash".into(),
        }
    }
}

/// What a finished operation produced.
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// Original paths of items moved to the trash (for Undo).
    pub trashed: Vec<PathBuf>,
    /// Items created at the destination (to select them).
    pub created: Vec<gio::File>,
    pub errors: Vec<String>,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
struct ConflictAsk {
    name: String,
    dest_dir: String,
    is_dir: bool,
}

#[derive(Default)]
struct Shared {
    done: u64,
    total: u64,
    current: String,
    conflict: Option<ConflictAsk>,
    finished: Option<Outcome>,
}

struct Worker {
    shared: Arc<Mutex<Shared>>,
    cancel: gio::Cancellable,
    answers: mpsc::Receiver<(Resolution, bool)>,
    /// "Apply to all" answer.
    sticky: Option<Resolution>,
    cancelled: Arc<AtomicBool>,
}

impl Worker {
    fn set<T>(&self, f: impl FnOnce(&mut Shared) -> T) -> Option<T> {
        self.shared.lock().ok().map(|mut s| f(&mut s))
    }

    fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || self.cancel.is_cancelled()
    }

    fn ask(&mut self, name: &str, dest: &gio::File, is_dir: bool) -> Resolution {
        if let Some(r) = self.sticky {
            return r;
        }
        let dest_dir = dest.parse_name().to_string();
        self.set(|s| s.conflict = Some(ConflictAsk { name: name.to_owned(), dest_dir, is_dir }));
        let (r, all) = self.answers.recv().unwrap_or((Resolution::Cancel, false));
        self.set(|s| s.conflict = None);
        if all && r != Resolution::Cancel {
            self.sticky = Some(r);
        }
        if r == Resolution::Cancel {
            self.cancelled.store(true, Ordering::Relaxed);
            self.cancel.cancel();
        }
        r
    }

    fn info(f: &gio::File) -> Option<gio::FileInfo> {
        f.query_info(
            "standard::name,standard::type,standard::size,id::filesystem",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .ok()
    }

    /// Total bytes below `f` (files count their size; folders recurse).
    fn measure(&self, f: &gio::File) -> u64 {
        let Some(i) = Self::info(f) else { return 0 };
        if i.file_type() != gio::FileType::Directory {
            return i.size().max(0) as u64;
        }
        let mut total = 0;
        if let Ok(en) = f.enumerate_children(
            "standard::name,standard::type,standard::size",
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            Some(&self.cancel),
        ) {
            while let Ok(Some(c)) = en.next_file(Some(&self.cancel)) {
                if c.file_type() == gio::FileType::Directory {
                    total += self.measure(&f.child(c.name()));
                } else {
                    total += c.size().max(0) as u64;
                }
            }
        }
        total
    }

    fn copy_file(&self, src: &gio::File, dest: &gio::File, overwrite: bool) -> Result<(), glib::Error> {
        let base = self.set(|s| s.done).unwrap_or(0);
        let shared = self.shared.clone();
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        if overwrite {
            flags |= gio::FileCopyFlags::OVERWRITE;
        }
        let mut progress = move |cur: i64, _total: i64| {
            if let Ok(mut s) = shared.lock() {
                s.done = base + cur.max(0) as u64;
            }
        };
        let r = src.copy(dest, flags, Some(&self.cancel), Some(&mut progress));
        if r.is_err() && self.stopped() {
            // Don't leave a half-written file behind.
            let _ = dest.delete(gio::Cancellable::NONE);
        }
        r
    }

    /// Copy `src` (file or folder) to `dest`, asking about conflicts.
    fn copy_tree(&mut self, src: &gio::File, dest: &gio::File, out: &mut Outcome, top: bool) -> bool {
        if self.stopped() {
            return false;
        }
        let Some(info) = Self::info(src) else {
            out.errors.push(format!("Can't read “{}”.", src.parse_name()));
            return true;
        };
        let name = info.name().to_string_lossy().into_owned();
        self.set(|s| s.current = name.clone());
        let is_dir = info.file_type() == gio::FileType::Directory;
        let mut dest = dest.clone();
        let mut overwrite = false;
        if dest.query_exists(gio::Cancellable::NONE) {
            let dest_is_dir = dest.query_file_type(gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, gio::Cancellable::NONE)
                == gio::FileType::Directory;
            // Copying onto itself (paste in the same folder): keep both.
            let same = src.equal(&dest);
            let r = if same {
                Resolution::KeepBoth
            } else {
                self.ask(&name, &dest.parent().unwrap_or(dest.clone()), is_dir && dest_is_dir)
            };
            match r {
                Resolution::Cancel => return false,
                Resolution::Skip => {
                    self.set(|s| s.done += self.measure(src));
                    return true;
                }
                Resolution::KeepBoth => {
                    let parent = dest.parent().unwrap_or_else(|| dest.clone());
                    let n = util::keep_both_name(&name, |c| parent.child(c).query_exists(gio::Cancellable::NONE));
                    dest = parent.child(n);
                }
                // Folders merge; files are overwritten.
                Resolution::Replace => overwrite = !(is_dir && dest_is_dir),
            }
        }
        if is_dir {
            if let Err(e) = dest.make_directory(gio::Cancellable::NONE) {
                if !e.matches(gio::IOErrorEnum::Exists) {
                    out.errors.push(format!("Can't create “{}”: {}", dest.parse_name(), e.message()));
                    return true;
                }
            }
            if top {
                out.created.push(dest.clone());
            }
            let en = match src.enumerate_children(
                "standard::name",
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                Some(&self.cancel),
            ) {
                Ok(en) => en,
                Err(e) => {
                    out.errors.push(format!("Can't read “{}”: {}", src.parse_name(), e.message()));
                    return true;
                }
            };
            let mut children = Vec::new();
            while let Ok(Some(c)) = en.next_file(Some(&self.cancel)) {
                children.push(c.name());
            }
            for c in children {
                if !self.copy_tree(&src.child(&c), &dest.child(&c), out, false) {
                    return false;
                }
            }
            true
        } else {
            match self.copy_file(src, &dest, overwrite) {
                Ok(()) => {
                    if top {
                        out.created.push(dest.clone());
                    }
                    true
                }
                Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => false,
                Err(e) => {
                    out.errors.push(format!("Can't copy “{name}”: {}", e.message()));
                    true
                }
            }
        }
    }

    fn delete_tree(&self, f: &gio::File) -> Result<(), glib::Error> {
        if f.query_file_type(gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, gio::Cancellable::NONE)
            == gio::FileType::Directory
        {
            let en =
                f.enumerate_children("standard::name", gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, Some(&self.cancel))?;
            while let Some(c) = en.next_file(Some(&self.cancel))? {
                self.delete_tree(&f.child(c.name()))?;
            }
        }
        f.delete(Some(&self.cancel))
    }

    fn run(&mut self, job: Job) -> Outcome {
        let mut out = Outcome::default();
        match job {
            Job::Copy { sources, dest } => {
                let total = sources.iter().map(|s| self.measure(s)).sum();
                self.set(|s| s.total = total);
                for s in &sources {
                    let Some(name) = s.basename() else { continue };
                    if is_inside(&dest, s) {
                        out.errors.push(format!("Can't copy “{}” into itself.", s.parse_name()));
                        continue;
                    }
                    if !self.copy_tree(s, &dest.child(name), &mut out, true) {
                        out.cancelled = true;
                        break;
                    }
                }
            }
            Job::Move { sources, dest } => {
                let total = sources.iter().map(|s| self.measure(s)).sum();
                self.set(|s| s.total = total);
                for s in &sources {
                    if self.stopped() {
                        out.cancelled = true;
                        break;
                    }
                    let Some(name) = s.basename() else { continue };
                    let target = dest.child(&name);
                    if s.equal(&target) {
                        continue; // already there
                    }
                    if is_inside(&dest, s) {
                        out.errors.push(format!("Can't move “{}” into itself.", s.parse_name()));
                        continue;
                    }
                    let size = self.measure(s);
                    let label = name.to_string_lossy().into_owned();
                    self.set(|st| st.current = label.clone());
                    // Fast path: a rename on the same filesystem.
                    if !target.query_exists(gio::Cancellable::NONE) {
                        match s.move_(
                            &target,
                            gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE,
                            Some(&self.cancel),
                            None,
                        ) {
                            Ok(()) => {
                                self.set(|st| st.done += size);
                                out.created.push(target);
                                continue;
                            }
                            Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => {
                                out.cancelled = true;
                                break;
                            }
                            Err(_) => {} // fall back to copy + delete
                        }
                    }
                    let before = out.errors.len();
                    if !self.copy_tree(s, &target, &mut out, true) {
                        out.cancelled = true;
                        break;
                    }
                    if out.errors.len() == before {
                        if let Err(e) = self.delete_tree(s) {
                            out.errors.push(format!("Copied, but can't remove “{label}”: {}", e.message()));
                        }
                    }
                }
            }
            Job::Trash(files) => {
                self.set(|s| s.total = files.len() as u64);
                for f in &files {
                    if self.stopped() {
                        out.cancelled = true;
                        break;
                    }
                    self.set(|s| {
                        s.current = f.basename().map(|b| b.to_string_lossy().into_owned()).unwrap_or_default()
                    });
                    match f.trash(Some(&self.cancel)) {
                        Ok(()) => {
                            if let Some(p) = f.path() {
                                out.trashed.push(p);
                            }
                        }
                        Err(e) => {
                            out.errors.push(format!("Can't move “{}” to the trash: {}", f.parse_name(), e.message()))
                        }
                    }
                    self.set(|s| s.done += 1);
                }
            }
            Job::Delete(files) => {
                self.set(|s| s.total = files.len() as u64);
                for f in &files {
                    if self.stopped() {
                        out.cancelled = true;
                        break;
                    }
                    self.set(|s| {
                        s.current = f.basename().map(|b| b.to_string_lossy().into_owned()).unwrap_or_default()
                    });
                    let r = match f.path().filter(|p| trash::contains(p)) {
                        // Deleting inside the trash also drops its info file.
                        Some(p) => trash::list()
                            .into_iter()
                            .find(|i| i.path == p)
                            .map_or_else(|| self.delete_tree(f).map_err(|e| e.to_string()), |i| trash::delete(&i)),
                        None => self.delete_tree(f).map_err(|e| e.to_string()),
                    };
                    if let Err(e) = r {
                        out.errors.push(format!("Can't delete “{}”: {e}", f.parse_name()));
                    }
                    self.set(|s| s.done += 1);
                }
            }
            Job::Restore(items) => {
                self.set(|s| s.total = items.len() as u64);
                for i in &items {
                    self.set(|s| s.current = i.name.clone());
                    match trash::restore(i) {
                        Ok(()) => out.created.push(gio::File::for_path(&i.info.path)),
                        Err(e) => out.errors.push(e),
                    }
                    self.set(|s| s.done += 1);
                }
            }
            Job::EmptyTrash => {
                self.set(|s| s.total = 1);
                if let Err(e) = trash::empty() {
                    out.errors.push(e);
                }
                self.set(|s| s.done = 1);
            }
        }
        out
    }
}

/// Is `dir` the same as or inside `src`?
fn is_inside(dir: &gio::File, src: &gio::File) -> bool {
    dir.equal(src) || dir.has_prefix(src)
}

type Done = Box<dyn Fn(&Outcome)>;

struct Running {
    title: String,
    shared: Arc<Mutex<Shared>>,
    answers: mpsc::Sender<(Resolution, bool)>,
    done: Done,
    row: gtk::Box,
    bar: m3_widgets::WavyProgress,
    label: gtk::Label,
    asking: bool,
}

/// Runs operations and shows them in a toolbar button + popover.
pub struct Operations {
    parent: gtk::Window,
    pub button: gtk::MenuButton,
    list: gtk::Box,
    running: RefCell<Vec<Running>>,
    indicator: m3_widgets::CircularWavyProgress,
}

impl Operations {
    pub fn new(parent: &gtk::Window) -> Rc<Self> {
        let indicator = m3_widgets::CircularWavyProgress::new(24, None);
        let button = gtk::MenuButton::new();
        button.set_child(Some(indicator.widget()));
        button.add_css_class("icon");
        button.set_tooltip_text(Some("File operations"));
        button.set_visible(false);
        let pop = gtk::Popover::new();
        pop.add_css_class("m3-files-ops");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        list.add_css_class("m3-group");
        list.set_size_request(340, -1);
        pop.set_child(Some(&list));
        button.set_popover(Some(&pop));
        Rc::new(Self { parent: parent.clone(), button, list, running: RefCell::new(Vec::new()), indicator })
    }

    /// Start `job`; `done` runs on the main thread when it finishes.
    pub fn start(self: &Rc<Self>, job: Job, done: impl Fn(&Outcome) + 'static) {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let cancel = gio::Cancellable::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let title = job.title();
        {
            let shared = shared.clone();
            let cancel = cancel.clone();
            let cancelled = cancelled.clone();
            std::thread::spawn(move || {
                let mut w = Worker { shared: shared.clone(), cancel, answers: rx, sticky: None, cancelled };
                let out = w.run(job);
                if let Ok(mut s) = shared.lock() {
                    s.finished = Some(out);
                }
            });
        }
        // Row in the operations popover.
        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
        row.add_css_class("m3-files-op");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&title));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        head.append(&label);
        let stop = gtk::Button::new();
        stop.set_child(Some(&crate::window::icon("close")));
        stop.add_css_class("icon");
        stop.set_tooltip_text(Some("Cancel"));
        {
            let cancel = cancel.clone();
            let cancelled = cancelled.clone();
            let tx = tx.clone();
            stop.connect_clicked(move |_| {
                cancelled.store(true, Ordering::Relaxed);
                cancel.cancel();
                let _ = tx.send((Resolution::Cancel, false));
            });
        }
        head.append(&stop);
        row.append(&head);
        let bar = m3_widgets::WavyProgress::new(Some(0.0));
        row.append(bar.widget());
        self.list.append(&row);
        self.button.set_visible(true);
        let first = self.running.borrow().is_empty();
        self.running.borrow_mut().push(Running {
            title,
            shared,
            answers: tx,
            done: Box::new(done),
            row,
            bar,
            label,
            asking: false,
        });
        if first {
            let me = Rc::downgrade(self);
            glib::timeout_add_local(Duration::from_millis(100), move || match me.upgrade() {
                Some(o) if o.poll() => glib::ControlFlow::Continue,
                _ => glib::ControlFlow::Break,
            });
        }
        // Long operations show their progress without being asked.
        let me = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            if let Some(o) = me.upgrade().filter(|o| !o.running.borrow().is_empty()) {
                if o.parent.is_active() {
                    o.button.popup();
                }
            }
        });
    }

    /// Update rows; returns whether anything is still running.
    fn poll(self: &Rc<Self>) -> bool {
        let mut finished = Vec::new();
        let mut ask: Option<(usize, ConflictAsk)> = None;
        {
            let mut running = self.running.borrow_mut();
            for (i, r) in running.iter_mut().enumerate() {
                let Ok(mut s) = r.shared.lock() else { continue };
                if let Some(out) = s.finished.take() {
                    finished.push((i, out));
                    continue;
                }
                let frac = if s.total > 0 { (s.done as f64 / s.total as f64).min(1.0) } else { 0.0 };
                r.bar.set_fraction(Some(frac));
                r.label.set_text(&if s.current.is_empty() {
                    r.title.clone()
                } else {
                    format!("{} — {}", r.title, s.current)
                });
                if let Some(c) = s.conflict.clone() {
                    if !r.asking {
                        r.asking = true;
                        ask = Some((i, c));
                    }
                } else {
                    r.asking = false;
                }
            }
        }
        if let Some((i, c)) = ask {
            let tx = self.running.borrow().get(i).map(|r| r.answers.clone());
            if let Some(tx) = tx {
                conflict_dialog(&self.parent, &c, move |r, all| {
                    let _ = tx.send((r, all));
                });
            }
        }
        for (i, out) in finished.into_iter().rev() {
            let r = self.running.borrow_mut().remove(i);
            self.list.remove(&r.row);
            (r.done)(&out);
        }
        let any = !self.running.borrow().is_empty();
        if !any {
            self.button.set_visible(false);
            if let Some(p) = self.button.popover() {
                p.popdown();
            }
        }
        self.indicator.set_fraction(None);
        any
    }
}

fn conflict_dialog(parent: &gtk::Window, c: &ConflictAsk, answer: impl Fn(Resolution, bool) + 'static) {
    let body = format!("There's already an item named “{}” in “{}”.", c.name, c.dest_dir);
    let title = if c.is_dir { "Merge folders?" } else { "Replace file?" };
    let (w, content, buttons) = dialogs::dialog(parent, title, Some(&body));
    let all = gtk::CheckButton::with_label("Do this for all conflicts");
    content.append(&all);
    let answer = Rc::new(answer);
    let answered = Rc::new(std::cell::Cell::new(false));
    for (label, res, class) in [
        ("Cancel", Resolution::Cancel, "text"),
        ("Skip", Resolution::Skip, "text"),
        ("Keep both", Resolution::KeepBoth, "tonal"),
        (if c.is_dir { "Merge" } else { "Replace" }, Resolution::Replace, "filled"),
    ] {
        let b = dialogs::button(label, class);
        let (w2, a, all2, done) = (w.clone(), answer.clone(), all.clone(), answered.clone());
        b.connect_clicked(move |_| {
            done.set(true);
            a(res, all2.is_active());
            w2.close();
        });
        buttons.append(&b);
    }
    // Closing the dialog (Esc) cancels the operation.
    w.connect_close_request(move |_| {
        if !answered.get() {
            answer(Resolution::Cancel, false);
        }
        glib::Propagation::Proceed
    });
    w.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("m3-files-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tmp");
        d
    }

    fn worker(answers: Vec<(Resolution, bool)>) -> Worker {
        let (tx, rx) = mpsc::channel();
        for a in answers {
            let _ = tx.send(a);
        }
        // Keep the sender alive for the test's duration by leaking it.
        std::mem::forget(tx);
        Worker {
            shared: Arc::default(),
            cancel: gio::Cancellable::new(),
            answers: rx,
            sticky: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn copy_tree_and_keep_both() {
        let d = tmp("copy");
        std::fs::create_dir_all(d.join("src/sub")).expect("mkdir");
        std::fs::write(d.join("src/a.txt"), "hello").expect("write");
        std::fs::write(d.join("src/sub/b.txt"), "world!").expect("write");
        std::fs::create_dir_all(d.join("dst")).expect("mkdir");
        let mut w = worker(vec![(Resolution::KeepBoth, false)]);
        let job =
            Job::Copy { sources: vec![gio::File::for_path(d.join("src"))], dest: gio::File::for_path(d.join("dst")) };
        let out = w.run(job.clone());
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(std::fs::read_to_string(d.join("dst/src/sub/b.txt")).expect("copied"), "world!");
        assert_eq!(w.shared.lock().map(|s| (s.done, s.total)).ok(), Some((11, 11)));
        // Second copy conflicts: keep both → "src (2)".
        let out = w.run(job);
        assert!(out.errors.is_empty());
        assert!(d.join("dst/src (2)/a.txt").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn move_and_delete() {
        let d = tmp("move");
        std::fs::create_dir_all(d.join("a")).expect("mkdir");
        std::fs::write(d.join("a/f.txt"), "x").expect("write");
        std::fs::create_dir_all(d.join("b")).expect("mkdir");
        let mut w = worker(vec![]);
        let out = w
            .run(Job::Move { sources: vec![gio::File::for_path(d.join("a"))], dest: gio::File::for_path(d.join("b")) });
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(d.join("b/a/f.txt").is_file() && !d.join("a").exists());
        // Into itself is refused.
        let out = w.run(Job::Move {
            sources: vec![gio::File::for_path(d.join("b"))],
            dest: gio::File::for_path(d.join("b/a")),
        });
        assert_eq!(out.errors.len(), 1);
        let out = w.run(Job::Delete(vec![gio::File::for_path(d.join("b"))]));
        assert!(out.errors.is_empty() && !d.join("b").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn skip_and_replace_with_apply_to_all() {
        let d = tmp("conflict");
        std::fs::create_dir_all(d.join("dst")).expect("mkdir");
        for n in ["x", "y"] {
            std::fs::write(d.join(n), "new").expect("write");
            std::fs::write(d.join("dst").join(n), "old").expect("write");
        }
        let mut w = worker(vec![(Resolution::Replace, true)]);
        let out = w.run(Job::Copy {
            sources: vec![gio::File::for_path(d.join("x")), gio::File::for_path(d.join("y"))],
            dest: gio::File::for_path(d.join("dst")),
        });
        assert!(out.errors.is_empty());
        assert_eq!(std::fs::read_to_string(d.join("dst/y")).expect("y"), "new", "apply to all");
        let mut w = worker(vec![(Resolution::Skip, false), (Resolution::Skip, false)]);
        std::fs::write(d.join("x"), "newer").expect("write");
        let _ = w.run(Job::Copy {
            sources: vec![gio::File::for_path(d.join("x"))],
            dest: gio::File::for_path(d.join("dst")),
        });
        assert_eq!(std::fs::read_to_string(d.join("dst/x")).expect("x"), "new", "skipped");
        let _ = std::fs::remove_dir_all(&d);
    }
}
