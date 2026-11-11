//! The desktop panel: launcher button, workspaces, tasks, clock.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_core::config::{self as cfg, ClockFormat, PanelModule, PanelPosition, PanelVisibility, PanelsConfig};
use m3_protocol::{Action, WindowInfo, WorkspaceInfo};

use crate::{apps::App, icons, ipc, shell::Cmd};

pub struct Panel {
    pub window: gtk::Window,
    cfg: PanelsConfig,
    monitor: gdk::Monitor,
    revealer: gtk::Revealer,
    workspaces: Option<gtk::Box>,
    tasks: Option<gtk::Box>,
    status: Option<gtk::Box>,
    bell: Option<(gtk::Label, gtk::Label)>,
    clock: Option<gtk::Label>,
    tray: Option<Rc<crate::tray::TrayView>>,
    clock_timer: RefCell<Option<glib::SourceId>>,
    dispatch: Rc<dyn Fn(Cmd)>,
    hovered: Cell<bool>,
    overlapped: Cell<bool>,
    /// Task buttons by group key, with the data they were built from, so an
    /// update only rebuilds the buttons that changed.
    task_cache: RefCell<Vec<(String, String, gtk::Widget)>>,
    /// What the status area currently shows.
    status_sig: RefCell<String>,
}

impl Panel {
    pub fn new(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        cfg: &PanelsConfig,
        dispatch: Rc<dyn Fn(Cmd)>,
        tray_handle: Option<m3_services::tray::TrayHandle>,
    ) -> Rc<Self> {
        crate::placement::set_panel_position(cfg.position);
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_namespace(Some("m3-panel"));
        window.set_monitor(Some(monitor));
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-panel-window");

        let vertical = cfg.position.is_vertical();
        let (edge, sides) = match cfg.position {
            PanelPosition::Top => (Edge::Top, [Edge::Left, Edge::Right]),
            PanelPosition::Bottom => (Edge::Bottom, [Edge::Left, Edge::Right]),
            PanelPosition::Left => (Edge::Left, [Edge::Top, Edge::Bottom]),
            PanelPosition::Right => (Edge::Right, [Edge::Top, Edge::Bottom]),
        };
        window.set_anchor(edge, true);
        for s in sides {
            window.set_anchor(s, true);
        }
        let margin = if cfg.floating { 8 } else { 0 };
        for e in [edge, sides[0], sides[1]] {
            window.set_margin(e, margin);
        }

        let orientation = if vertical { gtk::Orientation::Vertical } else { gtk::Orientation::Horizontal };
        let bar = gtk::CenterBox::new();
        bar.set_orientation(orientation);
        bar.add_css_class("m3-panel");
        if cfg.floating {
            bar.add_css_class("floating");
        }
        if cfg.style == m3_core::config::PanelStyle::Islands {
            bar.add_css_class("islands");
        }
        if vertical {
            bar.set_width_request(cfg.size as i32);
        } else {
            bar.set_height_request(cfg.size as i32);
        }

        let section = || {
            let b = gtk::Box::new(orientation, 4);
            b.set_valign(gtk::Align::Center);
            b.set_halign(gtk::Align::Center);
            b
        };
        let (start, center, end) = (section(), section(), section());
        bar.set_start_widget(Some(&start));
        bar.set_center_widget(Some(&center));
        bar.set_end_widget(Some(&end));

        let mut workspaces = None;
        let mut tasks = None;
        let mut status = None;
        let mut bell = None;
        let mut clock = None;
        let mut tray = None;
        for (container, modules) in [(&start, &cfg.start), (&center, &cfg.center), (&end, &cfg.end)] {
            for m in modules {
                match m {
                    PanelModule::Launcher => container.append(&launcher_button(dispatch.clone())),
                    PanelModule::Workspaces => {
                        let b = gtk::Box::new(orientation, 6);
                        b.add_css_class("m3-workspaces");
                        b.set_valign(gtk::Align::Center);
                        let scroll = gtk::EventControllerScroll::new(
                            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
                        );
                        scroll.connect_scroll(|_, dx, dy| {
                            let d = if dy.abs() > dx.abs() { dy } else { dx };
                            if d > 0.0 {
                                ipc::action(Action::NextWorkspace);
                            } else if d < 0.0 {
                                ipc::action(Action::PreviousWorkspace);
                            }
                            glib::Propagation::Stop
                        });
                        b.add_controller(scroll);
                        container.append(&b);
                        workspaces = Some(b);
                    }
                    PanelModule::Tasks => {
                        let b = gtk::Box::new(orientation, 4);
                        b.add_css_class("m3-tasks");
                        container.append(&b);
                        tasks = Some(b);
                    }
                    PanelModule::Clock => {
                        let (button, label) = clock_widget(cfg);
                        container.append(&button);
                        clock = Some(label);
                    }
                    PanelModule::Status => {
                        let button = gtk::Button::new();
                        button.add_css_class("m3-status-button");
                        button.set_tooltip_text(Some("Quick settings"));
                        let icons_box = gtk::Box::new(orientation, 6);
                        button.set_child(Some(&icons_box));
                        let d = dispatch.clone();
                        button.connect_clicked(move |_| d(Cmd::ToggleQuickSettings));
                        container.append(&button);
                        status = Some(icons_box);
                    }
                    PanelModule::Notifications => {
                        let button = gtk::Button::new();
                        button.add_css_class("m3-bell");
                        let overlay = gtk::Overlay::new();
                        let icon = icons::symbol("notifications");
                        overlay.set_child(Some(&icon));
                        let badge = gtk::Label::new(None);
                        badge.add_css_class("m3-badge");
                        badge.set_halign(gtk::Align::End);
                        badge.set_valign(gtk::Align::Start);
                        badge.set_visible(false);
                        overlay.add_overlay(&badge);
                        button.set_child(Some(&overlay));
                        button.set_tooltip_text(Some("Notifications"));
                        let d = dispatch.clone();
                        button.connect_clicked(move |_| d(Cmd::ToggleNotifications));
                        container.append(&button);
                        bell = Some((icon, badge));
                    }
                    // StatusNotifierItem icons; hidden while there are none.
                    PanelModule::Tray => {
                        if let Some(h) = tray_handle.clone() {
                            let view = crate::tray::TrayView::new(h, orientation);
                            container.append(view.widget());
                            tray = Some(view);
                        }
                    }
                    // A user menu is not part of this build; the module is
                    // skipped rather than shown as an inert placeholder.
                    PanelModule::User => {}
                    PanelModule::Power => container.append(&power_button(dispatch.clone())),
                    PanelModule::Search => {
                        let b = gtk::Button::new();
                        b.add_css_class("m3-panel-search");
                        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                        row.append(&icons::symbol_with("search", &["small"]));
                        if !vertical {
                            row.append(&gtk::Label::new(Some("Search")));
                        }
                        b.set_child(Some(&row));
                        b.set_tooltip_text(Some("Search apps and files"));
                        b.update_property(&[gtk::accessible::Property::Label("Search")]);
                        let d = dispatch.clone();
                        b.connect_clicked(move |_| d(Cmd::ToggleLauncher));
                        container.append(&b);
                    }
                }
            }
        }

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(match cfg.position {
            PanelPosition::Top => gtk::RevealerTransitionType::SlideDown,
            PanelPosition::Bottom => gtk::RevealerTransitionType::SlideUp,
            PanelPosition::Left => gtk::RevealerTransitionType::SlideRight,
            PanelPosition::Right => gtk::RevealerTransitionType::SlideLeft,
        });
        revealer.set_transition_duration(250);
        revealer.set_child(Some(&bar));
        revealer.set_reveal_child(true);

        // A hover strip keeps hidden panels reachable.
        let holder = gtk::Box::new(orientation, 0);
        if vertical {
            revealer.set_vexpand(true)
        } else {
            revealer.set_hexpand(true)
        }
        holder.append(&revealer);
        if vertical {
            holder.set_width_request(2)
        } else {
            holder.set_height_request(2)
        }
        window.set_child(Some(&holder));

        let panel = Rc::new(Self {
            window,
            cfg: cfg.clone(),
            monitor: monitor.clone(),
            revealer,
            workspaces,
            tasks,
            status,
            bell,
            clock,
            tray,
            clock_timer: RefCell::new(None),
            dispatch,
            hovered: Cell::new(false),
            overlapped: Cell::new(false),
            task_cache: RefCell::new(Vec::new()),
            status_sig: RefCell::new(String::new()),
        });

        match cfg.visibility {
            PanelVisibility::Always => panel.window.auto_exclusive_zone_enable(),
            PanelVisibility::Autohide | PanelVisibility::Intelligent => {
                panel.window.set_exclusive_zone(0);
                let motion = gtk::EventControllerMotion::new();
                let weak = Rc::downgrade(&panel);
                motion.connect_enter(move |_, _, _| {
                    if let Some(p) = weak.upgrade() {
                        p.hovered.set(true);
                        p.update_reveal();
                    }
                });
                let weak = Rc::downgrade(&panel);
                motion.connect_leave(move |_| {
                    if let Some(p) = weak.upgrade() {
                        p.hovered.set(false);
                        let weak = Rc::downgrade(&p);
                        glib::timeout_add_local_once(Duration::from_millis(600), move || {
                            if let Some(p) = weak.upgrade() {
                                p.update_reveal();
                            }
                        });
                    }
                });
                panel.window.add_controller(motion);
                panel.update_reveal();
            }
        }

        panel.start_clock();
        panel.window.present();
        panel
    }

