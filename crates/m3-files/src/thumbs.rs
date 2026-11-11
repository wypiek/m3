//! Image thumbnails, freedesktop thumbnail spec: `~/.cache/thumbnails/large`
//! PNGs named by the MD5 of the file URI, generated off the main thread.
//! GIO already reports existing thumbnails as `thumbnail::path`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};

/// Size of "large" thumbnails.
const SIZE: i32 = 256;
/// Don't thumbnail huge images in the UI process.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub fn is_thumbnailable(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/x-bmp" | "image/tiff"
    )
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("thumbnails").join("large"))
}

/// Cache path for `uri`.
pub fn cache_path(uri: &str) -> Option<PathBuf> {
    let md5 = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)?;
    Some(cache_dir()?.join(format!("{md5}.png")))
}

/// Generate (blocking) a thumbnail PNG for `path` and return its path.
fn generate(path: PathBuf, uri: String, mtime: i64) -> Option<PathBuf> {
    let out = cache_path(&uri)?;
    if out.is_file() {
        return Some(out);
    }
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > MAX_BYTES {
        return None;
    }
    let pix = gdk_pixbuf::Pixbuf::from_file_at_scale(&path, SIZE, SIZE, true).ok()?;
    let pix = pix.apply_embedded_orientation().unwrap_or(pix);
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension(format!("{}.tmp", std::process::id()));
    let mtime = mtime.to_string();
    pix.savev(&tmp, "png", &[("tEXt::Thumb::URI", uri.as_str()), ("tEXt::Thumb::MTime", mtime.as_str())]).ok()?;
    std::fs::rename(&tmp, &out).ok()?;
    Some(out)
}

/// Loads thumbnails and hands them to whoever still wants them.
#[derive(Default)]
pub struct Thumbnailer {
    textures: RefCell<HashMap<String, gdk::Texture>>,
    failed: RefCell<HashSet<String>>,
    pending: RefCell<HashSet<String>>,
}

impl Thumbnailer {
    /// Ask for the thumbnail of `info`; `deliver` runs on the main thread
    /// (possibly right away) if one becomes available.
    pub fn request(self: &Rc<Self>, info: &gio::FileInfo, file: &gio::File, deliver: impl Fn(&gdk::Texture) + 'static) {
        let uri = file.uri().to_string();
        if let Some(t) = self.textures.borrow().get(&uri) {
            deliver(t);
            return;
        }
        if self.failed.borrow().contains(&uri) {
            return;
        }
        // An existing thumbnail GIO already knows about.
        if let Some(p) = info.attribute_byte_string("thumbnail::path").filter(|_| !info.boolean("thumbnail::failed")) {
            if let Ok(t) = gdk::Texture::from_filename(p.as_str()) {
                self.textures.borrow_mut().insert(uri, t.clone());
                deliver(&t);
                return;
            }
        }
        let ct = info.content_type().map(|c| c.to_string()).unwrap_or_default();
        let Some(path) = file.path().filter(|_| is_thumbnailable(&ct)) else { return };
        if !self.pending.borrow_mut().insert(uri.clone()) {
            return;
        }
        let mtime = info.modification_date_time().map_or(0, |d| d.to_unix());
        let me = self.clone();
        glib::spawn_future_local(async move {
            let u = uri.clone();
            let res = gio::spawn_blocking(move || generate(path, u, mtime)).await.ok().flatten();
            me.pending.borrow_mut().remove(&uri);
            match res.and_then(|p| gdk::Texture::from_filename(&p).ok()) {
                Some(t) => {
                    me.textures.borrow_mut().insert(uri, t.clone());
                    deliver(&t);
                }
                None => {
                    me.failed.borrow_mut().insert(uri);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_name_is_md5_of_uri() {
        // MD5("file:///tmp/a.png")
        let p = cache_path("file:///tmp/a.png").expect("path");
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        assert_eq!(name.len(), 36);
        assert!(name.ends_with(".png"));
        assert!(is_thumbnailable("image/jpeg") && !is_thumbnailable("text/plain"));
    }
}
