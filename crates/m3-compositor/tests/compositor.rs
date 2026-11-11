//! End-to-end compositor tests against real Wayland clients on the headless
//! backend. Tests skip themselves when the client programs are missing.

mod common;

use common::*;
use m3_protocol::{Action, Direction, OutputChange, Request, Response, TestInput};

const CLIENT: &str = "weston-simple-shm";

#[test]
fn starts_and_reports_outputs() {
    require!();
    let s = Session::start_with("1280x800,800x600@2", &[], &[]);
    let Response::Version { protocol, .. } = s.request(Request::Version).unwrap() else { panic!() };
    assert_eq!(protocol, m3_protocol::PROTOCOL_VERSION);
    let Response::Outputs { outputs } = s.request(Request::Outputs).unwrap() else { panic!() };
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].logical.width, 1280);
    // Second output: 800x600 physical at scale 2 → 400x300 logical, placed
    // to the right of the first.
    assert_eq!(outputs[1].scale, 2.0);
    assert_eq!((outputs[1].logical.x, outputs[1].logical.width, outputs[1].logical.height), (1280, 400, 300));
    assert_eq!(s.workspaces().len(), 4);
}

#[test]
fn maps_focuses_and_cascades_windows() {
    require!(CLIENT);
    let mut s = Session::start();
    s.spawn_client(&[CLIENT]);
    let w = s.wait_windows("first window", |w| w.len() == 1);
    assert!(w[0].focused);
    assert_eq!(w[0].app_id.as_deref(), Some("org.freedesktop.weston.simple-shm"));
    // Centered on the 1280x800 output.
    let g = w[0].geometry;
    assert_eq!((g.x + g.width / 2, g.y + g.height / 2), (640, 400));

    s.spawn_client(&[CLIENT]);
    let w = s.wait_windows("second window", |w| w.len() == 2);
    assert!(!w[0].focused && w[1].focused, "new windows take focus");
    assert_ne!((w[0].geometry.x, w[0].geometry.y), (w[1].geometry.x, w[1].geometry.y), "cascade");
}

#[test]
fn click_to_focus() {
    require!(CLIENT);
    let mut s = Session::start();
    s.spawn_client(&[CLIENT]);
    s.wait_windows("one", |w| w.len() == 1);
    s.spawn_client(&[CLIENT]);
    let w = s.wait_windows("two", |w| w.len() == 2 && w[1].focused);
    // Click on a part of window 1 not covered by window 2.
    let g = w[0].geometry;
    s.input(TestInput::PointerMotion { x: (g.x + 4) as f64, y: (g.y + 4) as f64 });
    s.input(TestInput::PointerButton { button: BTN_LEFT, pressed: true });
    s.input(TestInput::PointerButton { button: BTN_LEFT, pressed: false });
    s.wait_windows("window 1 focused", |w| w[0].focused && !w[1].focused);
}

#[test]
fn keybinding_spawns_terminal() {
    require!(CLIENT);
    // $TERMINAL picks what Super+Return launches.
    let s = Session::start_with("1280x800", &[], &[("TERMINAL", CLIENT)]);
    s.chord(&[KEY_LEFTMETA], KEY_ENTER);
    s.wait_windows("terminal from keybinding", |w| w.len() == 1);
}

#[test]
fn custom_keybinding_from_config() {
    require!(CLIENT);
    let kb = format!("[bindings]\n\"Super+Shift+Return\" = {{ action = \"spawn\", command = [\"{CLIENT}\"] }}\n");
    let s = Session::start_with("1280x800", &[("keybindings.toml", &kb)], &[]);
    s.chord(&[KEY_LEFTMETA, KEY_LEFTSHIFT], KEY_ENTER);
    s.wait_windows("window from custom binding", |w| w.len() == 1);
}

#[test]
fn workspaces_switch_and_move() {
    require!(CLIENT);
    let mut s = Session::start();
    s.spawn_client(&[CLIENT]);
    let w = s.wait_windows("window", |w| w.len() == 1);
    let id = w[0].id;

    s.chord(&[KEY_LEFTMETA], KEY_2);
    assert!(s.workspaces()[1].active);
    s.action(Action::SwitchWorkspace { index: 1 });

    // Super+Shift+2 moves the focused window to workspace 2.
    s.chord(&[KEY_LEFTMETA, KEY_LEFTSHIFT], KEY_2);
    let w = s.wait_windows("moved", |w| w[0].workspace == 2);
    assert_eq!(w[0].id, id);
    assert!(s.workspaces()[0].active, "moving does not follow the window");
    assert_eq!(s.workspaces()[1].window_count, 1);

    // Focusing a window by id switches to its workspace.
    s.action(Action::FocusWindow { id });
    assert!(s.workspaces()[1].active);
    s.wait_windows("focused after activation", |w| w[0].focused);
}

