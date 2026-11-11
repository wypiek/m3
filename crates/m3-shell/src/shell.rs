//! The shell coordinator: owns every surface, reacts to compositor events,
//! monitors and configuration changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{gdk, gio, glib, prelude::*};
use m3_core::config::{self as cfg, ColorSource, PanelsConfig, ThemeConfig, WidgetsConfig};
use m3_protocol::{Action, Event, Request, WindowInfo, WorkspaceInfo};
use m3_theme::Theme;
use tracing::{info, warn};

use m3_services::{
    Command, Event as SvcEvent, Services,
    audio::{Audio, AudioCmd, AudioState},
};

use crate::{
    apps::{self, App},
    background::Background,
    ipc::{self, IpcMsg},
    launcher::Launcher,
    notifications::Notifications,
    osd::Osd,
    overview::Overview,
    panel::Panel,
    quick_settings::QuickSettings,
    status::{self, SysState},
    style::Style,
};

thread_local! {
    static SHELL: RefCell<Option<Rc<Shell>>> = const { RefCell::new(None) };
}

/// Commands surfaces send to the shell.
pub enum Cmd {
    ToggleLauncher,
    ToggleQuickSettings,
    ToggleNotifications,
    Launched(String),
    /// From the panel's power menu.
    Session(SessionAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Lock,
    LogOut,
    Suspend,
    Restart,
    ShutDown,
}

pub struct Shell {
    app: gtk::Application,
    _hold: gio::ApplicationHoldGuard,
    style: Style,
    config_dir: PathBuf,
    theme_cfg: RefCell<ThemeConfig>,
    panels_cfg: RefCell<PanelsConfig>,
    widgets_cfg: RefCell<WidgetsConfig>,
    /// Desktop widgets on the primary output: (output name, widgets).
    desktop: RefCell<Option<(String, Rc<crate::desktop::DesktopWidgets>)>>,
    theme: RefCell<Theme>,
    wallpaper_seed: RefCell<Option<u32>>,
    backgrounds: RefCell<HashMap<String, Rc<Background>>>,
    panels: RefCell<HashMap<String, Rc<Panel>>>,
    /// System tray service and its current items.
    tray: m3_services::tray::TrayHandle,
    tray_items: RefCell<Vec<m3_services::tray::TrayItem>>,
    launcher: Rc<Launcher>,
    overview: Rc<Overview>,
    /// Current slideshow image, if a slideshow folder is configured.
    slide: RefCell<Option<PathBuf>>,
    slide_timer: RefCell<Option<glib::SourceId>>,
    services: Services,
    audio: Audio,
    quick: Rc<QuickSettings>,
    notifications: Rc<Notifications>,
    osd: Osd,
    sys: RefCell<SysState>,
    apps: RefCell<Vec<App>>,
    windows: RefCell<Vec<WindowInfo>>,
    workspaces: RefCell<Vec<WorkspaceInfo>>,
    _monitors: RefCell<Vec<gio::FileMonitor>>,
    /// Lowest low-battery threshold already warned about (`u32::MAX` = none).
    battery_warned: std::cell::Cell<u32>,
    screenshot_count: std::cell::Cell<u32>,
}

impl Shell {
    pub fn start(app: &gtk::Application) {
        let config_dir = match cfg::default_dir() {
            Ok(d) => d,
            Err(e) => {
                warn!("{e}");
                app.quit();
                return;
            }
        };
        let (theme_cfg, _) = cfg::load_or(&config_dir, ThemeConfig::default());
        let (panels_cfg, _) = cfg::load_or(&config_dir, PanelsConfig::default());
        let (widgets_cfg, _) = cfg::load_or(&config_dir, WidgetsConfig::default());
        let wallpaper_seed = wallpaper_path(&theme_cfg, None, None).and_then(|p| m3_theme::wallpaper::cached_seed(&p));
        let theme = Theme::resolve(&theme_cfg, wallpaper_seed, m3_core::time::local_minutes());
        let style = Style::new();
        style.apply(&theme_cfg, &theme);
        let (services, svc_events) = Services::start();
        let (tray, tray_events) = m3_services::tray::start();
        let (audio, audio_events) = Audio::start();

        let shell = Rc::new(Self {
            app: app.clone(),
            _hold: app.hold(),
            style,
            config_dir,
            theme_cfg: RefCell::new(theme_cfg),
            panels_cfg: RefCell::new(panels_cfg),
            widgets_cfg: RefCell::new(widgets_cfg),
            desktop: RefCell::new(None),
            theme: RefCell::new(theme),
            wallpaper_seed: RefCell::new(wallpaper_seed),
            backgrounds: RefCell::new(HashMap::new()),
            panels: RefCell::new(HashMap::new()),
            launcher: Launcher::new(app),
            overview: Overview::new(app),
            slide: RefCell::new(None),
            slide_timer: RefCell::new(None),
            quick: QuickSettings::new(app, services.clone(), audio.clone()),
            notifications: Notifications::new(app, services.clone()),
            osd: Osd::new(app),
            services,
            tray,
            tray_items: RefCell::new(Vec::new()),
            audio,
            sys: RefCell::new(SysState::default()),
            apps: RefCell::new(apps::all_apps()),
            windows: RefCell::new(Vec::new()),
            workspaces: RefCell::new(Vec::new()),
            _monitors: RefCell::new(Vec::new()),
            battery_warned: std::cell::Cell::new(u32::MAX),
            screenshot_count: std::cell::Cell::new(0),
        });

        shell.quick.set_clock_12h(shell.panels_cfg.borrow().clock_format == m3_core::config::ClockFormat::H12);
        {
            let weak = Rc::downgrade(&shell);
            shell.launcher.set_on_launch(Rc::new(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.refresh_tasks();
                }
            }));
        }

