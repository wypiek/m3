//! The music library: scanning, tag reading (GStreamer Discoverer), cover
//! art, the on-disk index cache, and grouping into albums and artists.

use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use gstreamer as gst;
use gstreamer_pbutils as pbutils;
use gtk::prelude::FileExt;
use serde::{Deserialize, Serialize};

/// File extensions treated as audio.
pub const AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "aiff", "aif", "ape", "wv", "mka"];

const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Track {
    pub path: PathBuf,
    /// Modification time (seconds since the epoch) when the tags were read.
    pub mtime: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_ms: u64,
    /// Cover image file (embedded art is extracted into the cache).
    pub cover: Option<PathBuf>,
}

impl Track {
    pub fn display_title(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn display_artist(&self) -> &str {
        if self.artist.is_empty() { "Unknown artist" } else { &self.artist }
    }

    pub fn display_album(&self) -> &str {
        if self.album.is_empty() { "Unknown album" } else { &self.album }
    }

    /// The artist an album is filed under.
    pub fn album_owner(&self) -> &str {
        if !self.album_artist.is_empty() { &self.album_artist } else { self.display_artist() }
    }

    /// Case-insensitive match against title, artist and album.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || [self.display_title().as_str(), &self.artist, &self.album, &self.album_artist]
                .iter()
                .any(|f| f.to_lowercase().contains(&q))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Album {
    pub title: String,
    pub artist: String,
    /// Indices into the track list, in disc/track order.
    pub tracks: Vec<usize>,
    pub cover: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Artist {
    pub name: String,
    pub tracks: Vec<usize>,
    pub albums: usize,
}

/// Sort key for songs: artist, album, disc, track, title.
pub fn sort_tracks(tracks: &mut [Track]) {
    tracks.sort_by(|a, b| {
        (a.album_owner().to_lowercase(), a.display_album().to_lowercase(), a.disc_no, a.track_no, a.display_title())
            .cmp(&(
                b.album_owner().to_lowercase(),
                b.display_album().to_lowercase(),
                b.disc_no,
                b.track_no,
                b.display_title(),
            ))
    });
}

/// Group tracks into albums (by album owner + album title).
pub fn albums(tracks: &[Track]) -> Vec<Album> {
    let mut map: BTreeMap<(String, String), Album> = BTreeMap::new();
    for (i, t) in tracks.iter().enumerate() {
        let key = (t.display_album().to_lowercase(), t.album_owner().to_lowercase());
        let a = map.entry(key).or_insert_with(|| Album {
            title: t.display_album().to_owned(),
            artist: t.album_owner().to_owned(),
            tracks: Vec::new(),
            cover: None,
        });
        a.tracks.push(i);
        if a.cover.is_none() {
            a.cover = t.cover.clone();
        }
    }
    let mut out: Vec<Album> = map.into_values().collect();
    for a in &mut out {
        a.tracks.sort_by_key(|&i| (tracks[i].disc_no, tracks[i].track_no, tracks[i].display_title()));
    }
    out
}

/// Group tracks by (track) artist.
pub fn artists(tracks: &[Track]) -> Vec<Artist> {
    let mut map: BTreeMap<String, (String, Vec<usize>, Vec<String>)> = BTreeMap::new();
    for (i, t) in tracks.iter().enumerate() {
        let e = map
            .entry(t.display_artist().to_lowercase())
            .or_insert_with(|| (t.display_artist().to_owned(), Vec::new(), Vec::new()));
        e.1.push(i);
        let album = t.display_album().to_lowercase();
        if !e.2.contains(&album) {
            e.2.push(album);
        }
    }
    map.into_values().map(|(name, tracks, albums)| Artist { name, tracks, albums: albums.len() }).collect()
}

/// `mm:ss` or `h:mm:ss`.
pub fn format_time(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

fn mtime(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn is_audio(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// The folder that is scanned: the XDG music directory, else `~/Music`.
pub fn music_dir() -> PathBuf {
    if let Some(d) = gtk::glib::user_special_dir(gtk::glib::UserDirectory::Music) {
        return d;
    }
    gtk::glib::home_dir().join("Music")
}

pub fn cache_dir() -> PathBuf {
    gtk::glib::user_cache_dir().join("m3").join("music")
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    root: PathBuf,
    tracks: Vec<Track>,
}

/// Serialise the index (tested separately from the file system).
pub fn encode_cache(root: &Path, tracks: &[Track]) -> Result<String, serde_json::Error> {
    serde_json::to_string(&CacheFile { version: CACHE_VERSION, root: root.to_owned(), tracks: tracks.to_vec() })
}

/// Parse a cache; `None` if it is for another folder or another version.
pub fn decode_cache(root: &Path, data: &str) -> Option<Vec<Track>> {
    let c: CacheFile = serde_json::from_str(data).ok()?;
    (c.version == CACHE_VERSION && c.root == root).then_some(c.tracks)
}

pub fn load_cache(root: &Path) -> Option<Vec<Track>> {
    let data = std::fs::read_to_string(cache_dir().join("library.json")).ok()?;
    decode_cache(root, &data)
}

pub fn save_cache(root: &Path, tracks: &[Track]) {
    let dir = cache_dir();
    let res = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| {
        let data = encode_cache(root, tracks).map_err(|e| e.to_string())?;
        let tmp = dir.join("library.json.tmp");
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join("library.json")).map_err(|e| e.to_string())
    });
    if let Err(e) = res {
        tracing::warn!("cannot write the music index: {e}");
    }
}

/// Recursively collect audio files under `root` (symlinked directories are
/// not followed, to avoid loops).
pub fn find_audio_files(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                if !entry.file_name().to_string_lossy().starts_with('.') {
                    stack.push(path);
                }
            } else if is_audio(&path)
                && let Ok(meta) = std::fs::metadata(&path)
            {
                out.push((path, mtime(&meta)));
            }
        }
    }
    out.sort();
    out
}

/// A cover image next to the file (cover.jpg, folder.png, front.jpg, …).
pub fn folder_cover(file: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    let mut best: Option<(u8, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        let ext = p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
            continue;
        }
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let rank = match stem.as_str() {
            "cover" => 0,
            "folder" => 1,
            "front" => 2,
            "album" => 3,
            _ => continue,
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, p));
        }
    }
    best.map(|(_, p)| p)
}

