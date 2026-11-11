//! Playback state kept across restarts (`~/.local/state/m3/music.json`):
//! volume, shuffle, repeat, the queue and the position in the current track.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::queue::Repeat;

const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub version: u32,
    pub volume: f64,
    pub shuffle: bool,
    /// MPRIS loop status: None / Playlist / Track.
    pub repeat: String,
    /// The queue in its natural order.
    pub queue: Vec<PathBuf>,
    /// Index into `queue` of the current track.
    pub current: Option<usize>,
    pub position_ms: u64,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            version: VERSION,
            volume: 1.0,
            shuffle: false,
            repeat: Repeat::Off.mpris().into(),
            queue: Vec::new(),
            current: None,
            position_ms: 0,
        }
    }
}

impl Saved {
    pub fn repeat(&self) -> Repeat {
        Repeat::from_mpris(&self.repeat).unwrap_or_default()
    }
}

pub fn path() -> PathBuf {
    gtk::glib::user_state_dir().join("m3").join("music.json")
}

/// Parse a state file; unknown versions and broken files give `None`.
pub fn decode(data: &str) -> Option<Saved> {
    let mut s: Saved = serde_json::from_str(data).ok()?;
    if s.version != VERSION {
        return None;
    }
    s.volume = if s.volume.is_finite() { s.volume.clamp(0.0, 1.0) } else { 1.0 };
    if s.current.is_some_and(|c| c >= s.queue.len()) {
        s.current = None;
    }
    Some(s)
}

pub fn load_from(path: &Path) -> Option<Saved> {
    decode(&std::fs::read_to_string(path).ok()?)
}

/// Write atomically (temporary file + rename), owner-only.
pub fn save_to(path: &Path, s: &Saved) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let data = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    f.write_all(data.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_validation() {
        let dir = std::env::temp_dir().join(format!("m3-music-state-{}", std::process::id()));
        let p = dir.join("m3/music.json");
        let s = Saved {
            volume: 0.4,
            shuffle: true,
            repeat: "Track".into(),
            queue: vec!["/m/a.ogg".into(), "/m/b.ogg".into()],
            current: Some(1),
            position_ms: 61_000,
            ..Default::default()
        };
        save_to(&p, &s).unwrap();
        assert_eq!(load_from(&p), Some(s.clone()));
        assert_eq!(s.repeat(), Repeat::One);
        assert!(!p.with_extension("json.tmp").exists(), "temporary file is renamed away");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(decode("{broken"), None);
        let bad = serde_json::to_string(&Saved { volume: 7.0, current: Some(5), ..Default::default() }).unwrap();
        let fixed = decode(&bad).unwrap();
        assert_eq!((fixed.volume, fixed.current), (1.0, None), "values are clamped/validated");
        let future = serde_json::to_string(&Saved { version: 99, ..Default::default() }).unwrap();
        assert_eq!(decode(&future), None);
    }
}