    fn update_reveal(&self) {
        let show = match self.cfg.visibility {
            PanelVisibility::Always => true,
            PanelVisibility::Autohide => self.hovered.get(),
            PanelVisibility::Intelligent => self.hovered.get() || !self.overlapped.get(),
        };
        self.revealer.set_reveal_child(show);
    }

    /// Schedule clock updates at minute (or second) boundaries.
    fn start_clock(self: &Rc<Self>) {
        let Some(label) = self.clock.clone() else { return };
        let cfg = self.cfg.clone();
        let weak = Rc::downgrade(self);
        let tick = move || -> Duration {
            let now = chrono::Local::now();
            let text = format_clock(&now, &cfg);
            if label.text() != text {
                label.set_text(&text);
            }
            let next = if cfg.clock_seconds {
                1000 - now.timestamp_subsec_millis() as u64
            } else {
                use chrono::Timelike;
                (60 - now.second() as u64) * 1000 - now.timestamp_subsec_millis() as u64
            };
            // Timers don't run during suspend: re-check at least every 10 s
            // so the clock is right soon after waking up.
            Duration::from_millis(next.clamp(50, 10_000))
        };
        fn schedule(tick: Rc<dyn Fn() -> Duration>, weak: std::rc::Weak<Panel>) {
            let d = tick();
            let w = weak.clone();
            let id = glib::timeout_add_local_once(d, move || {
                if w.upgrade().is_some() {
                    schedule(tick, w);
                }
            });
            if let Some(p) = weak.upgrade() {
                *p.clock_timer.borrow_mut() = Some(id);
            }
        }
        schedule(Rc::new(tick), weak);
    }

