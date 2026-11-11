//! `m3ctl` — control the running m3 desktop from the command line.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, ValueEnum};
use m3_core::config::{self, ColorSource, ThemeConfig, ThemeMode};
use m3_protocol::{Action, Client, Direction, OutputChange, Request, Response, TestInput, Transform};

#[derive(Parser)]
#[command(name = "m3ctl", version, about = "Control the m3 desktop environment", propagate_version = true)]
struct Cli {
    /// Print machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show m3ctl and compositor versions.
    Version,
    /// Re-read all configuration files.
    Reload,
    /// Theme: `dark`, `light`, `auto`, or a sub-command.
    #[command(subcommand)]
    Theme(ThemeCmd),
    /// Wallpaper settings.
    #[command(subcommand)]
    Wallpaper(WallpaperCmd),
    /// Workspaces.
    #[command(subcommand)]
    Workspace(WorkspaceCmd),
    /// Windows.
    #[command(subcommand)]
    Window(WindowCmd),
    /// Displays (outputs).
    #[command(subcommand)]
    Display(DisplayCmd),
    /// Notifications.
    #[command(subcommand)]
    Notifications(NotificationsCmd),
    /// Lock the session.
    Lock,
    /// End the session (closes the compositor and every application).
    Logout,
    /// Restart the desktop shell (panel, launcher, notifications).
    Restart,
    /// Take a screenshot (PNG). Defaults to ~/Pictures/Screenshots.
    Screenshot {
        path: Option<PathBuf>,
        #[arg(long)]
        output: Option<String>,
    },
    /// Run a compositor action given as JSON, e.g. '{"action":"toggle_overview"}'.
    Action {
        #[arg(value_name = "JSON")]
        action: String,
    },
    /// Print compositor events as they happen (one JSON object per line).
    Events,
    /// Synthetic input (only when m3 runs with --test-mode).
    #[command(hide = true, subcommand)]
    TestInput(TestCmd),
}

#[derive(Subcommand)]
enum ThemeCmd {
    Dark,
    Light,
    /// Dark at night, light during the day (see `dark_from` / `dark_until`).
    Auto,
    /// Use a fixed seed colour instead of the wallpaper.
    SetColor {
        color: String,
    },
    /// Use one of the built-in colour presets.
    Preset {
        name: String,
    },
    /// Derive colours from the wallpaper.
    FromWallpaper,
    /// Scheme variant: tonal_spot, vibrant, expressive, content, fidelity,
    /// neutral, monochrome, rainbow, fruit_salad.
    Variant {
        variant: String,
    },
    /// Show the current theme settings.
    Show,
}

#[derive(Subcommand)]
enum WallpaperCmd {
    /// Set the wallpaper (for all outputs, or one with --output).
    Set {
        path: PathBuf,
        #[arg(long)]
        output: Option<String>,
    },
    /// Show the current wallpaper settings.
    Show,
}