#[test]
fn tiling_layout_partitions_the_output() {
    require!(CLIENT);
    let mut s = Session::start();
    for _ in 0..3 {
        s.spawn_client(&[CLIENT]);
    }
    s.wait_windows("three", |w| w.len() == 3);
    s.action(Action::SetLayout { layout: "tiling".into() });
    // Tile order follows map order, which depends on client startup timing:
    // check the layout invariants rather than which window is master.
    let w = s.wait_windows("tiled positions", |w| {
        w.iter().filter(|w| w.geometry.x == 8 && w.geometry.height == 784).count() == 1
            && w.iter().filter(|w| w.geometry.x > 8).count() == 2
    });
    let mut stack: Vec<_> = w.iter().filter(|w| w.geometry.x > 8).map(|w| w.geometry).collect();
    stack.sort_by_key(|g| g.y);
    assert_eq!(stack[0].x, stack[1].x, "stack column");
    assert!(stack[0].y + stack[0].height <= stack[1].y, "stack windows do not overlap");
    s.action(Action::SetLayout { layout: "floating".into() });
}

#[test]
fn maximize_and_restore_with_keyboard() {
    require!("foot");
    let mut s = Session::start();
    s.spawn_client(&["foot"]);
    let before = s.wait_windows("foot", |w| w.len() == 1 && w[0].geometry.width > 0)[0].geometry;
    s.chord(&[KEY_LEFTMETA], KEY_UP);
    let w = s.wait_windows("maximized", |w| w[0].maximized && w[0].geometry.width == 1280);
    assert_eq!((w[0].geometry.x, w[0].geometry.y, w[0].geometry.height), (0, 0, 800));
    s.action(Action::ToggleMaximize);
    let w = s.wait_windows("restored", |w| !w[0].maximized && w[0].geometry.width == before.width);
    assert_eq!((w[0].geometry.x, w[0].geometry.y), (before.x, before.y));
}

#[test]
fn snapping_halves_the_output() {
    require!("foot");
    let mut s = Session::start();
    s.spawn_client(&["foot"]);
    s.wait_windows("foot", |w| w.len() == 1 && w[0].geometry.width > 0);
    s.action(Action::Snap { direction: Direction::Left });
    let gap = 8;
    s.wait_windows("snapped left", |w| w[0].geometry.x == gap && w[0].geometry.width == (1280 - 3 * gap) / 2);
}

#[test]
fn typing_reaches_the_client() {
    require!("foot");
    let mut s = Session::start();
    s.spawn_client(&["foot"]);
    s.wait_windows("foot", |w| w.len() == 1 && w[0].focused);
    std::thread::sleep(std::time::Duration::from_millis(300));
    // Type `exit` + Enter: the shell exits, foot closes, the window goes away.
    for k in [18, 45, 23, 20, KEY_ENTER] {
        s.tap(k);
    }
    s.wait_windows("window closed by typed `exit`", |w| w.is_empty());
}

#[test]
fn close_window_action() {
    require!(CLIENT);
    let mut s = Session::start();
    s.spawn_client(&[CLIENT]);
    s.wait_windows("window", |w| w.len() == 1);
    s.chord(&[KEY_LEFTMETA], KEY_Q);
    s.wait_windows("closed", |w| w.is_empty());
}

#[test]
fn alt_tab_cycles_focus() {
    require!(CLIENT);
    let mut s = Session::start();
    s.spawn_client(&[CLIENT]);
    s.wait_windows("one", |w| w.len() == 1);
    s.spawn_client(&[CLIENT]);
    s.wait_windows("two", |w| w.len() == 2 && w[1].focused);
    s.chord(&[KEY_LEFTALT], KEY_TAB);
    s.wait_windows("alt-tab focused previous", |w| w[0].focused);
    s.chord(&[KEY_LEFTALT], KEY_TAB);
    s.wait_windows("alt-tab back", |w| w[1].focused);
}

#[test]
fn survives_client_crash() {
    require!(CLIENT);
    let mut s = Session::start();
    let pid = s.spawn_client(&[CLIENT]);
    s.wait_windows("window", |w| w.len() == 1);
    s.kill_client(pid);
    s.wait_windows("window removed after crash", |w| w.is_empty());
    assert!(s.alive());
    s.spawn_client(&[CLIENT]);
    s.wait_windows("new window after crash", |w| w.len() == 1 && w[0].focused);
}