    pub fn update_tray(&self, items: &[m3_services::tray::TrayItem]) {
        if let Some(t) = &self.tray {
            t.update(items);
        }
    }

    pub fn show_tray_menu(&self, key: &str, menu: &m3_services::tray::MenuNode) {
        if let Some(t) = &self.tray {
            t.show_menu(key, menu);
        }
    }

    pub fn update_status(&self, st: &crate::status::SysState) {
        if let Some(b) = self.status.as_ref().filter(|_| {
            let sig = format!(
                "{:?}|{:?}|{:?}|{:?}|{}",
                st.network.as_ref().and_then(crate::status::network_icon),
                st.bluetooth.as_ref().map(|b| (b.available, b.powered, b.devices.iter().any(|d| d.connected))),
                st.audio.as_ref().map(|a| (
                    a.output().map(|o| ((o.volume * 100.0).round() as i32, o.muted)),
                    a.input().map(|i| i.muted)
                )),
                st.battery.as_ref().map(|b| (b.present, b.percentage.round() as i32, crate::status::battery_icon(b))),
                self.cfg.battery_percentage,
            );
            let changed = *self.status_sig.borrow() != sig;
            if changed {
                *self.status_sig.borrow_mut() = sig;
            }
            changed
        }) {
            while let Some(c) = b.first_child() {
                b.remove(&c);
            }
            let mut descr = Vec::new();
            if let Some((icon, text)) = st.network.as_ref().and_then(crate::status::network_icon) {
                b.append(&icons::symbol_with(icon, &["small"]));
                descr.push(text);
            }
            if let Some(bt) = st.bluetooth.as_ref().filter(|b| b.available && b.powered) {
                let connected = bt.devices.iter().any(|d| d.connected);
                b.append(&icons::symbol_with(if connected { "bluetooth_connected" } else { "bluetooth" }, &["small"]));
                descr.push(if connected { "Bluetooth device connected".into() } else { "Bluetooth on".into() });
            }
            if let Some(o) = st.audio.as_ref().and_then(|a| a.output()) {
                b.append(&icons::symbol_with(crate::status::volume_icon(o.volume, o.muted), &["small"]));
                descr.push(if o.muted { "Muted".into() } else { format!("Volume {:.0}%", o.volume * 100.0) });
            }
            if st.audio.as_ref().and_then(|a| a.input()).is_some_and(|i| i.muted) {
                b.append(&icons::symbol_with("mic_off", &["small"]));
                descr.push("Microphone muted".into());
            }
            if let Some(bat) = st.battery.as_ref().filter(|b| b.present) {
                b.append(&icons::symbol_with(crate::status::battery_icon(bat), &["small"]));
                if self.cfg.battery_percentage {
                    let l = gtk::Label::new(Some(&format!("{:.0}%", bat.percentage)));
                    l.add_css_class("m3-label-large");
                    b.append(&l);
                }
                descr.push(crate::status::battery_text(bat));
            }
            if b.first_child().is_none() {
                b.append(&icons::symbol_with("tune", &["small"]));
            }
            let text = if descr.is_empty() { "Quick settings".to_owned() } else { descr.join(", ") };
            if let Some(button) = b.parent() {
                button.set_tooltip_text(Some(&text));
                button.update_property(&[gtk::accessible::Property::Label(&format!("Quick settings: {text}"))]);
            }
        }
        if let Some((icon, badge)) = &self.bell {
            icon.set_text(if st.dnd {
                "notifications_off"
            } else if st.unread > 0 {
                "notifications_unread"
            } else {
                "notifications"
            });
            badge.set_visible(st.unread > 0);
            badge.set_text(&if st.unread > 99 { "99+".to_owned() } else { st.unread.to_string() });
        }
    }

