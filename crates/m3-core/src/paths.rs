//! XDG base-directory resolution for m3.

use std::path::PathBuf;

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        // The spec requires ignoring relative paths.
        Some(p) if PathBuf::from(&p).is_absolute() => Some(PathBuf::from(p)),
        _ => home().map(|h| h.join(fallback)),
    }
}

/// `~/.config/m3` (honours `$M3_CONFIG_DIR` for tests and `$XDG_CONFIG_HOME`).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("M3_CONFIG_DIR") {
        return Some(PathBuf::from(p));
    }
    xdg("XDG_CONFIG_HOME", ".config").map(|p| p.join("m3"))
}

/// `~/.local/state/m3` — logs, recently-used lists, notification history.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("M3_STATE_DIR") {
        return Some(PathBuf::from(p));
    }
    xdg("XDG_STATE_HOME", ".local/state").map(|p| p.join("m3"))
}

/// `~/.cache/m3` — generated theme CSS, thumbnails.
pub fn cache_dir() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache").map(|p| p.join("m3"))
}

/// `~/.local/share/m3`.
pub fn data_dir() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share").map(|p| p.join("m3"))
}

/// System data directories (`$XDG_DATA_DIRS`), most important first.
pub fn system_data_dirs() -> Vec<PathBuf> {
    let raw = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    raw.split(':').map(PathBuf::from).filter(|p| p.is_absolute()).collect()
}

/// `$XDG_RUNTIME_DIR` — must exist and be owned by the user per the spec.
pub fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute())
}