#[derive(Subcommand)]
enum WorkspaceCmd {
    List,
    /// Switch to workspace N (1-based).
    Switch {
        index: usize,
    },
    /// Move the focused window to workspace N.
    Move {
        index: usize,
    },
    Next,
    Previous,
    /// Set the layout of the active workspace.
    Layout {
        layout: LayoutArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum LayoutArg {
    Floating,
    Tiling,
    Monocle,
}

#[derive(Subcommand)]
enum WindowCmd {
    List,
    /// Show the focused window.
    Focused,
    Focus {
        id: u64,
    },
    Close {
        id: u64,
    },
    /// Move window ID to workspace N.
    Move {
        id: u64,
        workspace: usize,
    },
    /// Snap the focused window.
    Snap {
        direction: DirArg,
    },
    Maximize,
    Fullscreen,
    Minimize,
    Float,
}

#[derive(Clone, Copy, ValueEnum)]
enum DirArg {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Subcommand)]
enum DisplayCmd {
    List,
    SetScale {
        scale: f64,
        #[arg(long)]
        output: Option<String>,
    },
    SetTransform {
        transform: String,
        #[arg(long)]
        output: Option<String>,
    },
    SetPosition {
        x: i32,
        y: i32,
        #[arg(long)]
        output: Option<String>,
    },
    /// WIDTHxHEIGHT[@HZ]
    SetMode {
        mode: String,
        #[arg(long)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
enum NotificationsCmd {
    /// Dismiss every notification.
    Clear,
}

#[derive(Subcommand)]
enum TestCmd {
    Motion {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        #[arg(long)]
        release: bool,
    },
    Click {
        #[arg(default_value_t = 0x110)]
        button: u32,
    },
    Key {
        keycode: u32,
        #[arg(long)]
        release: bool,
    },
    Tap {
        keycode: u32,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("m3ctl: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn connect() -> anyhow::Result<Client> {
    Ok(Client::connect()?)
}

fn request(req: Request) -> anyhow::Result<Response> {
    Ok(connect()?.request(&req)?)
}

fn action(a: Action) -> anyhow::Result<()> {
    request(Request::Action { action: a })?;
    Ok(())
}

fn print_json<T: serde::Serialize>(v: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let json = cli.json;
    match cli.cmd {
        Cmd::Version => {
            let compositor = match request(Request::Version) {
                Ok(Response::Version { compositor, protocol }) => Some((compositor, protocol)),
                _ => None,
            };
            if json {
                print_json(&serde_json::json!({
                    "m3ctl": m3_core::VERSION,
                    "compositor": compositor.as_ref().map(|c| &c.0),
                    "protocol": compositor.as_ref().map(|c| c.1),
                }))?;
            } else {
                println!("m3ctl {}", m3_core::VERSION);
                match compositor {
                    Some((c, p)) => println!("{c} (IPC protocol {p})"),
                    None => println!("compositor: not running"),
                }
            }
        }
        Cmd::Reload => {
            request(Request::Reload)?;
        }
        Cmd::Theme(t) => theme(t, json)?,
        Cmd::Wallpaper(w) => wallpaper(w, json)?,
        Cmd::Workspace(w) => match w {
            WorkspaceCmd::List => {
                let Response::Workspaces { workspaces } = request(Request::Workspaces)? else {
                    bail!("unexpected reply")
                };
                if json {
                    print_json(&workspaces)?;
                } else {
                    for ws in workspaces {
                        let mark = if ws.active { "*" } else { " " };
                        println!("{mark} {:>2}  {:<16} {} window(s)", ws.index, ws.name, ws.window_count);
                    }
                }
            }
            WorkspaceCmd::Switch { index } => action(Action::SwitchWorkspace { index })?,
            WorkspaceCmd::Move { index } => action(Action::MoveToWorkspace { index })?,
            WorkspaceCmd::Next => action(Action::NextWorkspace)?,
            WorkspaceCmd::Previous => action(Action::PreviousWorkspace)?,
            WorkspaceCmd::Layout { layout } => action(Action::SetLayout {
                layout: match layout {
                    LayoutArg::Floating => "floating",
                    LayoutArg::Tiling => "tiling",
                    LayoutArg::Monocle => "monocle",
                }
                .into(),
            })?,
        },
        Cmd::Window(w) => match w {
            WindowCmd::List => {
                let Response::Windows { windows } = request(Request::Windows)? else { bail!("unexpected reply") };
                if json {
                    print_json(&windows)?;
                } else {
                    for w in windows {
                        let mut flags = Vec::new();
                        if w.focused {
                            flags.push("focused")
                        }
                        if w.minimized {
                            flags.push("minimized")
                        }
                        if w.maximized {
                            flags.push("maximized")
                        }
                        if w.fullscreen {
                            flags.push("fullscreen")
                        }
                        if w.xwayland {
                            flags.push("x11")
                        }
                        println!(
                            "{:>4}  ws {:<2} {:<28} {:<40} {}x{}+{}+{} {}",
                            w.id,
                            w.workspace,
                            w.app_id.unwrap_or_default(),
                            w.title.unwrap_or_default().chars().take(40).collect::<String>(),
                            w.geometry.width,
                            w.geometry.height,
                            w.geometry.x,
                            w.geometry.y,
                            flags.join(",")
                        );
                    }
                }
            }
            WindowCmd::Focused => {
                let Response::FocusedWindow { window } = request(Request::FocusedWindow)? else {
                    bail!("unexpected reply")
                };
                if json {
                    print_json(&window)?;
                } else {
                    match window {
                        Some(w) => {
                            println!("{} {} {}", w.id, w.app_id.unwrap_or_default(), w.title.unwrap_or_default())
                        }
                        None => println!("no focused window"),
                    }
                }
            }
            WindowCmd::Focus { id } => action(Action::FocusWindow { id })?,
            WindowCmd::Close { id } => action(Action::CloseWindowById { id })?,
            WindowCmd::Move { id, workspace } => action(Action::MoveWindowToWorkspace { id, index: workspace })?,
            WindowCmd::Snap { direction } => action(Action::Snap {
                direction: match direction {
                    DirArg::Left => Direction::Left,
                    DirArg::Right => Direction::Right,
                    DirArg::Up => Direction::Up,
                    DirArg::Down => Direction::Down,
                },
            })?,
            WindowCmd::Maximize => action(Action::ToggleMaximize)?,
            WindowCmd::Fullscreen => action(Action::ToggleFullscreen)?,
            WindowCmd::Minimize => action(Action::Minimize)?,
            WindowCmd::Float => action(Action::ToggleFloating)?,
        },
        Cmd::Display(d) => display(d, json)?,
        Cmd::Notifications(NotificationsCmd::Clear) => {
            // The notification server listens on the session bus.
            let status = std::process::Command::new("busctl")
                .args([
                    "--user",
                    "call",
                    "org.m3.Notifications",
                    "/org/m3/Notifications",
                    "org.m3.Notifications",
                    "ClearAll",
                ])
                .status()
                .context("running busctl (systemd) to reach the notification server")?;
            if !status.success() {
                bail!("the m3 notification server is not running");
            }
        }
        Cmd::Lock => action(Action::Lock)?,
        Cmd::Logout => action(Action::Quit)?,
        Cmd::Restart => action(Action::RestartShell)?,
        Cmd::Screenshot { path, output } => {
            let path = match path {
                Some(p) => std::path::absolute(p)?,
                None => {
                    let home = std::env::var_os("HOME").context("$HOME is not set")?;
                    let dir = PathBuf::from(home).join("Pictures/Screenshots");
                    std::fs::create_dir_all(&dir)?;
                    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
                    dir.join(format!("Screenshot-{ts}.png"))
                }
            };
            request(Request::Screenshot { output, path: path.clone(), region: None, notify: false })?;
            if json {
                print_json(&serde_json::json!({ "path": path }))?;
            } else {
                println!("{}", path.display());
            }
        }
        Cmd::Action { action: a } => {
            let a: Action = serde_json::from_str(&a).context("parsing the action JSON")?;
            action(a)?;
        }
        Cmd::Events => {
            let stream = connect()?.subscribe()?;
            for event in stream {
                println!("{}", serde_json::to_string(&event?)?);
            }
        }
        Cmd::TestInput(t) => {
            let mut c = connect()?;
            let mut send = |input| c.request(&Request::TestInput { input }).map(|_| ());
            match t {
                TestCmd::Motion { x, y } => send(TestInput::PointerMotion { x, y })?,
                TestCmd::Button { button, release } => send(TestInput::PointerButton { button, pressed: !release })?,
                TestCmd::Click { button } => {
                    send(TestInput::PointerButton { button, pressed: true })?;
                    send(TestInput::PointerButton { button, pressed: false })?;
                }
                TestCmd::Key { keycode, release } => send(TestInput::Key { keycode, pressed: !release })?,
                TestCmd::Tap { keycode } => {
                    send(TestInput::Key { keycode, pressed: true })?;
                    send(TestInput::Key { keycode, pressed: false })?;
                }
            }
        }
    }
    Ok(())
}

/// Load, modify and save theme.toml, then tell running components.
fn edit_theme(f: impl FnOnce(&mut ThemeConfig)) -> anyhow::Result<()> {
    let dir = config::default_dir()?;
    let mut theme = config::load::<ThemeConfig>(&dir)?.value;
    f(&mut theme);
    config::save(&dir, &theme)?;
    // The shell and daemon watch theme.toml; the compositor needs a nudge.
    let _ = request(Request::Reload);
    Ok(())
}

fn theme(t: ThemeCmd, json: bool) -> anyhow::Result<()> {
    match t {
        ThemeCmd::Dark => edit_theme(|t| t.mode = ThemeMode::Dark),
        ThemeCmd::Light => edit_theme(|t| t.mode = ThemeMode::Light),
        ThemeCmd::Auto => edit_theme(|t| t.mode = ThemeMode::Auto),
        ThemeCmd::SetColor { color } => {
            config::parse_hex_color(&color).map_err(anyhow::Error::msg)?;
            edit_theme(|t| {
                t.seed_color = color;
                t.source = ColorSource::Seed;
            })
        }
        ThemeCmd::Preset { name } => {
            if !config::PRESETS.iter().any(|p| p.0 == name) {
                let ids: Vec<&str> = config::PRESETS.iter().map(|p| p.0).collect();
                bail!("unknown preset `{name}`; available: {}", ids.join(", "));
            }
            edit_theme(|t| {
                t.preset = name;
                t.source = ColorSource::Preset;
            })
        }
        ThemeCmd::FromWallpaper => edit_theme(|t| t.source = ColorSource::Wallpaper),
        ThemeCmd::Variant { variant } => {
            let v: config::SchemeVariant = serde_json::from_value(serde_json::Value::String(variant.clone()))
                .with_context(|| format!("unknown variant `{variant}`"))?;
            edit_theme(|t| t.variant = v)
        }
        ThemeCmd::Show => {
            let dir = config::default_dir()?;
            let t = config::load::<ThemeConfig>(&dir)?.value;
            if json {
                print_json(&serde_json::json!({
                    "mode": t.mode, "source": t.source, "seed_color": t.seed_color,
                    "preset": t.preset, "variant": t.variant, "contrast": t.contrast,
                }))
            } else {
                println!("mode     {:?}", t.mode);
                println!("source   {:?}", t.source);
                println!("seed     {}", t.seed_color);
                println!("preset   {}", t.preset);
                println!("variant  {}", t.variant.label());
                Ok(())
            }
        }
    }
}

fn wallpaper(w: WallpaperCmd, json: bool) -> anyhow::Result<()> {
    match w {
        WallpaperCmd::Set { path, output } => {
            let path = std::path::absolute(&path)?;
            if !path.is_file() {
                bail!("{} is not a file", path.display());
            }
            edit_theme(|t| match output {
                Some(o) => {
                    t.wallpaper.outputs.insert(o, path);
                }
                None => t.wallpaper.path = path,
            })
        }
        WallpaperCmd::Show => {
            let dir = config::default_dir()?;
            let t = config::load::<ThemeConfig>(&dir)?.value;
            if json {
                print_json(&serde_json::json!({ "path": t.wallpaper.path, "outputs": t.wallpaper.outputs }))
            } else {
                println!(
                    "{}",
                    if t.wallpaper.path.as_os_str().is_empty() {
                        "(default)".into()
                    } else {
                        t.wallpaper.path.display().to_string()
                    }
                );
                for (o, p) in t.wallpaper.outputs {
                    println!("  {o}: {}", p.display());
                }
                Ok(())
            }
        }
    }
}

fn display(d: DisplayCmd, json: bool) -> anyhow::Result<()> {
    let first_output = || -> anyhow::Result<String> {
        let Response::Outputs { outputs } = request(Request::Outputs)? else { bail!("unexpected reply") };
        outputs.first().map(|o| o.name.clone()).context("no outputs")
    };
    let configure = |output: Option<String>, change: OutputChange| -> anyhow::Result<()> {
        let output = match output {
            Some(o) => o,
            None => first_output()?,
        };
        request(Request::ConfigureOutput { output, change })?;
        Ok(())
    };
    match d {
        DisplayCmd::List => {
            let Response::Outputs { outputs } = request(Request::Outputs)? else { bail!("unexpected reply") };
            if json {
                return print_json(&outputs);
            }
            for o in outputs {
                let mode = o
                    .current_mode
                    .map(|m| format!("{}x{}@{:.3}", m.width, m.height, m.refresh_mhz as f64 / 1000.0))
                    .unwrap_or_else(|| "off".into());
                println!(
                    "{}  {} {}  {}  scale {}  at {},{}  ({}x{} logical)",
                    o.name, o.make, o.model, mode, o.scale, o.logical.x, o.logical.y, o.logical.width, o.logical.height
                );
            }
            Ok(())
        }
        DisplayCmd::SetScale { scale, output } => configure(output, OutputChange::Scale { scale }),
        DisplayCmd::SetTransform { transform, output } => {
            let t: Transform =
                serde_json::from_value(serde_json::Value::String(transform.clone())).with_context(|| {
                    format!("unknown transform `{transform}` (normal, 90, 180, 270, flipped, flipped_90, …)")
                })?;
            configure(output, OutputChange::Transform { transform: t })
        }
        DisplayCmd::SetPosition { x, y, output } => configure(output, OutputChange::Position { x, y }),
        DisplayCmd::SetMode { mode, output } => {
            let m = config::OutputMode::parse(&mode).map_err(anyhow::Error::msg)?;
            configure(output, OutputChange::Mode { width: m.width, height: m.height, refresh_mhz: m.refresh_mhz })
        }
    }
}
