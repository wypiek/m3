//! Shared building blocks for every m3 component: configuration schemas and
//! loading, XDG paths, and small utilities.

pub mod config;
pub mod glob;
pub mod paths;

/// Local wall-clock helpers.
pub mod time {
    use chrono::Timelike;

    /// Minutes since local midnight (honours $TZ / /etc/localtime).
    pub fn local_minutes() -> u32 {
        let now = chrono::Local::now();
        now.hour() * 60 + now.minute()
    }
}

pub use m3_protocol as protocol;

/// The m3 release version, shared by every component.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