        {
            let weak = Rc::downgrade(&shell);
            shell.notifications.set_on_count(Box::new(move |n| {
                if let Some(s) = weak.upgrade() {
                    s.sys.borrow_mut().unread = n;
                    s.refresh_status();
                }
            }));
        }
        shell.listen_services(svc_events, audio_events);
        shell.listen_tray(tray_events);
        shell.watch_monitors();
        shell.watch_config();
        shell.watch_apps();
        shell.listen_ipc();
        shell.watch_theme_schedule();
        shell.restart_slideshow();
        info!("m3-shell started");
        // Everything else holds weak references; this is the owner.
        SHELL.with(|s| *s.borrow_mut() = Some(shell));
    }

    fn dispatcher(self: &Rc<Self>) -> Rc<dyn Fn(Cmd)> {
        let weak = Rc::downgrade(self);
        Rc::new(move |cmd| {
            let Some(s) = weak.upgrade() else { return };
            match cmd {
                Cmd::ToggleLauncher => s.toggle_sheet(Sheet::Launcher),
                Cmd::ToggleQuickSettings => s.toggle_sheet(Sheet::QuickSettings),
                Cmd::ToggleNotifications => s.toggle_sheet(Sheet::Notifications),
                Cmd::Launched(id) => s.launcher.note_launch(&id),
                Cmd::Session(a) => match a {
                    SessionAction::Lock => ipc::action(Action::Lock),
                    SessionAction::LogOut => ipc::action(Action::Quit),
                    SessionAction::Suspend => s.services.send(m3_services::Command::Suspend),
                    SessionAction::Restart => s.services.send(m3_services::Command::Reboot),
                    SessionAction::ShutDown => s.services.send(m3_services::Command::PowerOff),
                },
            }
        })
    }

    // ---- monitors ----------------------------------------------------------

    fn watch_monitors(self: &Rc<Self>) {
        let Some(display) = gdk::Display::default() else { return };
        let monitors = display.monitors();
        let weak = Rc::downgrade(self);
        monitors.connect_items_changed(move |_, _, _, _| {
            if let Some(s) = weak.upgrade() {
                // Monitor metadata (connector) arrives slightly after the item.
                let w = Rc::downgrade(&s);
                glib::idle_add_local_once(move || {
                    if let Some(s) = w.upgrade() {
                        s.sync_monitors();
                    }
                });
            }
        });
        self.sync_monitors();
    }

    fn current_monitors() -> Vec<gdk::Monitor> {
        let Some(display) = gdk::Display::default() else { return Vec::new() };
        let list = display.monitors();
        (0..list.n_items()).filter_map(|i| list.item(i).and_downcast::<gdk::Monitor>()).collect()
    }

    fn sync_monitors(self: &Rc<Self>) {
        let monitors = Self::current_monitors();
        let names: Vec<String> = monitors.iter().map(connector).collect();
        self.backgrounds.borrow_mut().retain(|k, b| {
            let keep = names.contains(k);
            if !keep {
                b.window.destroy();
            }
            keep
        });
        self.panels.borrow_mut().retain(|k, p| {
            let keep = names.contains(k);
            if !keep {
                p.window.destroy();
            }
            keep
        });
        let panels_cfg = self.panels_cfg.borrow().clone();
        // Desktop widgets live on the first output.
        let primary = monitors.first().map(connector);
        let stale = self.desktop.borrow().as_ref().is_some_and(|(n, _)| Some(n) != primary.as_ref());
        if stale {
            if let Some((_, d)) = self.desktop.borrow_mut().take() {
                d.window.destroy();
            }
        }
        if let (Some(m), Some(name)) = (monitors.first(), primary) {
            if self.desktop.borrow().is_none() {
                let d =
                    crate::desktop::DesktopWidgets::new(&self.app, m, &self.widgets_cfg.borrow(), self.dispatcher());
                *self.desktop.borrow_mut() = Some((name, d));
            }
        }
        for (i, m) in monitors.iter().enumerate() {
            let name = connector(m);
            if !self.backgrounds.borrow().contains_key(&name) {
                let bg = Background::new(&self.app, m, self.theme.borrow().colors.clone());
                self.backgrounds.borrow_mut().insert(name.clone(), bg.clone());
                self.apply_wallpaper_to(&name, &bg);
            }
            let wants_panel = panels_cfg.all_outputs || i == 0;
            if wants_panel && !self.panels.borrow().contains_key(&name) {
                let p = Panel::new(&self.app, m, &panels_cfg, self.dispatcher(), Some(self.tray.clone()));
                p.update_tray(&self.tray_items.borrow());
                self.panels.borrow_mut().insert(name, p);
            }
        }
        self.refresh_workspaces();
        self.refresh_tasks();
        self.refresh_status();
    }

    fn rebuild_panels(self: &Rc<Self>) {
        for (_, p) in self.panels.borrow_mut().drain() {
            p.window.destroy();
        }
        self.sync_monitors();
    }

    // ---- theme & wallpaper ---------------------------------------------------

    fn apply_wallpaper_to(self: &Rc<Self>, output: &str, bg: &Rc<Background>) {
        let cfg = self.theme_cfg.borrow().clone();
        let path = wallpaper_path(&cfg, Some(output), self.slide.borrow().as_ref());
        let weak = Rc::downgrade(self);
        bg.set_wallpaper(path, &cfg, move |seed| {
            if let Some(s) = weak.upgrade() {
                s.wallpaper_seed_found(seed);
            }
        });
    }

    fn wallpaper_seed_found(self: &Rc<Self>, seed: u32) {
        if *self.wallpaper_seed.borrow() == Some(seed) {
            return;
        }
        *self.wallpaper_seed.borrow_mut() = Some(seed);
        if self.theme_cfg.borrow().source == ColorSource::Wallpaper {
            info!("new wallpaper colour #{seed:06x}");
            self.reapply_theme();
            // The compositor reads the cached seed on reload.
            ipc::send(Request::Reload);
        }
    }

    fn reapply_theme(self: &Rc<Self>) {
        let cfg = self.theme_cfg.borrow().clone();
        let theme = Theme::resolve(&cfg, *self.wallpaper_seed.borrow(), m3_core::time::local_minutes());
        if theme == *self.theme.borrow() {
            return;
        }
        self.style.apply(&cfg, &theme);
        for bg in self.backgrounds.borrow().values() {
            bg.set_colors(theme.colors.clone());
        }
        *self.theme.borrow_mut() = theme;
    }

    fn watch_theme_schedule(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let last_dnd = std::cell::Cell::new(None::<bool>);
        glib::timeout_add_seconds_local(60, move || {
            let Some(s) = weak.upgrade() else { return glib::ControlFlow::Break };
            if s.theme_cfg.borrow().mode == cfg::ThemeMode::Auto {
                s.reapply_theme();
            }
            // Do Not Disturb schedule: switch at the boundaries only, so a
            // manual toggle in between is respected.
            let prefs = cfg::load_or(&s.config_dir, cfg::NotificationsConfig::default()).0;
            let want = prefs.scheduled_dnd(m3_core::time::local_minutes());
            if prefs.dnd_schedule && last_dnd.get() != Some(want) {
                s.services.send(Command::SetDoNotDisturb(want));
            }
            last_dnd.set(prefs.dnd_schedule.then_some(want));
            glib::ControlFlow::Continue
        });
    }

    // ---- configuration --------------------------------------------------------

    fn watch_config(self: &Rc<Self>) {
        let _ = std::fs::create_dir_all(&self.config_dir);
        let dir = gio::File::for_path(&self.config_dir);
        match dir.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            Ok(mon) => {
                mon.set_rate_limit(250);
                let weak = Rc::downgrade(self);
                mon.connect_changed(move |_, file, other, event| {
                    use gio::FileMonitorEvent as E;
                    if !matches!(event, E::ChangesDoneHint | E::Created | E::Deleted | E::MovedIn | E::Renamed) {
                        return;
                    }
                    let Some(s) = weak.upgrade() else { return };
                    let names: Vec<String> = [Some(file), other]
                        .into_iter()
                        .flatten()
                        .filter_map(|f| f.basename())
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    if names.iter().any(|n| n == "theme.toml") {
                        s.reload_theme_config();
                    }
                    if names.iter().any(|n| n == "panels.toml") {
                        s.reload_panels_config();
                    }
                    if names.iter().any(|n| n == "widgets.toml") {
                        let (c, err) = cfg::load_or(&s.config_dir, s.widgets_cfg.borrow().clone());
                        if let Some(e) = err {
                            warn!("widgets.toml: {e}");
                        }
                        *s.widgets_cfg.borrow_mut() = c.clone();
                        if let Some((_, d)) = s.desktop.borrow().as_ref() {
                            d.apply_config(&c);
                        }
                    }
                });
                self._monitors.borrow_mut().push(mon);
            }
            Err(e) => warn!("cannot watch {}: {e}", self.config_dir.display()),
        }
    }

    fn reload_theme_config(self: &Rc<Self>) {
        let prev = self.theme_cfg.borrow().clone();
        let (new, err) = cfg::load_or(&self.config_dir, prev.clone());
        if err.is_some() || new == prev {
            return;
        }
        *self.theme_cfg.borrow_mut() = new.clone();
        if new.wallpaper.slideshow_dir != prev.wallpaper.slideshow_dir
            || new.wallpaper.slideshow_minutes != prev.wallpaper.slideshow_minutes
        {
            self.restart_slideshow();
        }
        if new.wallpaper != prev.wallpaper || new.reduced_motion != prev.reduced_motion {
            let seed = wallpaper_path(&new, None, self.slide.borrow().as_ref())
                .and_then(|p| m3_theme::wallpaper::cached_seed(&p));
            *self.wallpaper_seed.borrow_mut() = seed;
            let bgs: Vec<(String, Rc<Background>)> =
                self.backgrounds.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (name, bg) in bgs {
                self.apply_wallpaper_to(&name, &bg);
            }
        }
        self.reapply_theme();
    }

    fn reload_panels_config(self: &Rc<Self>) {
        let prev = self.panels_cfg.borrow().clone();
        let (new, err) = cfg::load_or(&self.config_dir, prev.clone());
        if err.is_some() || new == prev {
            return;
        }
        let only_pins = PanelsConfig { pinned: prev.pinned.clone(), ..new.clone() } == prev;
        self.quick.set_clock_12h(new.clock_format == m3_core::config::ClockFormat::H12);
        *self.panels_cfg.borrow_mut() = new;
        if only_pins {
            self.refresh_tasks();
        } else {
            self.rebuild_panels();
        }
    }

    fn watch_apps(self: &Rc<Self>) {
        let monitor = gio::AppInfoMonitor::get();
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_| {
            if let Some(s) = weak.upgrade() {
                *s.apps.borrow_mut() = apps::all_apps();
                s.refresh_tasks();
            }
        });
        // Keep the monitor alive for the life of the process.
        std::mem::forget(monitor);
    }

    // ---- compositor events ----------------------------------------------------

    fn listen_ipc(self: &Rc<Self>) {
        let rx = ipc::subscribe();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(s) = weak.upgrade() else { break };
                match msg {
                    IpcMsg::Connected => {}
                    IpcMsg::Disconnected => warn!("lost the compositor connection; reconnecting"),
                    IpcMsg::Event(e) => s.on_event(e),
                }
            }
        });
    }

    fn on_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::ScreenshotSaved { path } => self.screenshot_saved(path),
            Event::WorkspacesChanged { workspaces } => {
                let prev = self.workspaces.borrow().iter().find(|w| w.active).map(|w| w.index);
                let now = workspaces.iter().find(|w| w.active).map(|w| (w.index, w.name.clone()));
                *self.workspaces.borrow_mut() = workspaces;
                self.refresh_workspaces();
                if let (Some(p), Some((i, name))) = (prev, now) {
                    if p != i {
                        self.osd.show("space_dashboard", None, &format!("Workspace {name}"));
                    }
                }
            }
            Event::WindowsChanged { windows } => {
                *self.windows.borrow_mut() = windows;
                self.refresh_tasks();
            }
            Event::FocusChanged { window } => {
                for w in self.windows.borrow_mut().iter_mut() {
                    w.focused = Some(w.id) == window;
                }
                self.refresh_tasks();
            }
            Event::ShellAction { action } => self.on_shell_action(action),
            Event::LockChanged { locked } => {
                if locked {
                    self.launcher.hide();
                    self.overview.hide();
                    self.quick.hide();
                    self.notifications.hide_center();
                }
            }
            Event::OutputsChanged { .. } | Event::ConfigReloaded => {}
        }
    }

    /// Low-battery warnings at 20 %, 10 % and 5 % while discharging (each
    /// once per discharge; the last one is critical and stays on screen).
    fn check_battery(self: &Rc<Self>, b: &m3_services::BatteryState) {
        const ID: u32 = 0xFFFF_0B47;
        if !b.present || !b.on_battery {
            if self.battery_warned.get() != u32::MAX {
                self.battery_warned.set(u32::MAX);
                self.notifications.remove(ID, m3_services::CloseReason::Closed);
            }
            return;
        }
        let pct = b.percentage.round() as u32;
        let Some(threshold) = low_battery_threshold(pct, self.battery_warned.get()) else { return };
        self.battery_warned.set(threshold);
        let critical = threshold <= 5;
        let left = b.time_to_empty.filter(|s| *s > 0).map(|s| {
            let m = s / 60;
            if m >= 60 {
                format!(" — about {} h {} min left", m / 60, m % 60)
            } else {
                format!(" — about {m} min left")
            }
        });
        let n = m3_services::Notification {
            id: ID,
            replaces: true,
            app_name: "Power".into(),
            app_icon: if critical { "battery-empty".into() } else { "battery-caution".into() },
            summary: if critical { "Battery critically low".into() } else { "Battery low".into() },
            body: format!(
                "{pct}% remaining{}.{}",
                left.unwrap_or_default(),
                if critical { " Plug in now to avoid losing work." } else { " Plug in soon." }
            ),
            actions: Vec::new(),
            urgency: if critical {
                m3_services::notifications::Urgency::Critical
            } else {
                m3_services::notifications::Urgency::Normal
            },
            desktop_entry: None,
            category: Some("device".into()),
            image_path: None,
            image: None,
            transient: false,
            resident: false,
            expire_timeout: if critical { 0 } else { -1 },
            timestamp: chrono::Utc::now().timestamp(),
        };
        self.notifications.add(n, true);
    }

    /// Close every sheet (before a screenshot, for example).
    fn toggle_sheet_close_all(&self) {
        self.launcher.hide();
        self.quick.hide();
        self.notifications.hide_center();
    }

    /// "Screenshot saved" with the picture, Open and Show in folder.
    fn screenshot_saved(self: &Rc<Self>, path: std::path::PathBuf) {
        let id = 0xFFFF_5000 | (self.screenshot_count.get() & 0xFFF);
        self.screenshot_count.set(self.screenshot_count.get().wrapping_add(1));
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let n = m3_services::Notification {
            id,
            replaces: false,
            app_name: "Screenshot".into(),
            app_icon: "applets-screenshooter".into(),
            summary: "Screenshot saved".into(),
            body: name,
            actions: vec![
                ("default".into(), "Open".into()),
                ("open".into(), "Open".into()),
                ("folder".into(), "Show in folder".into()),
            ],
            urgency: m3_services::notifications::Urgency::Low,
            desktop_entry: None,
            category: Some("transfer.complete".into()),
            image_path: Some(path.to_string_lossy().into_owned()),
            image: None,
            transient: false,
            resident: false,
            expire_timeout: -1,
            timestamp: chrono::Utc::now().timestamp(),
        };
        self.notifications.add_internal(n, move |key| crate::screenshot::open(&path, key == "folder"));
    }

    /// Open one of the shell's sheets, closing the others: only one of
    /// launcher, quick settings and notification centre is open at a time.
    fn toggle_sheet(self: &Rc<Self>, which: Sheet) {
        if which != Sheet::Launcher {
            self.launcher.hide();
        }
        if which != Sheet::QuickSettings {
            self.quick.hide();
        }
        if which != Sheet::Notifications {
            self.notifications.hide_center();
        }
        match which {
            Sheet::Launcher => self.launcher.toggle(),
            Sheet::QuickSettings => {
                self.notifications.hide_popups();
                self.quick.toggle();
            }
            Sheet::Notifications => self.notifications.toggle_center(),
        }
    }

    fn on_shell_action(self: &Rc<Self>, action: Action) {
        let step = 0.05;
        match action {
            Action::ScreenshotRegion => {
                self.toggle_sheet_close_all();
                crate::screenshot::select_region(&self.app);
            }
            Action::ToggleLauncher => self.toggle_sheet(Sheet::Launcher),
            Action::ToggleOverview => {
                self.launcher.hide();
                self.quick.hide();
                self.notifications.hide_center();
                self.overview.toggle();
            }
            Action::ToggleQuickSettings => self.toggle_sheet(Sheet::QuickSettings),
            Action::ToggleNotifications => self.toggle_sheet(Sheet::Notifications),
            Action::VolumeUp => self.audio_osd(AudioCmd::StepOutputVolume(step), step),
            Action::VolumeDown => self.audio_osd(AudioCmd::StepOutputVolume(-step), -step),
            Action::VolumeMute => self.audio_osd(AudioCmd::ToggleOutputMute, 0.0),
            Action::MicMute => {
                self.audio.send(AudioCmd::ToggleInputMute);
                let muted = self.sys.borrow().audio.as_ref().and_then(|a| a.input()).is_some_and(|i| !i.muted);
                self.osd.show(
                    if muted { "mic_off" } else { "mic" },
                    None,
                    if muted { "Microphone muted" } else { "Microphone on" },
                );
            }
            Action::BrightnessUp => self.services.send(Command::StepBrightness(step)),
            Action::BrightnessDown => self.services.send(Command::StepBrightness(-step)),
            Action::MediaPlayPause => self.services.send(Command::MediaPlayPause),
            Action::MediaNext => self.services.send(Command::MediaNext),
            Action::MediaPrevious => self.services.send(Command::MediaPrevious),
            other => info!("unexpected shell action {other:?}"),
        }
    }

    /// Apply an audio change and show the predicted level immediately (the
    /// real level arrives moments later from the sound server).
    fn audio_osd(&self, cmd: AudioCmd, delta: f64) {
        let current = self.sys.borrow().audio.as_ref().and_then(|a| a.output().cloned());
        self.audio.send(cmd.clone());
        let Some(o) = current else {
            self.osd.show("volume_off", None, "No audio output");
            return;
        };
        let (level, muted) = match cmd {
            AudioCmd::ToggleOutputMute => (o.volume, !o.muted),
            _ => ((o.volume + delta).clamp(0.0, 1.0), if delta > 0.0 { false } else { o.muted }),
        };
        let shown = if muted { 0.0 } else { level };
        self.osd.show(status::volume_icon(level, muted), Some(shown), if muted { "Muted" } else { "Volume" });
    }

    fn listen_services(
        self: &Rc<Self>,
        events: async_channel::Receiver<SvcEvent>,
        audio: async_channel::Receiver<AudioState>,
    ) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(e) = events.recv().await {
                let Some(s) = weak.upgrade() else { break };
                s.on_service_event(e);
            }
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(a) = audio.recv().await {
                let Some(s) = weak.upgrade() else { break };
                s.sys.borrow_mut().audio = Some(a);
                s.refresh_status();
            }
        });
    }

    fn listen_tray(self: &Rc<Self>, events: async_channel::Receiver<m3_services::tray::TrayEvent>) {
        use m3_services::tray::TrayEvent;
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(e) = events.recv().await {
                let Some(s) = weak.upgrade() else { break };
                match e {
                    TrayEvent::Items(items) => {
                        for p in s.panels.borrow().values() {
                            p.update_tray(&items);
                        }
                        *s.tray_items.borrow_mut() = items;
                    }
                    TrayEvent::Menu { key, menu } => {
                        for p in s.panels.borrow().values() {
                            p.show_tray_menu(&key, &menu);
                        }
                    }
                }
            }
        });
    }

    fn on_service_event(self: &Rc<Self>, e: SvcEvent) {
        match e {
            SvcEvent::Notification(n) => {
                let prefs = cfg::load_or(&self.config_dir, cfg::NotificationsConfig::default()).0;
                let app = prefs.app(&n.app_name);
                if !app.enabled {
                    // Blocked app: close it right away so the sender knows.
                    self.services
                        .send(Command::CloseNotification { id: n.id, reason: m3_services::CloseReason::Dismissed });
                } else {
                    self.notifications.add(n, prefs.popups && app.popups);
                }
            }
            SvcEvent::NotificationClosed { id } => self.notifications.remove(id, m3_services::CloseReason::Closed),
            SvcEvent::ClearAllNotifications => self.notifications.clear_all(),
            SvcEvent::DoNotDisturb(on) => {
                self.sys.borrow_mut().dnd = on;
                self.notifications.set_dnd(on);
            }
            SvcEvent::Network(n) => self.sys.borrow_mut().network = Some(n),
            SvcEvent::Bluetooth(b) => self.sys.borrow_mut().bluetooth = Some(b),
            SvcEvent::Battery(b) => {
                self.check_battery(&b);
                self.sys.borrow_mut().battery = Some(b);
            }
            SvcEvent::PowerProfiles(p) => self.sys.borrow_mut().profiles = Some(p),
            SvcEvent::Brightness(b) => {
                let changed = self.sys.borrow().brightness.is_some() && b != self.sys.borrow().brightness;
                self.sys.borrow_mut().brightness = b;
                if let (true, Some(v)) = (changed, b) {
                    self.osd.show("brightness_6", Some(v), "Brightness");
                }
            }
            SvcEvent::PrepareForSleep => {
                let lock = cfg::load_or(&self.config_dir, cfg::CompositorConfig::default()).0.idle.lock_on_suspend;
                if lock {
                    ipc::action(Action::Lock);
                }
                // Give the lock screen a moment to cover every output.
                let services = self.services.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(if lock { 750 } else { 0 }), move || {
                    services.send(Command::ReadyForSleep);
                });
            }
            SvcEvent::Error(msg) => self.osd.show("error", None, &msg),
        }
        self.refresh_status();
    }

    // ---- slideshow ------------------------------------------------------------

    fn slideshow_images(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        matches!(
                            p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(),
                            Some("png" | "jpg" | "jpeg" | "webp")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    fn next_slide(&self) -> Option<PathBuf> {
        let cfg = self.theme_cfg.borrow();
        let dir = &cfg.wallpaper.slideshow_dir;
        if dir.as_os_str().is_empty() {
            return None;
        }
        let images = Self::slideshow_images(dir);
        if images.is_empty() {
            return None;
        }
        let current = self.slide.borrow().clone();
        let pos = current.as_ref().and_then(|c| images.iter().position(|p| p == c));
        let next = if cfg.wallpaper.slideshow_shuffle && images.len() > 1 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0);
            let mut i = nanos % images.len();
            if Some(i) == pos {
                i = (i + 1) % images.len();
            }
            i
        } else {
            pos.map(|p| (p + 1) % images.len()).unwrap_or(0)
        };
        Some(images[next].clone())
    }

    fn advance_slideshow(self: &Rc<Self>) {
        let next = self.next_slide();
        *self.slide.borrow_mut() = next;
        let bgs: Vec<(String, Rc<Background>)> =
            self.backgrounds.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, bg) in bgs {
            self.apply_wallpaper_to(&name, &bg);
        }
    }

    fn restart_slideshow(self: &Rc<Self>) {
        if let Some(id) = self.slide_timer.borrow_mut().take() {
            id.remove();
        }
        let (dir_set, minutes) = {
            let c = self.theme_cfg.borrow();
            (!c.wallpaper.slideshow_dir.as_os_str().is_empty(), c.wallpaper.slideshow_minutes.max(1))
        };
        if !dir_set {
            if self.slide.borrow_mut().take().is_some() {
                self.advance_slideshow();
            }
            return;
        }
        self.advance_slideshow();
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_seconds_local(minutes * 60, move || {
            let Some(s) = weak.upgrade() else { return glib::ControlFlow::Break };
            s.advance_slideshow();
            glib::ControlFlow::Continue
        });
        *self.slide_timer.borrow_mut() = Some(id);
    }

    fn refresh_status(self: &Rc<Self>) {
        let st = self.sys.borrow().clone();
        for p in self.panels.borrow().values() {
            p.update_status(&st);
        }
        self.quick.update(&st);
    }

    fn refresh_workspaces(&self) {
        let ws = self.workspaces.borrow();
        self.overview.update(&ws, &self.windows.borrow(), &self.apps.borrow());
        for p in self.panels.borrow().values() {
            p.update_workspaces(&ws);
        }
    }

    fn refresh_tasks(&self) {
        let windows = self.windows.borrow();
        let apps = self.apps.borrow();
        self.overview.update(&self.workspaces.borrow(), &windows, &apps);
        let pinned = self.panels_cfg.borrow().pinned.clone();
        for p in self.panels.borrow().values() {
            p.update_tasks(&windows, &apps, &pinned);
        }
    }
}