    pub fn update_workspaces(&self, workspaces: &[WorkspaceInfo]) {
        tracing::debug!(count = workspaces.len(), has_module = self.workspaces.is_some(), "update workspaces");
        let Some(b) = &self.workspaces else { return };
        // Reuse the buttons (by index) so the active pill animates between
        // workspaces instead of being recreated.
        let mut existing: Vec<gtk::Button> = Vec::new();
        let mut c = b.first_child();
        while let Some(w) = c {
            c = w.next_sibling();
            match w.downcast::<gtk::Button>() {
                Ok(btn) if existing.len() < workspaces.len() => existing.push(btn),
                Ok(btn) => b.remove(&btn),
                Err(w) => b.remove(&w),
            }
        }
        for (i, ws) in workspaces.iter().enumerate() {
            let button = match existing.get(i) {
                Some(btn) => btn.clone(),
                None => {
                    let button = gtk::Button::new();
                    button.add_css_class("m3-ws");
                    button.set_valign(gtk::Align::Center);
                    let index = ws.index;
                    button.connect_clicked(move |_| ipc::action(Action::SwitchWorkspace { index }));
                    b.append(&button);
                    button
                }
            };
            button.remove_css_class("active");
            button.remove_css_class("occupied");
            if ws.active {
                button.add_css_class("active");
                match button.child().and_downcast::<gtk::Label>() {
                    Some(l) => l.set_text(&ws.name),
                    None => {
                        let l = gtk::Label::new(Some(&ws.name));
                        l.add_css_class("m3-label-medium");
                        button.set_child(Some(&l));
                    }
                }
            } else {
                button.set_child(None::<&gtk::Widget>);
                if ws.window_count > 0 {
                    button.add_css_class("occupied");
                }
            }
            let label = format!(
                "Workspace {} — {}{}",
                ws.name,
                ws.window_count,
                if ws.window_count == 1 { " window" } else { " windows" }
            );
            button.set_tooltip_text(Some(&label));
            button.update_property(&[gtk::accessible::Property::Label(&label)]);
        }
    }

