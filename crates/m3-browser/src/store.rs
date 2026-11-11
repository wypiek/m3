//! Bookmarks, history and browser settings, stored as JSON in
//! `~/.local/share/m3/browser` with atomic, owner-only writes.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Oldest history entries are dropped beyond this.
pub const HISTORY_LIMIT: usize = 5000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    /// Unix seconds.
    pub added: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// Unix seconds.
    pub last: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub search_engine: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { search_engine: crate::address::DEFAULT_ENGINE.to_owned() }
    }
}

/// Write `data` to `path` atomically (temporary file + rename, mode 0600).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            // Keep the unreadable file so nothing is lost when we next save.
            let backup = path.with_extension("json.corrupt");
            let kept = std::fs::copy(path, &backup).is_ok();
            tracing::warn!(
                "{}: {e}; starting empty{}",
                path.display(),
                if kept { format!(" (old file kept as {})", backup.display()) } else { String::new() }
            );
            T::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => T::default(),
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            T::default()
        }
    }
}

fn save_json<T: Serialize>(path: &Path, value: &T) {
    match serde_json::to_vec_pretty(value) {
        Ok(bytes) => {
            if let Err(e) = write_atomic(path, &bytes) {
                tracing::warn!("saving {}: {e}", path.display());
            }
        }
        Err(e) => tracing::warn!("serialising {}: {e}", path.display()),
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Only real web pages are remembered.
pub fn is_recordable(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// All persistent browser data.
pub struct Store {
    dir: PathBuf,
    pub bookmarks: Vec<Bookmark>,
    pub history: Vec<Visit>,
    pub settings: Settings,
}

impl Store {
    pub fn open(dir: PathBuf) -> Self {
        let bookmarks = load_json(&dir.join("bookmarks.json"));
        let history = load_json(&dir.join("history.json"));
        let settings = load_json(&dir.join("settings.json"));
        Self { dir, bookmarks, history, settings }
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    /// Add (or retitle) a bookmark; returns false for non-web URLs.
    pub fn add_bookmark(&mut self, url: &str, title: &str) -> bool {
        if !is_recordable(url) {
            return false;
        }
        match self.bookmarks.iter_mut().find(|b| b.url == url) {
            Some(b) => b.title = title.to_owned(),
            None => self.bookmarks.push(Bookmark { url: url.to_owned(), title: title.to_owned(), added: now() }),
        }
        self.save_bookmarks();
        true
    }

    pub fn remove_bookmark(&mut self, url: &str) {
        let before = self.bookmarks.len();
        self.bookmarks.retain(|b| b.url != url);
        if self.bookmarks.len() != before {
            self.save_bookmarks();
        }
    }

    /// Record a visit (de-duplicated by URL, most recent first).
    pub fn record_visit(&mut self, url: &str, title: &str) {
        if !is_recordable(url) {
            return;
        }
        let at = now();
        match self.history.iter().position(|v| v.url == url) {
            Some(i) => {
                let mut v = self.history.remove(i);
                v.visits = v.visits.saturating_add(1);
                v.last = at;
                if !title.is_empty() {
                    v.title = title.to_owned();
                }
                self.history.insert(0, v);
            }
            None => self.history.insert(0, Visit { url: url.to_owned(), title: title.to_owned(), visits: 1, last: at }),
        }
        self.history.truncate(HISTORY_LIMIT);
        self.save_history();
    }

    /// Update the title of the most recent visit to `url` (titles arrive
    /// after the load commits).
    pub fn retitle(&mut self, url: &str, title: &str) {
        if title.is_empty() {
            return;
        }
        let mut changed = false;
        if let Some(v) = self.history.iter_mut().find(|v| v.url == url)
            && v.title != title
        {
            v.title = title.to_owned();
            changed = true;
        }
        if changed {
            self.save_history();
        }
    }

    pub fn remove_visit(&mut self, url: &str) {
        self.history.retain(|v| v.url != url);
        self.save_history();
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.save_history();
    }

    pub fn set_engine(&mut self, id: &str) {
        self.settings.search_engine = id.to_owned();
        save_json(&self.dir.join("settings.json"), &self.settings);
    }

    /// Up to `limit` `(url, title, is_bookmark)` suggestions for `query`:
    /// bookmarks first, then history by visit count and recency.
    pub fn suggest(&self, query: &str, limit: usize) -> Vec<(String, String, bool)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let hit = |url: &str, title: &str| {
            let u = url.to_lowercase();
            let u = u.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
            u.contains(&q) || title.to_lowercase().contains(&q)
        };
        let mut out: Vec<(String, String, bool)> = self
            .bookmarks
            .iter()
            .filter(|b| hit(&b.url, &b.title))
            .map(|b| (b.url.clone(), b.title.clone(), true))
            .collect();
        let mut visits: Vec<&Visit> =
            self.history.iter().filter(|v| hit(&v.url, &v.title) && !out.iter().any(|o| o.0 == v.url)).collect();
        visits.sort_by(|a, b| b.visits.cmp(&a.visits).then(b.last.cmp(&a.last)));
        out.extend(visits.into_iter().map(|v| (v.url.clone(), v.title.clone(), false)));
        out.truncate(limit);
        out
    }

    fn save_bookmarks(&self) {
        save_json(&self.dir.join("bookmarks.json"), &self.bookmarks);
    }

    fn save_history(&self) {
        save_json(&self.dir.join("history.json"), &self.history);
    }
}

/// A file name for a download that doesn't overwrite anything in `dir`:
/// path components are stripped, and `name (2).ext`, `name (3).ext`, … are
/// tried. `exists` is injectable for testing.
pub fn unique_download_path(dir: &Path, suggested: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let base = suggested.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let base: String = base.chars().filter(|c| !c.is_control()).collect();
    let base = match base.trim_start_matches('.') {
        "" => "download".to_owned(),
        b => b.to_owned(),
    };
    let candidate = dir.join(&base);
    if !exists(&candidate) {
        return candidate;
    }
    // Keep compound extensions like `.tar.gz` together.
    let (stem, ext) = match base.find('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base.as_str(), ""),
    };
    for n in 2..10_000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !exists(&p) {
            return p;
        }
    }
    dir.join(format!("{stem} ({}){ext}", now()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("m3-browser-test-{tag}-{}-{}", std::process::id(), now()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn bookmarks_round_trip_and_dedupe() {
        let dir = temp_dir("bm");
        let mut s = Store::open(dir.clone());
        assert!(s.add_bookmark("https://example.com/", "Example"));
        assert!(s.add_bookmark("https://example.com/", "Example Domain"));
        assert!(!s.add_bookmark("about:blank", "Blank"));
        assert!(s.add_bookmark("https://rust-lang.org/", "Rust"));
        let s2 = Store::open(dir.clone());
        assert_eq!(s2.bookmarks.len(), 2);
        assert_eq!(s2.bookmarks[0].title, "Example Domain");
        assert!(s2.is_bookmarked("https://rust-lang.org/"));
        let mut s2 = s2;
        s2.remove_bookmark("https://example.com/");
        assert_eq!(Store::open(dir.clone()).bookmarks.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("bookmarks.json")).map(|m| m.permissions().mode() & 0o777).unwrap();
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn history_dedupes_and_orders() {
        let dir = temp_dir("hist");
        let mut s = Store::open(dir.clone());
        s.record_visit("https://a.example/", "A");
        s.record_visit("https://b.example/", "B");
        s.record_visit("https://a.example/", "");
        s.record_visit("file:///etc/passwd", "nope");
        s.retitle("https://b.example/", "Bee");
        let s2 = Store::open(dir.clone());
        assert_eq!(s2.history.len(), 2);
        assert_eq!(s2.history[0].url, "https://a.example/");
        assert_eq!(s2.history[0].visits, 2);
        assert_eq!(s2.history[0].title, "A");
        assert_eq!(s2.history[1].title, "Bee");
        let mut s2 = s2;
        s2.clear_history();
        assert!(Store::open(dir.clone()).history.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn suggestions_rank_bookmarks_first() {
        let dir = temp_dir("sug");
        let mut s = Store::open(dir.clone());
        s.record_visit("https://docs.rs/gtk4", "gtk4 docs");
        s.record_visit("https://www.rust-lang.org/", "Rust");
        s.record_visit("https://www.rust-lang.org/", "Rust");
        s.add_bookmark("https://doc.rust-lang.org/book/", "The Rust Book");
        let r = s.suggest("rust", 5);
        assert_eq!(r[0].0, "https://doc.rust-lang.org/book/");
        assert!(r[0].2);
        assert_eq!(r[1].0, "https://www.rust-lang.org/");
        assert!(s.suggest("   ", 5).is_empty());
        assert_eq!(s.suggest("www", 5).len(), 0, "the www. prefix is not a match");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_files_load_as_empty() {
        let dir = temp_dir("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bookmarks.json"), b"{not json").unwrap();
        let s = Store::open(dir.clone());
        assert!(s.bookmarks.is_empty());
        assert_eq!(s.settings.search_engine, "duckduckgo");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn download_names_are_unique_and_safe() {
        let dir = Path::new("/dl");
        let taken: HashSet<PathBuf> = ["/dl/report.pdf", "/dl/report (2).pdf", "/dl/a.tar.gz", "/dl/download"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let exists = |p: &Path| taken.contains(p);
        assert_eq!(unique_download_path(dir, "new.txt", exists), PathBuf::from("/dl/new.txt"));
        assert_eq!(unique_download_path(dir, "report.pdf", exists), PathBuf::from("/dl/report (3).pdf"));
        assert_eq!(unique_download_path(dir, "a.tar.gz", exists), PathBuf::from("/dl/a (2).tar.gz"));
        assert_eq!(unique_download_path(dir, "../../etc/passwd", exists), PathBuf::from("/dl/passwd"));
        assert_eq!(unique_download_path(dir, "..\\evil.exe", exists), PathBuf::from("/dl/evil.exe"));
        assert_eq!(unique_download_path(dir, "", exists), PathBuf::from("/dl/download (2)"));
        assert_eq!(unique_download_path(dir, ".bashrc", exists), PathBuf::from("/dl/bashrc"));
    }
}