fn connector(m: &gdk::Monitor) -> String {
    m.connector().map(|c| c.to_string()).unwrap_or_else(|| "unknown".into())
}

/// The wallpaper file for an output (per-output override, slideshow pick, or
/// the global path). `None` means the built-in m3 wallpaper.
fn wallpaper_path(cfg: &ThemeConfig, output: Option<&str>, slide: Option<&PathBuf>) -> Option<PathBuf> {
    if let Some(p) = output.and_then(|o| cfg.wallpaper.outputs.get(o)) {
        return Some(p.clone());
    }
    if let Some(p) = slide {
        return Some(p.clone());
    }
    let p = &cfg.wallpaper.path;
    (!p.as_os_str().is_empty()).then(|| p.clone())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sheet {
    Launcher,
    QuickSettings,
    Notifications,
}

/// The warning threshold newly crossed at `pct`, given the lowest one already
/// warned about this discharge (`u32::MAX` = none yet).
fn low_battery_threshold(pct: u32, warned: u32) -> Option<u32> {
    [5, 10, 20].into_iter().find(|t| pct <= *t && *t < warned)
}

#[cfg(test)]
mod battery_tests {
    use super::low_battery_threshold;

    #[test]
    fn warns_once_per_threshold() {
        assert_eq!(low_battery_threshold(50, u32::MAX), None);
        assert_eq!(low_battery_threshold(20, u32::MAX), Some(20));
        assert_eq!(low_battery_threshold(18, 20), None);
        assert_eq!(low_battery_threshold(9, 20), Some(10));
        // Dropping straight to 4 % warns with the critical threshold.
        assert_eq!(low_battery_threshold(4, u32::MAX), Some(5));
        assert_eq!(low_battery_threshold(3, 5), None);
    }
}
