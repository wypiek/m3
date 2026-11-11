//! The home trash (`$XDG_DATA_HOME/Trash`, freedesktop trash spec) read and
//! written directly, so the trash view, restore and empty work without gvfs.
//! Moving *into* the trash goes through `GFile::trash`.

use std::path::{Path, PathBuf};

use gtk::prelude::*;

use crate::util::{TrashInfo, parse_trashinfo};

#[derive(Debug, Clone)]
pub struct TrashItem {
    /// Name inside `Trash/files`.
    pub name: String,
    /// The trashed file (`Trash/files/<name>`).
    pub path: PathBuf,
    pub info: TrashInfo,
}

pub fn dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("Trash"))
}

fn info_path(trash: &Path, name: &str) -> PathBuf {
    trash.join("info").join(format!("{name}.trashinfo"))
}

/// Everything in the home trash, newest first.
pub fn list() -> Vec<TrashItem> {
    let Some(trash) = dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(trash.join("files")) else { return Vec::new() };
    let mut items: Vec<TrashItem> = rd
        .filter_map(Result::ok)
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(info_path(&trash, &name)).ok();
            let info = text
                .as_deref()
                .and_then(parse_trashinfo)
                .unwrap_or(TrashInfo { path: PathBuf::from(format!("/{name}")), deletion_date: String::new() });
            TrashItem { name, path: e.path(), info }
        })
        .collect();
    items.sort_by(|a, b| b.info.deletion_date.cmp(&a.info.deletion_date));
    items
}

pub fn count() -> usize {
    dir().and_then(|t| std::fs::read_dir(t.join("files")).ok()).map_or(0, |rd| rd.count())
}

/// The most recently trashed item that came from `orig` (for Undo).
pub fn find_latest(orig: &Path) -> Option<TrashItem> {
    list().into_iter().find(|i| i.info.path == orig)
}

/// Put an item back where it came from.
pub fn restore(item: &TrashItem) -> Result<(), String> {
    let dest = &item.info.path;
    if dest.exists() {
        return Err(format!("“{}” already exists.", dest.display()));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Can't recreate {}: {e}", parent.display()))?;
    }
    match std::fs::rename(&item.path, dest) {
        Ok(()) => {}
        // Another filesystem: let GIO copy it across (files only).
        Err(e) if e.raw_os_error() == Some(18) => {
            let src = gtk::gio::File::for_path(&item.path);
            src.move_(
                &gtk::gio::File::for_path(dest),
                gtk::gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                gtk::gio::Cancellable::NONE,
                None,
            )
            .map_err(|e| format!("Can't restore “{}”: {e}", item.name))?;
        }
        Err(e) => return Err(format!("Can't restore “{}”: {e}", item.name)),
    }
    if let Some(t) = dir() {
        let _ = std::fs::remove_file(info_path(&t, &item.name));
    }
    Ok(())
}

/// Delete one trashed item for good.
pub fn delete(item: &TrashItem) -> Result<(), String> {
    let r = match std::fs::symlink_metadata(&item.path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&item.path),
        Ok(_) => std::fs::remove_file(&item.path),
        Err(e) => Err(e),
    };
    r.map_err(|e| format!("Can't delete “{}”: {e}", item.name))?;
    if let Some(t) = dir() {
        let _ = std::fs::remove_file(info_path(&t, &item.name));
    }
    Ok(())
}

/// Empty the trash; returns the first error but tries every item.
pub fn empty() -> Result<(), String> {
    let mut first = None;
    for item in list() {
        if let Err(e) = delete(&item) {
            first.get_or_insert(e);
        }
    }
    // Stray info files without data.
    if let Some(t) = dir() {
        if let Ok(rd) = std::fs::read_dir(t.join("info")) {
            for e in rd.filter_map(Result::ok) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    first.map_or(Ok(()), Err)
}

/// Is `path` inside the home trash's `files` directory?
pub fn contains(path: &Path) -> bool {
    dir().is_some_and(|t| path.starts_with(t.join("files")))
}
