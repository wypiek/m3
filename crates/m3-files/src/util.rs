//! Pure helpers (unit-tested): path bar segments, `~` expansion, sorting,
//! "keep both" names, sizes, bookmarks, URI lists, trash info files.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Path bar segments for `path`: `(label, path)`. Paths under `home` start
/// with a "Home" segment; others with "Computer" (`/`).
pub fn path_segments(path: &Path, home: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let (mut cur, rest) = match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => {
            out.push(("Home".to_owned(), home.to_path_buf()));
            (home.to_path_buf(), rest.to_path_buf())
        }
        _ => {
            out.push(("Computer".to_owned(), PathBuf::from("/")));
            (PathBuf::from("/"), path.strip_prefix("/").unwrap_or(path).to_path_buf())
        }
    };
    for c in rest.components() {
        if let std::path::Component::Normal(n) = c {
            cur.push(n);
            out.push((n.to_string_lossy().into_owned(), cur.clone()));
        }
    }
    out
}

/// Expand a leading `~` / `~/` with `home`; trims whitespace.
pub fn expand_tilde(input: &str, home: &Path) -> PathBuf {
    let s = input.trim();
    if s == "~" {
        home.to_path_buf()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Type,
}

impl SortKey {
    pub fn id(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "modified",
            SortKey::Type => "type",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        Some(match s {
            "name" => SortKey::Name,
            "size" => SortKey::Size,
            "modified" => SortKey::Modified,
            "type" => SortKey::Type,
            _ => return None,
        })
    }
}

/// The parts of a file the sorters look at.
#[derive(Debug, Clone, PartialEq)]
pub struct SortEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub modified: i64,
    pub content_type: String,
}

/// Natural, case-insensitive name order ("file2" < "file10").
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Compare two entries for the view. Folders always come first when
/// `folders_first`; `descending` reverses the key order (not the folder
/// grouping). Ties fall back to the name.
pub fn compare(a: &SortEntry, b: &SortEntry, key: SortKey, descending: bool, folders_first: bool) -> Ordering {
    if folders_first && a.is_dir != b.is_dir {
        return if a.is_dir { Ordering::Less } else { Ordering::Greater };
    }
    let by_name = natural_cmp(&a.name, &b.name);
    let o = match key {
        SortKey::Name => by_name,
        SortKey::Size => a.size.cmp(&b.size).then(by_name),
        SortKey::Modified => a.modified.cmp(&b.modified).then(by_name),
        SortKey::Type => a.content_type.cmp(&b.content_type).then(by_name),
    };
    if descending { o.reverse() } else { o }
}

/// "Keep both": `file.txt` → `file (2).txt`, `file (2).txt` → `file (3).txt`;
/// `exists` says whether a candidate name is taken.
pub fn keep_both_name(name: &str, exists: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = split_ext(name);
    // Continue an existing " (n)" counter.
    let (base, start) = match stem.rsplit_once(" (") {
        Some((b, n)) if n.ends_with(')') => match n[..n.len() - 1].parse::<u32>() {
            Ok(k) => (b.to_owned(), k + 1),
            Err(_) => (stem.to_owned(), 2),
        },
        _ => (stem.to_owned(), 2),
    };
    let mut k = start;
    loop {
        let candidate = format!("{base} ({k}){ext}");
        if !exists(&candidate) {
            return candidate;
        }
        k += 1;
    }
}

/// Split `name` into stem and extension (`.tar.gz` kept together; dotfiles
/// have no extension).
pub fn split_ext(name: &str) -> (&str, &str) {
    for double in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if name.len() > double.len() && name.to_ascii_lowercase().ends_with(double) {
            let i = name.len() - double.len();
            return (&name[..i], &name[i..]);
        }
    }
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i..]),
    }
}

