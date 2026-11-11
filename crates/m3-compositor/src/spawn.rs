//! Launching programs.
//!
//! Commands are executed directly from an argv — never through a shell — in
//! their own process group (so terminal signals aimed at the compositor do
//! not reach them). Children are reaped through a pidfd registered with the
//! event loop, so no zombie processes accumulate and nothing polls.

use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use smithay::reexports::calloop::{Interest, Mode, PostAction, generic::Generic};
use tracing::{info, warn};

use crate::state::M3;

/// Called with the compositor state and whether the child exited cleanly.
type OnExit = Box<dyn FnOnce(&mut crate::state::State, bool)>;

pub fn spawn(m3: &M3, argv: &[String]) {
    spawn_with(m3, argv, None);
}

/// `m3ctl restart`: stop the shell (the supervisor starts a fresh one) or
/// start it if it is not running.
pub fn restart_shell(m3: &mut M3) {
    m3.shell_restarts.clear();
    match m3.shell_pid.take() {
        Some(pid) => {
            info!("restarting m3-shell");
            if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
                let _ = rustix::process::kill_process(p, rustix::process::Signal::TERM);
            }
        }
        None => supervise_shell(m3),
    }
}

/// Spawn and, when the child exits, run `on_exit` with the compositor state.
fn spawn_with(m3: &M3, argv: &[String], on_exit: Option<OnExit>) -> Option<u32> {
    let Some((program, args)) = argv.split_first() else {
        warn!("spawn: empty command");
        return None;
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .env("WAYLAND_DISPLAY", &m3.socket_name)
        .env("XDG_CURRENT_DESKTOP", "m3")
        .env("XDG_SESSION_TYPE", "wayland")
        .process_group(0);
    match m3.xdisplay {
        Some(d) => {
            cmd.env("DISPLAY", format!(":{d}"));
        }
        None => {
            cmd.env_remove("DISPLAY");
        }
    }
    if let Some(ipc) = &m3.ipc {
        cmd.env(m3_protocol::SOCKET_ENV, ipc.path());
    }
    match cmd.spawn() {
        Ok(child) => {
            let pid = child.id();
            info!(pid, %program, "spawned");
            reap(m3, child, on_exit);
            Some(pid)
        }
        Err(e) => {
            warn!("failed to start `{program}`: {e}");
            None
        }
    }
}

fn reap(m3: &M3, mut child: std::process::Child, on_exit: Option<OnExit>) {
    let mut on_exit = on_exit;
    let pid = rustix::process::Pid::from_raw(child.id() as i32);
    let pidfd: Option<OwnedFd> =
        pid.and_then(|p| rustix::process::pidfd_open(p, rustix::process::PidfdFlags::empty()).ok());
    match pidfd {
        Some(fd) => {
            let r = m3.loop_handle.insert_source(Generic::new(fd, Interest::READ, Mode::Level), move |_, _, state| {
                // The pidfd is readable once the process exited.
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::debug!(pid = child.id(), %status, "child exited");
                        if let Some(f) = on_exit.take() {
                            f(state, status.success());
                        }
                        Ok(PostAction::Remove)
                    }
                    Ok(None) => Ok(PostAction::Continue),
                    Err(_) => Ok(PostAction::Remove),
                }
            });
            if let Err(e) = r {
                warn!("cannot watch child process: {e}");
            }
        }
        None => {
            // Kernels without pidfd (<5.3): a blocking waiter thread.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

fn in_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

/// Terminals tried in order when `$TERMINAL` is unset.
const TERMINALS: &[&str] = &[
    "m3-terminal",
    "xdg-terminal-exec",
    "kgx",
    "gnome-terminal",
    "konsole",
    "foot",
    "alacritty",
    "kitty",
    "wezterm",
    "ghostty",
    "xfce4-terminal",
    "weston-terminal",
    "xterm",
];

pub fn pick_terminal() -> Option<String> {
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.trim().is_empty() && in_path(t.trim()) {
            return Some(t.trim().to_owned());
        }
    }
    TERMINALS.iter().find(|t| in_path(t)).map(|t| (*t).to_owned())
}

pub fn spawn_terminal(m3: &M3) {
    match pick_terminal() {
        Some(t) => spawn(m3, &[t]),
        None => warn!("no terminal emulator found; set $TERMINAL or install one (e.g. foot)"),
    }
}

// ---- session duties ------------------------------------------------------------

const SESSION_ENV: &[&str] = &[
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_SESSION_DESKTOP",
    "M3_SOCKET",
    "XCURSOR_THEME",
    "XCURSOR_SIZE",
];

/// Run once at session start.
pub fn start_session(m3: &mut M3) {
    // Make the session environment visible to D-Bus- and systemd-activated
    // services (portals, notification clients, autostart apps).
    if in_path("dbus-update-activation-environment") {
        let mut argv = vec!["dbus-update-activation-environment".to_owned()];
        if in_path("systemctl") {
            argv.push("--systemd".into());
        }
        argv.extend(SESSION_ENV.iter().map(|s| (*s).to_owned()));
        spawn(m3, &argv);
    } else if in_path("systemctl") {
        let mut argv = vec!["systemctl".to_owned(), "--user".into(), "import-environment".into()];
        argv.extend(SESSION_ENV.iter().map(|s| (*s).to_owned()));
        spawn(m3, &argv);
    }
    // graphical-session.target (portals, …) and XDG autostart via systemd
    // when available.
    if in_path("systemctl") {
        spawn(
            m3,
            &[
                "systemctl".into(),
                "--user".into(),
                "start".into(),
                // Pulls in graphical-session.target and XDG autostart (which
                // refuses to be started directly).
                "m3-session.target".into(),
            ],
        );
    }
    m3.session = true;
    supervise_shell(m3);
}

/// Keep m3-shell running: restart it with backoff if it crashes, give up
/// after repeated rapid failures (the compositor itself stays up).
fn supervise_shell(m3: &mut M3) {
    let now = std::time::Instant::now();
    m3.shell_restarts.retain(|t| now.duration_since(*t) < std::time::Duration::from_secs(60));
    if m3.shell_restarts.len() >= 5 {
        warn!("m3-shell crashed 5 times in a minute; not restarting it again (run `m3ctl restart` to retry)");
        return;
    }
    m3.shell_restarts.push(now);
    m3.shell_pid = spawn_with(
        m3,
        &["m3-shell".to_owned()],
        Some(Box::new(|state, clean| {
            state.m3.shell_pid = None;
            if state.m3.quitting {
                return;
            }
            if clean {
                info!("m3-shell exited");
            } else {
                warn!("m3-shell crashed; restarting it");
            }
            let delay = std::time::Duration::from_millis(500 * state.m3.shell_restarts.len().max(1) as u64);
            let _ = state.m3.loop_handle.insert_source(
                smithay::reexports::calloop::timer::Timer::from_duration(delay),
                |_, _, state| {
                    supervise_shell(&mut state.m3);
                    smithay::reexports::calloop::timer::TimeoutAction::Drop
                },
            );
        })),
    );
}

/// Stop session units on exit (blocking briefly is fine: we are exiting).
pub fn end_session(m3: &M3) {
    if m3.session && in_path("systemctl") {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", "m3-session.target", "graphical-session.target"])
            .stdin(Stdio::null())
            .status();
    }
}
