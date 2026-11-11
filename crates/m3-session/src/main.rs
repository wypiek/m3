//! `m3-session` — what the display manager runs (see
//! `/usr/share/wayland-sessions/m3.desktop`).
//!
//! It prepares a Wayland session environment and replaces itself with the
//! compositor in session mode (`m3 --session`), which then exports the
//! environment to systemd/D-Bus, starts `m3-session.target` (pulling in
//! `graphical-session.target`), XDG autostart, and supervises the shell.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

/// Defaults that make toolkits pick Wayland; never overrides user choices.
const DEFAULTS: &[(&str, &str)] = &[
    ("XDG_CURRENT_DESKTOP", "m3"),
    ("XDG_SESSION_DESKTOP", "m3"),
    ("XDG_SESSION_TYPE", "wayland"),
    ("MOZ_ENABLE_WAYLAND", "1"),
    ("QT_QPA_PLATFORM", "wayland;xcb"),
    ("ELECTRON_OZONE_PLATFORM_HINT", "auto"),
    ("_JAVA_AWT_WM_NONREPARENTING", "1"),
];

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("m3-session {}", m3_core::VERSION);
        return ExitCode::SUCCESS;
    }
    if rustix_is_root() {
        eprintln!("m3-session: refusing to start a desktop session as root");
        return ExitCode::FAILURE;
    }
    // Display managers usually discard session output; keep m3's logs in the
    // journal (`journalctl -t m3`), or in a file.
    let mut cmd = if in_path("systemd-cat") {
        let mut c = Command::new("systemd-cat");
        c.args(["--identifier=m3", "--", "m3", "--session"]);
        c
    } else {
        let mut c = Command::new("m3");
        c.arg("--session");
        if let Some(f) = log_file() {
            if let Ok(err) = f.try_clone() {
                c.stdout(f).stderr(err);
            }
        }
        c
    };
    for (k, v) in DEFAULTS {
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
    // Leftovers from another session must not leak in.
    cmd.env_remove("WAYLAND_DISPLAY").env_remove("DISPLAY").env_remove("M3_SOCKET");
    let err = cmd.exec();
    eprintln!("m3-session: cannot start the compositor `m3`: {err}");
    ExitCode::FAILURE
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// `~/.local/state/m3/session.log`, keeping the previous one as `.old`.
fn log_file() -> Option<std::fs::File> {
    let dir = m3_core::paths::state_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("session.log");
    let _ = std::fs::rename(&path, dir.join("session.log.old"));
    std::fs::File::create(path).ok()
}

fn rustix_is_root() -> bool {
    // Avoid a dependency for one syscall: /proc/self/status has the real UID.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(1) == Some("0")))
        .unwrap_or(false)
}