/// `1.5 MB`-style sizes (SI units, like GNOME).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["bytes", "kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return if bytes == 1 { "1 byte".to_owned() } else { format!("{bytes} bytes") };
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// Validate a new file name (rename / new folder). `Err` explains why.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() {
        return Err("The name can't be empty.");
    }
    if name.contains('/') {
        return Err("Names can't contain “/”.");
    }
    if name == "." || name == ".." {
        return Err("“.” and “..” are reserved names.");
    }
    if name.len() > 255 {
        return Err("The name is too long.");
    }
    Ok(())
}

/// A GTK bookmark: URI and optional label.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub uri: String,
    pub label: Option<String>,
}

/// Parse `~/.config/gtk-3.0/bookmarks` (`uri [label]` per line).
pub fn parse_bookmarks(text: &str) -> Vec<Bookmark> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() {
                return None;
            }
            let (uri, label) = match l.split_once(' ') {
                Some((u, lab)) => (u, Some(lab.trim().to_owned()).filter(|s| !s.is_empty())),
                None => (l, None),
            };
            Some(Bookmark { uri: uri.to_owned(), label })
        })
        .collect()
}

pub fn serialize_bookmarks(b: &[Bookmark]) -> String {
    let mut s = String::new();
    for bm in b {
        s.push_str(&bm.uri);
        if let Some(l) = &bm.label {
            s.push(' ');
            s.push_str(l);
        }
        s.push('\n');
    }
    s
}

/// `text/uri-list` (RFC 2483): CRLF-separated URIs.
pub fn encode_uri_list(uris: &[String]) -> String {
    uris.iter().map(|u| format!("{u}\r\n")).collect()
}

pub fn decode_uri_list(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect()
}

/// `x-special/gnome-copied-files`: `copy|cut` then one URI per line.
pub fn encode_gnome_copied(cut: bool, uris: &[String]) -> String {
    let mut s = String::from(if cut { "cut" } else { "copy" });
    for u in uris {
        s.push('\n');
        s.push_str(u);
    }
    s
}

pub fn decode_gnome_copied(text: &str) -> Option<(bool, Vec<String>)> {
    let mut lines = text.lines();
    let cut = match lines.next()?.trim() {
        "cut" => true,
        "copy" => false,
        _ => return None,
    };
    Some((cut, lines.map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect()))
}

/// Percent-decode (URIs in `.trashinfo` files).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A parsed `.trashinfo`: original path and deletion date.
#[derive(Debug, Clone, PartialEq)]
pub struct TrashInfo {
    pub path: PathBuf,
    pub deletion_date: String,
}

pub fn parse_trashinfo(text: &str) -> Option<TrashInfo> {
    let mut in_section = false;
    let (mut path, mut date) = (None, String::new());
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_section = l == "[Trash Info]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(v) = l.strip_prefix("Path=") {
            path = Some(PathBuf::from(percent_decode(v)));
        } else if let Some(v) = l.strip_prefix("DeletionDate=") {
            date = v.to_owned();
        }
    }
    Some(TrashInfo { path: path?, deletion_date: date })
}