#[test]
fn screenshot_matches_output_size() {
    require!();
    let s = Session::start_with("1024x640", &[], &[]);
    let p = s.screenshot("shot.png");
    assert_eq!(png_size(&p), (1024, 640));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn output_scale_change() {
    require!();
    let s = Session::start();
    s.request(Request::ConfigureOutput { output: "HEADLESS-1".into(), change: OutputChange::Scale { scale: 2.0 } })
        .unwrap();
    let Response::Outputs { outputs } = s.request(Request::Outputs).unwrap() else { panic!() };
    assert_eq!((outputs[0].logical.width, outputs[0].logical.height), (640, 400));
    let bad =
        s.request(Request::ConfigureOutput { output: "HEADLESS-1".into(), change: OutputChange::Scale { scale: 9.0 } });
    assert!(bad.is_err());
}

#[test]
fn invalid_config_falls_back_and_reload_applies() {
    require!();
    let s = Session::start_with("1280x800", &[("workspaces.toml", "count = 999\n")], &[]);
    assert_eq!(s.workspaces().len(), 4, "invalid file ignored, defaults used");
    assert!(s.log().contains("workspaces.toml has invalid settings"));
    std::fs::write(s.config_dir().join("workspaces.toml"), "count = 6\nnames = [\"Web\"]\n").unwrap();
    s.request(Request::Reload).unwrap();
    let ws = s.workspaces();
    assert_eq!(ws.len(), 6);
    assert_eq!(ws[0].name, "Web");
    // The broken file was never rewritten by m3.
    assert_eq!(
        std::fs::read_to_string(s.config_dir().join("workspaces.toml")).unwrap(),
        "count = 6\nnames = [\"Web\"]\n"
    );
}

#[test]
fn ipc_rejects_garbage_and_survives() {
    require!();
    use std::io::{BufRead, BufReader, Write};
    let s = Session::start();
    let mut stream = std::os::unix::net::UnixStream::connect(&s.socket).unwrap();
    stream.write_all(b"this is not json\n").unwrap();
    let mut line = String::new();
    BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
    assert!(line.contains("Err"), "{line}");
    drop(stream);
    assert!(matches!(s.request(Request::Version), Ok(Response::Version { .. })));
}

#[test]
fn event_stream_reports_changes() {
    require!(CLIENT);
    let mut s = Session::start();
    let mut events = s.ipc().subscribe().unwrap();
    // Initial snapshot.
    let first = events.next_event().unwrap().unwrap();
    assert!(matches!(first, m3_protocol::Event::WorkspacesChanged { .. }));
    s.spawn_client(&[CLIENT]);
    loop {
        match events.next_event().unwrap().unwrap() {
            m3_protocol::Event::WindowsChanged { windows } if windows.len() == 1 => break,
            _ => {}
        }
    }
}

#[test]
fn shell_actions_are_forwarded() {
    require!();
    let s = Session::start();
    let mut events = s.ipc().subscribe().unwrap();
    s.chord(&[KEY_LEFTMETA], 57); // Super+Space
    loop {
        if let m3_protocol::Event::ShellAction { action } = events.next_event().unwrap().unwrap() {
            assert_eq!(action, Action::ToggleLauncher);
            break;
        }
    }
}

#[test]
fn idle_locks_the_session() {
    require!();
    // m3-lock is not on PATH in tests; the attempt itself is what we check.
    let s =
        Session::start_with("800x600", &[("config.toml", "[idle]\nlock_after = 1\n")], &[("PATH", "/usr/bin:/bin")]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !s.log().contains("locking") {
        assert!(std::time::Instant::now() < deadline, "no idle lock:\n{}", s.log());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(s.log().contains("failed to start `m3-lock`"));
}

#[test]
fn input_postpones_idle_lock() {
    require!();
    let s = Session::start_with("800x600", &[("config.toml", "[idle]\nlock_after = 2\n")], &[]);
    for i in 0..8 {
        s.input(TestInput::PointerMotion { x: 10.0 + i as f64, y: 10.0 });
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    assert!(!s.log().contains("locking"), "locked despite activity:\n{}", s.log());
}

#[test]
fn session_supervises_a_crashing_shell() {
    require!();
    use std::os::unix::fs::PermissionsExt;
    let bin = tempfile::tempdir().unwrap();
    let fake = bin.path().join("m3-shell");
    std::fs::write(&fake, "#!/bin/sh\nexit 3\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.path().display());
    let mut s = Session::start_with_args("800x600", &["--session"], &[], &[("PATH", &path)]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !s.log().contains("not restarting it again") {
        assert!(std::time::Instant::now() < deadline, "no crash-loop protection:\n{}", s.log());
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(s.log().matches("m3-shell crashed; restarting it").count() >= 4);
    assert!(s.alive(), "the compositor must survive shell crashes");
}

#[test]
fn idle_lock_then_suspend_both_fire_once() {
    require!();
    let cfg = "[idle]\nlock_after = 1\nsuspend_after = 2\n";
    let s = Session::start_with("800x600", &[("config.toml", cfg)], &[("PATH", "/nonexistent")]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(70);
    while !s.log().contains("suspending") {
        assert!(std::time::Instant::now() < deadline, "suspend never triggered:\n{}", s.log());
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(s.log().matches("; locking").count(), 1, "lock fired more than once per idle period");
    assert_eq!(s.log().matches("; suspending").count(), 1);
}
