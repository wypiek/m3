//! The m3 compositor IPC protocol.
//!
//! Transport: a Unix stream socket owned by the user (mode `0600`) located at
//! [`socket_path`]. Messages are newline-delimited JSON. A client writes one
//! [`Request`] per line and receives exactly one [`Reply`] line in response.
//!
//! Sending [`Request::Subscribe`] turns the connection into an event stream:
//! after the `Ok` reply, the compositor writes one [`Event`] per line until the
//! client disconnects.
//!
//! The protocol is versioned with [`PROTOCOL_VERSION`]; additive changes do not
//! bump it, incompatible ones do.

mod action;
mod client;

pub use action::{Action, Direction};
pub use client::{Client, ClientError, EventStream};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Incremented on incompatible protocol changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Environment variable through which the compositor advertises its socket.
pub const SOCKET_ENV: &str = "M3_SOCKET";

/// Default location of the compositor socket for a given Wayland display name.
///
/// `$XDG_RUNTIME_DIR/m3/<wayland-display>.sock`
pub fn socket_path_for(runtime_dir: &std::path::Path, wayland_display: &str) -> PathBuf {
    runtime_dir.join("m3").join(format!("{wayland_display}.sock"))
}

/// Resolve the socket of the running compositor: `$M3_SOCKET` first, then the
/// path derived from `$XDG_RUNTIME_DIR` and `$WAYLAND_DISPLAY`.
pub fn socket_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(SOCKET_ENV) {
        return Some(PathBuf::from(p));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(socket_path_for(std::path::Path::new(&runtime), &display))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// Compositor and protocol version.
    Version,
    /// Re-read configuration files from disk.
    Reload,
    /// Turn this connection into an event stream.
    Subscribe,
    Workspaces,
    Windows,
    Outputs,
    /// Information about the currently focused window, if any.
    FocusedWindow,
    /// Perform a compositor action (the same actions keybindings use).
    Action {
        action: Action,
    },
    /// Configure an output.
    ConfigureOutput {
        output: String,
        change: OutputChange,
    },
    /// Write a PNG screenshot of an output (or the first output) to `path`.
    /// The path must be absolute; the file is created with mode `0600`.
    Screenshot {
        output: Option<String>,
        path: PathBuf,
        /// `[x, y, width, height]` in logical pixels relative to the
        /// output; the whole output when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<[i32; 4]>,
        /// Announce it with [`Event::ScreenshotSaved`] (the shell shows a
        /// notification); tools like `m3ctl screenshot` leave this off.
        #[serde(default)]
        notify: bool,
    },
    /// Synthetic input for automated testing. Only honoured when the
    /// compositor was started with `--test-mode`.
    TestInput {
        input: TestInput,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum OutputChange {
    Scale { scale: f64 },
    Transform { transform: Transform },
    Position { x: i32, y: i32 },
    Mode { width: i32, height: i32, refresh_mhz: Option<i32> },
    Enabled { enabled: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    #[default]
    Normal,
    #[serde(rename = "90")]
    Rotate90,
    #[serde(rename = "180")]
    Rotate180,
    #[serde(rename = "270")]
    Rotate270,
    Flipped,
    #[serde(rename = "flipped_90")]
    Flipped90,
    #[serde(rename = "flipped_180")]
    Flipped180,
    #[serde(rename = "flipped_270")]
    Flipped270,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum TestInput {
    /// Absolute pointer position in global logical coordinates.
    PointerMotion { x: f64, y: f64 },
    /// Linux evdev button code (BTN_LEFT = 0x110).
    PointerButton { button: u32, pressed: bool },
    /// Linux evdev key code (without the +8 xkb offset).
    Key { keycode: u32, pressed: bool },
}

pub type Reply = Result<Response, String>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Handled,
    Version { compositor: String, protocol: u32 },
    Workspaces { workspaces: Vec<WorkspaceInfo> },
    Windows { windows: Vec<WindowInfo> },
    Outputs { outputs: Vec<OutputInfo> },
    FocusedWindow { window: Option<WindowInfo> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    /// Stable 1-based index.
    pub index: usize,
    pub name: String,
    pub active: bool,
    pub window_count: usize,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u64,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub workspace: usize,
    pub focused: bool,
    pub minimized: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub floating: bool,
    pub geometry: Rect,
    pub xwayland: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub make: String,
    pub model: String,
    pub enabled: bool,
    pub current_mode: Option<Mode>,
    pub modes: Vec<Mode>,
    pub scale: f64,
    pub transform: Transform,
    /// Logical geometry in the global compositor space.
    pub logical: Rect,
    pub physical_size_mm: (i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
    pub preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    WorkspacesChanged {
        workspaces: Vec<WorkspaceInfo>,
    },
    WindowsChanged {
        windows: Vec<WindowInfo>,
    },
    FocusChanged {
        window: Option<u64>,
    },
    OutputsChanged {
        outputs: Vec<OutputInfo>,
    },
    ConfigReloaded,
    /// A screenshot was written (keybinding or request).
    ScreenshotSaved {
        path: PathBuf,
    },
    /// A shell-level action (launcher, overview, …) requested through a
    /// keybinding. The compositor does not implement these itself; the shell
    /// acts on them.
    ShellAction {
        action: Action,
    },
    /// A window asked for its window menu (title-bar right-click). The shell
    /// shows it on `output` at (`x`, `y`) logical pixels from the output's
    /// top-left corner.
    WindowMenu {
        id: u64,
        output: Option<String>,
        x: f64,
        y: f64,
        maximized: bool,
        fullscreen: bool,
        /// 1-based, like [`WorkspaceInfo::index`].
        workspace: usize,
    },
    /// Session lock state changed.
    LockChanged {
        locked: bool,
    },
}

/// Encode a message as one JSON line (including the trailing newline).
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut buf = serde_json::to_vec(msg)?;
    buf.push(b'\n');
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let reqs = vec![
            Request::Version,
            Request::Action { action: Action::SwitchWorkspace { index: 3 } },
            Request::Action { action: Action::Spawn { command: vec!["foot".into()] } },
            Request::ConfigureOutput { output: "DP-1".into(), change: OutputChange::Scale { scale: 1.5 } },
            Request::TestInput { input: TestInput::Key { keycode: 28, pressed: true } },
        ];
        for r in reqs {
            let line = encode(&r).unwrap();
            assert_eq!(*line.last().unwrap(), b'\n');
            let back: Request = serde_json::from_slice(&line).unwrap();
            assert_eq!(back, r);
        }
    }

    #[test]
    fn reply_shape_is_stable() {
        let reply: Reply = Ok(Response::Handled);
        assert_eq!(serde_json::to_string(&reply).unwrap(), r#"{"Ok":{"response":"handled"}}"#);
        let reply: Reply = Err("nope".into());
        assert_eq!(serde_json::to_string(&reply).unwrap(), r#"{"Err":"nope"}"#);
    }

    #[test]
    fn transform_names() {
        assert_eq!(serde_json::to_string(&Transform::Rotate90).unwrap(), "\"90\"");
        assert_eq!(serde_json::from_str::<Transform>("\"flipped_270\"").unwrap(), Transform::Flipped270);
    }

    #[test]
    fn socket_path_layout() {
        let p = socket_path_for(std::path::Path::new("/run/user/1000"), "wayland-1");
        assert_eq!(p, PathBuf::from("/run/user/1000/m3/wayland-1.sock"));
    }
}