    pub fn update_tasks(&self, windows: &[WindowInfo], apps: &[App], pinned: &[String]) {
        // Intelligent hide: a window overlapping the panel strip.
        if self.cfg.visibility == PanelVisibility::Intelligent {
            let g = self.monitor.geometry();
            let band = self.cfg.size as i32 + 16;
            let strip = match self.cfg.position {
                PanelPosition::Top => (g.x(), g.y(), g.width(), band),
                PanelPosition::Bottom => (g.x(), g.y() + g.height() - band, g.width(), band),
                PanelPosition::Left => (g.x(), g.y(), band, g.height()),
                PanelPosition::Right => (g.x() + g.width() - band, g.y(), band, g.height()),
            };
            let overlaps = windows.iter().filter(|w| !w.minimized).any(|w| {
                let r = w.geometry;
                r.x < strip.0 + strip.2
                    && strip.0 < r.x + r.width
                    && r.y < strip.1 + strip.3
                    && strip.1 < r.y + r.height
            });
            self.overlapped.set(overlaps);
            self.update_reveal();
        }

        let Some(b) = &self.tasks else { return };
        let active_ws = windows.iter().find(|w| w.focused).map(|w| w.workspace);
        // Groups: pinned apps first, then other running apps in window order.
        let mut groups: Vec<(Option<App>, String, Vec<WindowInfo>)> = Vec::new();
        for id in pinned {
            if let Some(app) = apps.iter().find(|a| &a.id == id) {
                groups.push((Some(app.clone()), id.clone(), Vec::new()));
            }
        }
        for w in windows {
            let key_app = w.app_id.as_deref().and_then(|a| crate::apps::app_for_window(apps, a));
            let key = key_app
                .as_ref()
                .map(|a| a.id.clone())
                .unwrap_or_else(|| w.app_id.clone().unwrap_or_else(|| format!("window-{}", w.id)));
            match groups.iter_mut().find(|g| g.1 == key) {
                Some(g) => g.2.push(w.clone()),
                None => groups.push((key_app, key, vec![w.clone()])),
            }
        }
        let _ = active_ws;
        // Keep buttons whose data didn't change (their indicator animations
        // play instead of the whole strip flickering); rebuild the rest.
        let mut old = std::mem::take(&mut *self.task_cache.borrow_mut());
        let mut new_cache = Vec::with_capacity(groups.len());
        let mut prev: Option<gtk::Widget> = None;
        for (app, key, wins) in groups {
            let sig = format!(
                "{:?}|{}|{:?}",
                app.as_ref().map(|a| (&a.id, &a.name)),
                pinned.contains(&key),
                wins.iter().map(|w| (w.id, w.focused, w.minimized, &w.title)).collect::<Vec<_>>()
            );
            let widget = match old.iter().position(|(k, s, _)| *k == key && *s == sig) {
                Some(i) => old.remove(i).2,
                None => task_button(app, key.clone(), wins, pinned.to_vec(), self.dispatch.clone()),
            };
            if widget.parent().is_none() {
                b.append(&widget);
            }
            b.reorder_child_after(&widget, prev.as_ref());
            prev = Some(widget.clone());
            new_cache.push((key, sig, widget));
        }
        for (_, _, w) in old {
            b.remove(&w);
        }
        // An empty island would show as a stray sliver.
        b.set_visible(!new_cache.is_empty());
        *self.task_cache.borrow_mut() = new_cache;
    }
}

/// Power menu: lock, log out, suspend, restart, shut down.
fn power_button(dispatch: Rc<dyn Fn(Cmd)>) -> gtk::MenuButton {
    use crate::shell::SessionAction as S;
    let button = gtk::MenuButton::new();
    button.add_css_class("m3-power-button");
    button.set_child(Some(&icons::symbol("power_settings_new")));
    button.set_tooltip_text(Some("Power"));
    button.update_property(&[gtk::accessible::Property::Label("Power")]);
    let pop = gtk::Popover::new();
    pop.add_css_class("menu");
    pop.set_position(crate::placement::popover_side());
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for (icon, label, action) in [
        ("lock", "Lock", S::Lock),
        ("bedtime", "Suspend", S::Suspend),
        ("logout", "Log out", S::LogOut),
        ("restart_alt", "Restart", S::Restart),
        ("power_settings_new", "Shut down", S::ShutDown),
    ] {
        let b = gtk::Button::new();
        b.add_css_class("m3-menu-item");
        b.add_css_class("flat");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.append(&icons::symbol_with(icon, &["small"]));
        row.append(&gtk::Label::new(Some(label)));
        b.set_child(Some(&row));
        let (d, p) = (dispatch.clone(), pop.clone());
        b.connect_clicked(move |_| {
            p.popdown();
            d(Cmd::Session(action));
        });
        list.append(&b);
    }
    pop.set_child(Some(&list));
    button.set_popover(Some(&pop));
    button
}