/// Recent-search match: every whitespace-separated word appears in the name
/// (case-insensitive).
pub fn search_matches(name: &str, query: &str) -> bool {
    let n = name.to_lowercase();
    let mut any = false;
    for w in query.split_whitespace() {
        any = true;
        if !n.contains(&w.to_lowercase()) {
            return false;
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        let home = Path::new("/home/me");
        let s = path_segments(Path::new("/home/me/Documents/Work"), home);
        let labels: Vec<&str> = s.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Home", "Documents", "Work"]);
        assert_eq!(s[2].1, PathBuf::from("/home/me/Documents/Work"));
        let s = path_segments(Path::new("/usr/share"), home);
        assert_eq!(s[0], ("Computer".to_owned(), PathBuf::from("/")));
        assert_eq!(s[2].1, PathBuf::from("/usr/share"));
        assert_eq!(path_segments(Path::new("/"), home).len(), 1);
    }

    #[test]
    fn tilde() {
        let h = Path::new("/home/me");
        assert_eq!(expand_tilde("~", h), PathBuf::from("/home/me"));
        assert_eq!(expand_tilde(" ~/Music ", h), PathBuf::from("/home/me/Music"));
        assert_eq!(expand_tilde("/etc", h), PathBuf::from("/etc"));
        assert_eq!(expand_tilde("~other", h), PathBuf::from("~other"));
    }

    fn e(name: &str, dir: bool, size: i64, modified: i64) -> SortEntry {
        SortEntry { name: name.into(), is_dir: dir, size, modified, content_type: String::new() }
    }

    #[test]
    fn sorting() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
        let (dir, small, big) = (e("zeta", true, 0, 0), e("a", false, 10, 5), e("b", false, 99, 1));
        assert_eq!(compare(&dir, &small, SortKey::Name, false, true), Ordering::Less);
        assert_eq!(compare(&dir, &small, SortKey::Name, false, false), Ordering::Greater);
        assert_eq!(compare(&small, &big, SortKey::Size, false, true), Ordering::Less);
        assert_eq!(compare(&small, &big, SortKey::Size, true, true), Ordering::Greater);
        assert_eq!(compare(&small, &big, SortKey::Modified, false, true), Ordering::Greater);
        // Descending keeps folders first.
        assert_eq!(compare(&dir, &small, SortKey::Name, true, true), Ordering::Less);
    }

    #[test]
    fn keep_both() {
        let taken = ["file (2).txt", "file (3).txt"];
        assert_eq!(keep_both_name("file.txt", |c| taken.contains(&c)), "file (4).txt");
        assert_eq!(keep_both_name("file (2).txt", |_| false), "file (3).txt");
        assert_eq!(keep_both_name("Folder", |_| false), "Folder (2)");
        assert_eq!(keep_both_name("x.tar.gz", |_| false), "x (2).tar.gz");
        assert_eq!(keep_both_name(".bashrc", |_| false), ".bashrc (2)");
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(1), "1 byte");
        assert_eq!(format_size(999), "999 bytes");
        assert_eq!(format_size(1500), "1.5 kB");
        assert_eq!(format_size(3_200_000_000), "3.2 GB");
    }

    #[test]
    fn names() {
        assert!(validate_name("ok.txt").is_ok());
        assert!(validate_name("  ").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
    }

    #[test]
    fn bookmarks_roundtrip() {
        let text = "file:///home/me/Work Work stuff\nfile:///tmp\n\n";
        let b = parse_bookmarks(text);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].label.as_deref(), Some("Work stuff"));
        assert_eq!(b[1].label, None);
        assert_eq!(parse_bookmarks(&serialize_bookmarks(&b)), b);
    }

    #[test]
    fn uri_lists() {
        let uris = vec!["file:///a%20b".to_owned(), "file:///c".to_owned()];
        assert_eq!(encode_uri_list(&uris), "file:///a%20b\r\nfile:///c\r\n");
        assert_eq!(decode_uri_list(&encode_uri_list(&uris)), uris);
        assert_eq!(decode_uri_list("# comment\r\nfile:///x\r\n"), vec!["file:///x".to_owned()]);
        let g = encode_gnome_copied(true, &uris);
        assert_eq!(decode_gnome_copied(&g), Some((true, uris)));
        assert_eq!(decode_gnome_copied("move\nfile:///x"), None);
    }

    #[test]
    fn trashinfo() {
        let t = "[Trash Info]\nPath=/home/me/My%20File.txt\nDeletionDate=2026-11-10T19:00:00\n";
        let i = parse_trashinfo(t).expect("parses");
        assert_eq!(i.path, PathBuf::from("/home/me/My File.txt"));
        assert_eq!(i.deletion_date, "2026-11-10T19:00:00");
        assert!(parse_trashinfo("[Other]\nPath=/x\n").is_none());
        assert_eq!(percent_decode("a%2Fb%zz%"), "a/b%zz%");
    }

    #[test]
    fn search() {
        assert!(search_matches("Holiday Photos 2024.jpg", "photo 2024"));
        assert!(!search_matches("Holiday.jpg", "photo"));
        assert!(!search_matches("x", "  "));
    }
}
