//! m3 — the Material 3 Expressive Wayland compositor.

mod actions;
mod backend;
mod config;
mod cursor;
mod focus;
mod grabs;
mod handlers;
mod input;
mod ipc;
mod nightlight;
mod render;
mod screencopy;
mod screenshot;
mod shell;
mod spawn;
mod state;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
use tracing::{error, info};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendKind {
    /// Pick automatically: nested if a Wayland/X11 session is running, DRM otherwise.
    Auto,
    /// Run in a window inside an existing Wayland or X11 session (development).
    Winit,
    /// Drive real displays through DRM/KMS and libinput (a real session).
    Udev,
    /// Render offscreen only (automated tests, CI).
    Headless,
}

#[derive(Debug, Parser)]
#[command(name = "m3", version, about = "The m3 Material 3 Expressive Wayland compositor")]
pub struct Args {
    /// Which backend to run on.
    #[arg(long, value_enum, default_value_t = BackendKind::Auto)]
    pub backend: BackendKind,

    /// Virtual outputs for the headless backend, e.g. `1920x1080,1280x720@1.5`
    /// (`@` gives the scale).
    #[arg(long, default_value = "1920x1080")]
    pub headless_outputs: String,

    /// Enable synthetic-input and other test-only IPC requests.
    #[arg(long)]
    pub test_mode: bool,

    /// Use this directory instead of ~/.config/m3.
    #[arg(long)]
    pub config_dir: Option<std::path::PathBuf>,

    /// Run as the session's compositor: export the environment to systemd
    /// and D-Bus activation, start XDG autostart, and supervise m3-shell.
    #[arg(long)]
    pub session: bool,

    /// Do not start XWayland even if enabled in the configuration.
    #[arg(long)]
    pub no_xwayland: bool,

    /// Program (argv) to run once the compositor is ready, e.g. `-- foot`.
    #[arg(last = true)]
    pub command: Vec<String>,
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_env("M3_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,smithay=warn"));
    use std::io::IsTerminal;
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
}

fn main() {
    let args = Args::parse();
    init_logging();
    if let Err(e) = run(args) {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> anyhow::Result<()> {
    if rustix::process::geteuid().is_root() {
        anyhow::bail!("refusing to run as root; start m3 as a regular user (logind/seatd grant device access)");
    }
    info!("m3 {} starting", m3_core::VERSION);

    let mut event_loop: EventLoop<'static, state::State> = EventLoop::try_new().context("creating the event loop")?;
    let display: Display<state::State> = Display::new().context("creating the Wayland display")?;

    let kind = match args.backend {
        BackendKind::Auto => {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some() {
                BackendKind::Winit
            } else {
                BackendKind::Udev
            }
        }
        k => k,
    };
    info!(?kind, "selected backend");

    let mut state = state::State::new(&args, kind, display, &mut event_loop)?;

    if args.session {
        spawn::start_session(&mut state.m3);
    }
    if !args.command.is_empty() {
        spawn::spawn(&state.m3, &args.command);
    }

    let signal = event_loop.get_signal();
    state.m3.loop_signal = Some(signal);
    event_loop.run(None, &mut state, |state| state.refresh_and_flush()).context("running the event loop")?;

    info!("m3 exiting");
    state.shutdown();
    Ok(())
}