fn hash_of(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Read tags and duration of one file. Embedded cover art is written to the
/// cache (deduplicated per album).
pub fn read_track(disc: &pbutils::Discoverer, path: &Path, mtime: u64) -> Track {
    let mut t = Track { path: path.to_owned(), mtime, ..Default::default() };
    let uri = gtk::gio::File::for_path(path).uri();
    let info = match disc.discover_uri(&uri) {
        Ok(i) => i,
        Err(e) => {
            tracing::debug!("cannot read {}: {e}", path.display());
            t.cover = folder_cover(path);
            return t;
        }
    };
    t.duration_ms = info.duration().map(|d| d.mseconds()).unwrap_or(0);
    let mut embedded: Option<(Vec<u8>, String)> = None;
    if let Some(tags) = info.tags() {
        use gst::tags;
        t.title = tags.get::<tags::Title>().map(|v| v.get().to_owned()).unwrap_or_default();
        t.artist = tags.get::<tags::Artist>().map(|v| v.get().to_owned()).unwrap_or_default();
        t.album = tags.get::<tags::Album>().map(|v| v.get().to_owned()).unwrap_or_default();
        t.album_artist = tags.get::<tags::AlbumArtist>().map(|v| v.get().to_owned()).unwrap_or_default();
        t.track_no = tags.get::<tags::TrackNumber>().map(|v| v.get());
        t.disc_no = tags.get::<tags::AlbumVolumeNumber>().map(|v| v.get());
        let sample = tags.get::<tags::Image>().or_else(|| tags.get::<tags::PreviewImage>()).map(|v| v.get());
        if let Some(sample) = sample
            && let Some(buf) = sample.buffer()
            && let Ok(map) = buf.map_readable()
        {
            let mime = sample
                .caps()
                .and_then(|c| c.structure(0).map(|s| s.name().to_string()))
                .unwrap_or_else(|| "image/jpeg".into());
            embedded = Some((map.as_slice().to_vec(), mime));
        }
    }
    t.cover = folder_cover(path).or_else(|| {
        let (bytes, mime) = embedded?;
        let ext = if mime.contains("png") { "png" } else { "jpg" };
        let key = format!("{}\u{1}{}", t.album_owner(), t.display_album());
        let dir = cache_dir().join("covers");
        std::fs::create_dir_all(&dir).ok()?;
        let file = dir.join(format!("{:016x}.{ext}", hash_of(&key)));
        if !file.exists() {
            std::fs::write(&file, &bytes).ok()?;
        }
        Some(file)
    });
    t
}

/// Progress reported by [`scan`].
pub enum ScanEvent {
    Progress { done: usize, total: usize },
    Done(Vec<Track>),
    Failed(String),
}

/// Scan `root`, reusing cached tags for unchanged files. Runs on the
/// calling (background) thread.
pub fn scan(root: &Path, cached: &[Track], report: &(dyn Fn(ScanEvent) + Sync)) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let started = std::time::Instant::now();
    let files = find_audio_files(root);
    let known: HashMap<&Path, &Track> = cached.iter().map(|t| (t.path.as_path(), t)).collect();
    let total = files.len();
    // Unchanged files come straight from the cache; the rest are read by a
    // few workers in parallel, each with its own Discoverer.
    let mut tracks: Vec<Option<Track>> = files
        .iter()
        .map(|(path, mtime)| match known.get(path.as_path()) {
            Some(t) if t.mtime == *mtime && t.cover.as_ref().is_none_or(|c| c.exists()) => Some((*t).clone()),
            _ => None,
        })
        .collect();
    let todo: Vec<usize> = (0..total).filter(|&i| tracks[i].is_none()).collect();
    let done = AtomicUsize::new(total - todo.len());
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Track)>> = Mutex::new(Vec::with_capacity(todo.len()));
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 4).min(todo.len().max(1));
    let running = AtomicUsize::new(workers);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                /// Counts the worker out however it exits.
                struct Exit<'a>(&'a AtomicUsize);
                impl Drop for Exit<'_> {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                let _exit = Exit(&running);
                let disc = match pbutils::Discoverer::new(gst::ClockTime::from_seconds(8)) {
                    Ok(d) => d,
                    Err(e) => {
                        if let Ok(mut f) = failed.lock() {
                            *f = Some(format!("Cannot read audio tags: {e}"));
                        }
                        return;
                    }
                };
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&i) = todo.get(k) else { break };
                    let (path, mtime) = &files[i];
                    let t = read_track(&disc, path, *mtime);
                    if let Ok(mut r) = results.lock() {
                        r.push((i, t));
                    }
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        // Progress while the workers run.
        while running.load(Ordering::Relaxed) > 0 {
            std::thread::sleep(Duration::from_millis(150));
            report(ScanEvent::Progress { done: done.load(Ordering::Relaxed), total });
        }
    });
    if let Some(msg) = failed.into_inner().ok().flatten()
        && done.load(Ordering::Relaxed) < total
    {
        report(ScanEvent::Failed(msg));
        return;
    }
    for (i, t) in results.into_inner().unwrap_or_default() {
        tracks[i] = Some(t);
    }
    let mut tracks: Vec<Track> = tracks.into_iter().flatten().collect();
    sort_tracks(&mut tracks);
    save_cache(root, &tracks);
    tracing::info!(
        "library: {} tracks, {} read, scan took {} ms",
        tracks.len(),
        todo.len(),
        started.elapsed().as_millis()
    );
    report(ScanEvent::Done(tracks));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(title: &str, artist: &str, album: &str, n: u32) -> Track {
        Track {
            path: PathBuf::from(format!("/m/{artist}/{album}/{title}.ogg")),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            track_no: Some(n),
            ..Default::default()
        }
    }

    #[test]
    fn time_format() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(65_400), "1:05");
        assert_eq!(format_time(3_725_000), "1:02:05");
    }

    #[test]
    fn grouping() {
        let mut v = vec![t("B", "Zed", "Two", 2), t("A", "Zed", "Two", 1), t("C", "Amy", "One", 1)];
        sort_tracks(&mut v);
        assert_eq!(v.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["C", "A", "B"]);
        let al = albums(&v);
        assert_eq!(al.len(), 2);
        let two = al.iter().find(|a| a.title == "Two").map(|a| a.tracks.clone());
        assert_eq!(two, Some(vec![1, 2]), "album tracks in track-number order");
        let ar = artists(&v);
        assert_eq!(
            ar.iter().map(|a| (a.name.as_str(), a.tracks.len(), a.albums)).collect::<Vec<_>>(),
            [("Amy", 1, 1), ("Zed", 2, 1)]
        );
    }

    #[test]
    fn display_fallbacks_and_search() {
        let x = Track { path: "/m/song name.flac".into(), ..Default::default() };
        assert_eq!(x.display_title(), "song name");
        assert_eq!((x.display_artist(), x.display_album()), ("Unknown artist", "Unknown album"));
        let y = t("Blue Monday", "New Order", "Power", 1);
        assert!(y.matches("monday") && y.matches("ORDER") && y.matches("") && !y.matches("jazz"));
        assert!(is_audio(Path::new("a/b.FLAC")) && !is_audio(Path::new("cover.jpg")));
    }

    #[test]
    fn cache_roundtrip() {
        let root = Path::new("/home/me/Music");
        let v = vec![t("A", "B", "C", 1)];
        let s = encode_cache(root, &v).unwrap();
        assert_eq!(decode_cache(root, &s), Some(v));
        assert_eq!(decode_cache(Path::new("/elsewhere"), &s), None, "cache is per folder");
        assert_eq!(decode_cache(root, "{not json"), None);
    }

    #[test]
    fn folder_scan_and_cover() {
        let dir = std::env::temp_dir().join(format!("m3-music-test-{}", std::process::id()));
        let album = dir.join("Artist/Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("01.ogg"), b"x").unwrap();
        std::fs::write(album.join("notes.txt"), b"x").unwrap();
        std::fs::write(album.join("front.png"), b"x").unwrap();
        std::fs::write(album.join("Cover.JPG"), b"x").unwrap();
        let found = find_audio_files(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(folder_cover(&album.join("01.ogg")), Some(album.join("Cover.JPG")), "cover beats front");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