fn launcher_button(dispatch: Rc<dyn Fn(Cmd)>) -> gtk::Button {
    use m3_widgets::Shape;
    // An expressive cookie shape that morphs on hover and press.
    let b = gtk::Button::new();
    b.set_tooltip_text(Some("Applications"));
    b.update_property(&[gtk::accessible::Property::Label("Applications")]);
    b.add_css_class("m3-launcher-button");
    let icon = m3_widgets::ShapeIcon::new(Shape::Cookie(9), "apps", 40, true);
    m3_widgets::morph_on_interaction(&b, icon.shape(), Shape::Cookie(9), Shape::Sunny(8), Shape::Clover(4));
    b.set_child(Some(icon.widget()));
    b.connect_clicked(move |_| dispatch(Cmd::ToggleLauncher));
    b
}

fn format_clock(now: &chrono::DateTime<chrono::Local>, cfg: &PanelsConfig) -> String {
    let time = match (cfg.clock_format, cfg.clock_seconds) {
        (ClockFormat::H24, false) => now.format("%H:%M"),
        (ClockFormat::H24, true) => now.format("%H:%M:%S"),
        (ClockFormat::H12, false) => now.format("%-I:%M %p"),
        (ClockFormat::H12, true) => now.format("%-I:%M:%S %p"),
    }
    .to_string();
    if cfg.clock_date { format!("{}   {time}", now.format("%a %-d %b")) } else { time }
}

fn clock_widget(cfg: &PanelsConfig) -> (gtk::MenuButton, gtk::Label) {
    let label = gtk::Label::new(Some(&format_clock(&chrono::Local::now(), cfg)));
    label.add_css_class("m3-title-small");
    label.add_css_class("m3-clock");
    let button = gtk::MenuButton::new();
    button.set_child(Some(&label));
    button.add_css_class("m3-clock-button");
    button.set_tooltip_text(Some("Calendar"));

    let popover = gtk::Popover::new();
    popover.set_position(crate::placement::popover_side());
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.add_css_class("m3-calendar-sheet");
    let headline = gtk::Label::new(None);
    headline.add_css_class("m3-headline-small");
    headline.set_xalign(0.0);
    let weekday = gtk::Label::new(None);
    weekday.add_css_class("m3-title-medium");
    weekday.add_css_class("m3-secondary-text");
    weekday.set_xalign(0.0);
    let calendar = gtk::Calendar::new();
    content.append(&weekday);
    content.append(&headline);
    content.append(&calendar);
    popover.set_child(Some(&content));
    popover.connect_show(move |_| {
        let now = chrono::Local::now();
        weekday.set_text(&now.format("%A").to_string());
        headline.set_text(&now.format("%-d %B %Y").to_string());
        if let Ok(dt) = glib::DateTime::now_local() {
            calendar.set_date(&dt);
        }
    });
    button.set_popover(Some(&popover));
    (button, label)
}

