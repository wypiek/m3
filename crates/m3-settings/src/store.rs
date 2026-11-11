//! Typed, debounced access to one configuration file.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use m3_core::config::{self as cfg, ConfigFile};

pub struct Store<T: ConfigFile> {
    dir: PathBuf,
    value: RefCell<T>,
    pending: Cell<Option<glib::SourceId>>,
    /// Problems loading the file (shown to the user once).
    pub load_error: Option<String>,
    /// Ask the compositor to reload after saving.
    reload_compositor: bool,
    on_error: Rc<dyn Fn(String)>,
}

impl<T: ConfigFile + 'static> Store<T> {
    pub fn load(dir: &std::path::Path, reload_compositor: bool, on_error: Rc<dyn Fn(String)>) -> Rc<Self> {
        let (value, load_error) = match cfg::load::<T>(dir) {
            Ok(l) => (l.value, None),
            Err(e) => (T::default(), Some(e.to_string())),
        };
        Rc::new(Self {
            dir: dir.to_owned(),
            value: RefCell::new(value),
            pending: Cell::new(None),
            load_error,
            reload_compositor,
            on_error,
        })
    }

    pub fn get(&self) -> T {
        self.value.borrow().clone()
    }

    /// Change the value; it is saved shortly after the last change.
    pub fn update(self: &Rc<Self>, f: impl FnOnce(&mut T)) {
        f(&mut self.value.borrow_mut());
        if let Some(id) = self.pending.take() {
            id.remove();
        }
        let me = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(350), move || {
            if let Some(s) = me.upgrade() {
                s.pending.set(None);
                s.save_now();
            }
        });
        self.pending.set(Some(id));
    }

    /// Save a pending change now (e.g. before something reads the file).
    pub fn flush(&self) {
        if let Some(id) = self.pending.take() {
            id.remove();
            self.save_now();
        }
    }

    fn save_now(&self) {
        let value = self.value.borrow().clone();
        if let Err(e) = cfg::save(&self.dir, &value) {
            (self.on_error)(e.to_string());
            return;
        }
        if self.reload_compositor {
            reload_compositor();
        }
    }
}

impl<T: ConfigFile> Drop for Store<T> {
    fn drop(&mut self) {
        // Flush a pending save when the window closes.
        if let Some(id) = self.pending.take() {
            id.remove();
            let value = self.value.borrow().clone();
            if let Err(e) = cfg::save(&self.dir, &value) {
                tracing::warn!("could not save {}: {e}", T::FILE_NAME);
            }
        }
    }
}

pub fn reload_compositor() {
    std::thread::spawn(|| {
        if let Ok(mut c) = m3_protocol::Client::connect() {
            let _ = c.request(&m3_protocol::Request::Reload);
        }
    });
}

/// Send a request to the compositor in the background.
pub fn compositor(req: m3_protocol::Request) {
    std::thread::spawn(move || {
        if let Ok(mut c) = m3_protocol::Client::connect() {
            if let Err(e) = c.request(&req) {
                tracing::warn!("{e}");
            }
        }
    });
}

/// Query the compositor (blocking, short).
pub fn query(req: m3_protocol::Request) -> Option<m3_protocol::Response> {
    m3_protocol::Client::connect().ok()?.request(&req).ok()
}
