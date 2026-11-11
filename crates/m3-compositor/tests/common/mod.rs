//! Test harness: runs a real `m3 --backend headless --test-mode` with an
//! isolated config dir and Wayland socket, and talks to it over IPC.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use m3_protocol::{Action, Client, Request, Response, TestInput, WindowInfo, WorkspaceInfo};

pub struct Session {
    child: Child,
    pub dir: tempfile::TempDir,
    pub socket: PathBuf,
    pub wayland_display: String,
    clients: Vec<Child>,
}

pub fn have(program: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file())).unwrap_or(false)
}

/// Skip (return early) unless the given programs are installed.
#[macro_export]
macro_rules! require {
    ($($p:expr),*) => {
        $(if !common::have($p) {
            eprintln!("skipping: `{}` is not installed", $p);
            return;
        })*
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            eprintln!("skipping: XDG_RUNTIME_DIR is not set");
            return;
        }
    };
}

impl Session {
    pub fn start() -> Self {
        Self::start_with("1280x800", &[], &[])
    }

    /// Start m3 with the given headless outputs, config files and extra env.
    pub fn start_with(outputs: &str, config_files: &[(&str, &str)], env: &[(&str, &str)]) -> Self {
        Self::start_with_args(outputs, &[], config_files, env)
    }

    pub fn start_with_args(
        outputs: &str,
        extra_args: &[&str],
        config_files: &[(&str, &str)],
        env: &[(&str, &str)],
    ) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&config).expect("config dir");
        for (name, text) in config_files {
            std::fs::write(config.join(name), text).expect("write config");
        }
        let log = std::fs::File::create(dir.path().join("m3.log")).expect("log");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_m3"));
        cmd.args(["--backend", "headless", "--test-mode", "--no-xwayland", "--headless-outputs", outputs])
            .args(extra_args)
            .arg("--config-dir")
            .arg(&config)
            .env("M3_LOG", "info,smithay=warn")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(log);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("spawning m3");
        let log_path = dir.path().join("m3.log");
        let deadline = Instant::now() + Duration::from_secs(20);
        let socket = loop {
            let text = std::fs::read_to_string(&log_path).unwrap_or_default();
            if let Some(line) = text.lines().find(|l| l.contains("IPC listening on ")) {
                let p = PathBuf::from(line.rsplit("IPC listening on ").next().unwrap_or_default().trim());
                if p.exists() {
                    break p;
                }
            }
            assert!(Instant::now() < deadline, "m3 did not start:\n{text}");
            std::thread::sleep(Duration::from_millis(30));
        };
        let wayland_display = socket.file_stem().expect("stem").to_string_lossy().into_owned();
        Self { child, dir, socket, wayland_display, clients: Vec::new() }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("m3.log")).unwrap_or_default()
    }

    pub fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    pub fn ipc(&self) -> Client {
        Client::connect_to(&self.socket).expect("IPC connect")
    }

    pub fn request(&self, req: Request) -> Result<Response, m3_protocol::ClientError> {
        self.ipc().request(&req)
    }

    pub fn action(&self, action: Action) {
        self.request(Request::Action { action }).expect("action");
    }

    pub fn windows(&self) -> Vec<WindowInfo> {
        match self.request(Request::Windows).expect("windows") {
            Response::Windows { windows } => windows,
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn workspaces(&self) -> Vec<WorkspaceInfo> {
        match self.request(Request::Workspaces).expect("workspaces") {
            Response::Workspaces { workspaces } => workspaces,
            r => panic!("unexpected {r:?}"),
        }
    }

    pub fn input(&self, input: TestInput) {
        self.request(Request::TestInput { input }).expect("test input");
    }

    pub fn tap(&self, keycode: u32) {
        self.input(TestInput::Key { keycode, pressed: true });
        self.input(TestInput::Key { keycode, pressed: false });
    }

    /// Press `mods` (evdev codes), tap `key`, release mods.
    pub fn chord(&self, mods: &[u32], key: u32) {
        for m in mods {
            self.input(TestInput::Key { keycode: *m, pressed: true });
        }
        self.tap(key);
        for m in mods.iter().rev() {
            self.input(TestInput::Key { keycode: *m, pressed: false });
        }
    }

    pub fn spawn_client(&mut self, argv: &[&str]) -> u32 {
        let child = Command::new(argv[0])
            .args(&argv[1..])
            .env("WAYLAND_DISPLAY", &self.wayland_display)
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn client");
        let pid = child.id();
        self.clients.push(child);
        pid
    }

    pub fn kill_client(&mut self, pid: u32) {
        if let Some(c) = self.clients.iter_mut().find(|c| c.id() == pid) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Poll until `cond` holds for the window list.
    pub fn wait_windows(&self, what: &str, cond: impl Fn(&[WindowInfo]) -> bool) -> Vec<WindowInfo> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let w = self.windows();
            if cond(&w) {
                return w;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; windows: {w:#?}\nlog:\n{}", self.log());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn screenshot(&self, name: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        self.request(Request::Screenshot { output: None, path: path.clone(), region: None, notify: false })
            .expect("screenshot");
        path
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for c in &mut self.clients {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn png_size(path: &Path) -> (u32, u32) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).expect("open png")));
    let reader = decoder.read_info().expect("png header");
    let info = reader.info();
    (info.width, info.height)
}

// evdev key codes used by tests.
pub const KEY_LEFTMETA: u32 = 125;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_LEFTALT: u32 = 56;
pub const KEY_ENTER: u32 = 28;
pub const KEY_TAB: u32 = 15;
pub const KEY_1: u32 = 2;
pub const KEY_2: u32 = 3;
pub const KEY_Q: u32 = 16;
pub const KEY_UP: u32 = 103;
pub const BTN_LEFT: u32 = 0x110;