fn task_button(
    app: Option<App>,
    key: String,
    wins: Vec<WindowInfo>,
    pinned: Vec<String>,
    dispatch: Rc<dyn Fn(Cmd)>,
) -> gtk::Widget {
    let focused = wins.iter().any(|w| w.focused);
    let name = app
        .as_ref()
        .map(|a| a.name.clone())
        .or_else(|| wins.first().and_then(|w| w.title.clone()))
        .unwrap_or_else(|| key.clone());

    let overlay = gtk::Overlay::new();
    let button = gtk::Button::new();
    button.add_css_class("m3-task");
    if focused {
        button.add_css_class("focused");
    }
    if !wins.is_empty() {
        button.add_css_class("running");
    }
    button.set_child(Some(&icons::app_icon(app.as_ref().and_then(|a| a.icon.clone()), 28)));
    let state = if focused {
        ", focused"
    } else if wins.is_empty() {
        ""
    } else {
        ", running"
    };
    let label = format!("{name}{state}");
    button.set_tooltip_text(Some(&name));
    button.update_property(&[gtk::accessible::Property::Label(&label)]);
    overlay.set_child(Some(&button));

    // Running indicator (shape + position, not colour alone).
    if !wins.is_empty() {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("m3-task-indicator");
        if focused {
            dot.add_css_class("focused");
        }
        dot.set_halign(gtk::Align::Center);
        dot.set_valign(gtk::Align::End);
        dot.set_can_target(false);
        overlay.add_overlay(&dot);
    }

    {
        let app = app.clone();
        let wins = wins.clone();
        let dispatch = dispatch.clone();
        button.connect_clicked(move |_| {
            if wins.is_empty() {
                if let Some(a) = &app {
                    a.launch();
                    dispatch(Cmd::Launched(a.id.clone()));
                }
                return;
            }
            match wins.iter().position(|w| w.focused) {
                // Clicking the focused app's only window minimizes it.
                Some(_) if wins.len() == 1 => ipc::action(Action::Minimize),
                // Several windows: cycle through them.
                Some(i) => ipc::action(Action::FocusWindow { id: wins[(i + 1) % wins.len()].id }),
                None => ipc::action(Action::FocusWindow { id: wins[wins.len() - 1].id }),
            }
        });
    }

    // Context menu: windows, new window, pin/unpin, close.
    let menu = gtk::Popover::new();
    menu.set_position(crate::placement::popover_side());
    menu.add_css_class("menu");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let item = |text: &str, icon: &str| {
        let b = gtk::Button::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.append(&icons::symbol_with(icon, &["small"]));
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_max_width_chars(36);
        row.append(&l);
        b.set_child(Some(&row));
        b.add_css_class("m3-menu-item");
        b.add_css_class("flat");
        b
    };
    for w in &wins {
        let b = item(w.title.as_deref().unwrap_or("Window"), if w.minimized { "minimize" } else { "select_window" });
        let id = w.id;
        let m = menu.clone();
        b.connect_clicked(move |_| {
            m.popdown();
            ipc::action(Action::FocusWindow { id });
        });
        list.append(&b);
    }
    if let Some(a) = &app {
        if !wins.is_empty() {
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let b = item("New window", "add");
        let a2 = a.clone();
        let m = menu.clone();
        let d = dispatch.clone();
        b.connect_clicked(move |_| {
            m.popdown();
            a2.launch();
            d(Cmd::Launched(a2.id.clone()));
        });
        list.append(&b);
        let is_pinned = pinned.contains(&a.id);
        let b = item(
            if is_pinned { "Unpin from panel" } else { "Pin to panel" },
            if is_pinned { "keep_off" } else { "keep" },
        );
        let id = a.id.clone();
        let m = menu.clone();
        b.connect_clicked(move |_| {
            m.popdown();
            set_pinned(&id, !is_pinned);
        });
        list.append(&b);
    }
    if !wins.is_empty() {
        let b = item(if wins.len() > 1 { "Close all windows" } else { "Close window" }, "close");
        let ids: Vec<u64> = wins.iter().map(|w| w.id).collect();
        let m = menu.clone();
        b.connect_clicked(move |_| {
            m.popdown();
            for id in &ids {
                ipc::action(Action::CloseWindowById { id: *id });
            }
        });
        list.append(&b);
    }
    menu.set_child(Some(&list));
    menu.set_parent(&overlay);
    let click = gtk::GestureClick::builder().button(3).build();
    click.connect_pressed(move |_, _, _, _| menu.popup());
    overlay.add_controller(click);
    overlay.upcast()
}

/// Pin or unpin an app in panels.toml (the shell reloads on file change).
pub fn set_pinned(id: &str, pin: bool) {
    let Ok(dir) = cfg::default_dir() else { return };
    let mut c = match cfg::load::<PanelsConfig>(&dir) {
        Ok(l) => l.value,
        Err(e) => {
            tracing::warn!("not changing pins: {e}");
            return;
        }
    };
    c.pinned.retain(|p| p != id);
    if pin {
        c.pinned.push(id.to_owned());
    }
    if let Err(e) = cfg::save(&dir, &c) {
        tracing::warn!("cannot save panels.toml: {e}");
    }
}
