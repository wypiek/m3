//! Desktop widgets: a big expressive clock and greeting on one side, a
//! column of shaped widgets (day, shortcuts, now playing, countdown,
//! storage, RAM/CPU, weather) on the other. Drawn on the bottom layer, above
//! the wallpaper and below windows. Configured in `widgets.toml`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use chrono::{Datelike, Local, Timelike};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_core::config::{TemperatureUnit, WidgetSide, WidgetsConfig};
use m3_widgets::{Shape, ShapeIcon};

use crate::icons;
use crate::shell::Cmd;

/// "Morning" / "Afternoon" / "Evening" / "Night" (pure; tested).
pub fn part_of_day(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Morning",
        12..=16 => "Afternoon",
        17..=21 => "Evening",
        _ => "Night",
    }
}

/// Days from `today` until `target` (negative once past; pure; tested).
pub fn days_until(today: chrono::NaiveDate, target: chrono::NaiveDate) -> i64 {
    (target - today).num_days()
}

/// Used memory in percent from `/proc/meminfo` text (pure; tested).
pub fn ram_percent(meminfo: &str) -> Option<f64> {
    let field =
        |name: &str| meminfo.lines().find_map(|l| l.strip_prefix(name)?.split_whitespace().next()?.parse::<f64>().ok());
    let (total, avail) = (field("MemTotal:")?, field("MemAvailable:")?);
    (total > 0.0).then(|| (1.0 - avail / total) * 100.0)
}

/// `(busy, total)` jiffies from the first line of `/proc/stat` (pure; tested).
pub fn cpu_times(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() < 5 {
        return None;
    }
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v[4];
    Some((total.saturating_sub(idle), total))
}

/// "476GB", "1.9TB" — decimal units like disk labels (pure; tested).
pub fn disk_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 { format!("{:.1}TB", gb / 1000.0) } else { format!("{gb:.0}GB") }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// The pieces that update over time.
#[derive(Default)]
struct Live {
    clock: Vec<gtk::Label>,
    day_num: Option<gtk::Label>,
    month_num: Option<gtk::Label>,
    part: Option<gtk::Label>,
    day_badge: Option<gtk::Label>,
    countdown: Option<(gtk::Box, gtk::Label, gtk::Label)>,
    storage: Option<(gtk::Label, gtk::Label)>,
    ram: Option<gtk::Label>,
    cpu: Option<gtk::Label>,
    media: Option<MediaParts>,
    weather: Option<WeatherParts>,
}

struct MediaParts {
    card: gtk::Box,
    art: gtk::Picture,
    art_fallback: ShapeIcon,
    play: gtk::Button,
    title: gtk::Label,
    player: Rc<RefCell<Option<String>>>,
}

struct WeatherParts {
    card: gtk::Box,
    icon: gtk::Label,
    temp: gtk::Label,
    cond: gtk::Label,
    days: gtk::Box,
}

pub struct DesktopWidgets {
    pub window: gtk::Window,
    cfg: RefCell<WidgetsConfig>,
    live: RefCell<Live>,
    dispatch: Rc<dyn Fn(Cmd)>,
    cpu_prev: Cell<Option<(u64, u64)>>,
    bus: RefCell<Option<gio::DBusConnection>>,
}

impl DesktopWidgets {
    pub fn new(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        cfg: &WidgetsConfig,
        dispatch: Rc<dyn Fn(Cmd)>,
    ) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Bottom);
        window.set_namespace(Some("m3-widgets"));
        window.set_monitor(Some(monitor));
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::None);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-transparent");
        window.add_css_class("m3-desktop");

        let me = Rc::new(Self {
            window,
            cfg: RefCell::new(cfg.clone()),
            live: RefCell::new(Live::default()),
            dispatch,
            cpu_prev: Cell::new(None),
            bus: RefCell::new(None),
        });
        me.rebuild();
        me.start_timers();
        me.window.set_visible(cfg.enabled);
        me
    }

    pub fn apply_config(self: &Rc<Self>, cfg: &WidgetsConfig) {
        if *self.cfg.borrow() == *cfg {
            return;
        }
        *self.cfg.borrow_mut() = cfg.clone();
        self.rebuild();
        self.window.set_visible(cfg.enabled);
        self.refresh_slow();
        self.refresh_weather();
    }

    fn rebuild(self: &Rc<Self>) {
        let cfg = self.cfg.borrow().clone();
        let mut live = Live::default();
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("m3-desktop-root");

        let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
        left.set_valign(gtk::Align::Center);
        left.add_css_class("m3-desktop-hero");
        if cfg.clock {
            left.append(&self.clock(&mut live));
        }
        if cfg.greeting {
            left.append(&self.greeting(&cfg, &mut live));
        }

        let grid = gtk::Grid::new();
        grid.set_row_spacing(18);
        grid.set_column_spacing(18);
        grid.set_valign(gtk::Align::Center);
        grid.add_css_class("m3-desktop-column");
        let (mut c0, mut c1) = (Vec::<gtk::Widget>::new(), Vec::<gtk::Widget>::new());
        if cfg.day {
            c0.push(self.day_badge(&mut live).upcast());
        }
        if cfg.shortcuts {
            c1.push(self.shortcuts(&cfg).upcast());
        }
        if cfg.media {
            c0.push(self.media(&mut live).upcast());
        }
        if !cfg.countdown.title.trim().is_empty() {
            c1.push(self.countdown(&cfg, &mut live).upcast());
        }
        if cfg.storage || cfg.system {
            c0.push(self.system(&cfg, &mut live).upcast());
        }
        if cfg.weather.enabled {
            c1.push(self.weather(&mut live).upcast());
        }
        for (col, items) in [(0, c0), (1, c1)] {
            for (row, w) in items.into_iter().enumerate() {
                w.set_valign(gtk::Align::Center);
                w.set_halign(gtk::Align::Center);
                grid.attach(&w, col, row as i32, 1, 1);
            }
        }

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        spacer.set_can_target(false);
        let (a, b): (gtk::Widget, gtk::Widget) = match cfg.side {
            WidgetSide::Right => (left.upcast(), grid.upcast()),
            WidgetSide::Left => (grid.upcast(), left.upcast()),
        };
        a.add_css_class("m3-desktop-start");
        b.add_css_class("m3-desktop-end");
        root.append(&a);
        root.append(&spacer);
        root.append(&b);
        self.window.set_child(Some(&root));
        *self.live.borrow_mut() = live;
        self.tick();
        m3_widgets::animate(&root, "m3-fade");
    }

    // ---- hero: clock + greeting ---------------------------------------------

    fn clock(&self, live: &mut Live) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        b.add_css_class("m3-desk-clock");
        b.set_halign(gtk::Align::Start);
        for i in 0..4 {
            if i == 2 {
                let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                gap.set_size_request(28, -1);
                b.append(&gap);
            }
            // Two-tone: the last digit of each pair in the accent colour.
            let d = label("0", &["m3-desk-digit", if i % 2 == 1 { "accent" } else { "ink" }]);
            b.append(&d);
            live.clock.push(d);
        }
        b.update_property(&[gtk::accessible::Property::Label("Time")]);
        b
    }

    fn greeting(&self, cfg: &WidgetsConfig, live: &mut Live) -> gtk::Box {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 14);
        col.set_halign(gtk::Align::Start);
        // "G (day) (month) D" — the O's of "GOOD" are the date.
        let word = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        word.add_css_class("m3-desk-good");
        word.append(&label("G", &["m3-desk-letter"]));
        let day = ShapeIcon::new(Shape::Cookie(12), "1", 92, false);
        day.widget().add_css_class("m3-desk-date-cookie");
        day.label().add_css_class("m3-desk-date-num");
        let month = ShapeIcon::new(Shape::Cookie(12), "1", 92, false);
        month.widget().add_css_class("m3-desk-date-cookie");
        month.label().add_css_class("m3-desk-date-num");
        word.append(day.widget());
        word.append(month.widget());
        word.append(&label("D", &["m3-desk-letter"]));
        col.append(&word);
        let pills = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pills.add_css_class("m3-desk-pills");
        let part = label("", &["m3-desk-pill", "rose"]);
        let name = if cfg.name.trim().is_empty() { default_name() } else { cfg.name.trim().to_owned() };
        pills.append(&part);
        if !name.is_empty() {
            pills.append(&label(&name, &["m3-desk-pill", "gold"]));
        }
        col.append(&pills);
        live.day_num = Some(day.label().clone());
        live.month_num = Some(month.label().clone());
        live.part = Some(part);
        col
    }

    // ---- column widgets ------------------------------------------------------

    fn day_badge(&self, live: &mut Live) -> gtk::Overlay {
        let o = ShapeIcon::new(Shape::Flower(8), "", 176, false);
        o.widget().add_css_class("m3-desk-day");
        o.label().add_css_class("m3-desk-day-num");
        live.day_badge = Some(o.label().clone());
        o.widget().clone()
    }

    fn shortcuts(&self, cfg: &WidgetsConfig) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("m3-desk-card");
        card.add_css_class("m3-desk-shortcuts");
        let apps: Vec<gio_unix::DesktopAppInfo> =
            cfg.shortcut_apps.iter().filter_map(|id| gio_unix::DesktopAppInfo::new(id)).take(3).collect();
        let circle = |app: &gio_unix::DesktopAppInfo| {
            let b = gtk::Button::new();
            b.add_css_class("m3-desk-circle");
            b.add_css_class("rose");
            b.set_child(Some(&icons::app_icon(gio::prelude::AppInfoExt::icon(app), 26)));
            let name = gio::prelude::AppInfoExt::display_name(app);
            b.set_tooltip_text(Some(&name));
            b.update_property(&[gtk::accessible::Property::Label(&name)]);
            let app = app.clone();
            b.connect_clicked(move |_| {
                let ctx = gdk::Display::default().map(|d| d.app_launch_context());
                if let Err(e) = gio::prelude::AppInfoExt::launch(&app, &[], ctx.as_ref()) {
                    tracing::warn!("cannot start {}: {e}", gio::prelude::AppInfoExt::name(&app));
                }
            });
            b
        };
        if let Some(first) = apps.first() {
            let top = circle(first);
            top.set_halign(gtk::Align::Center);
            card.append(&top);
        }
        if apps.len() > 1 {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_halign(gtk::Align::Center);
            for a in &apps[1..] {
                row.append(&circle(a));
            }
            card.append(&row);
        }
        let search = gtk::Button::new();
        search.add_css_class("m3-desk-search");
        let sb = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        sb.append(&icons::symbol_with("search", &["small"]));
        sb.append(&label("Search", &["m3-label-large-emphasized"]));
        search.set_child(Some(&sb));
        let d = self.dispatch.clone();
        search.connect_clicked(move |_| d(Cmd::ToggleLauncher));
        card.append(&search);
        card
    }

    fn media(self: &Rc<Self>, live: &mut Live) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("m3-desk-card");
        card.add_css_class("m3-desk-media");
        let grid = gtk::Grid::new();
        grid.set_column_spacing(4);
        grid.set_row_spacing(4);
        let player: Rc<RefCell<Option<String>>> = Rc::default();
        let control = |icon: &str, tip: &str, method: &'static str, big: bool| {
            let b = icons::icon_button(icon, tip);
            b.add_css_class("m3-desk-circle");
            b.add_css_class(if big { "rose" } else { "ink" });
            if big {
                b.add_css_class("big");
            }
            let (weak, player) = (Rc::downgrade(self), player.clone());
            b.connect_clicked(move |_| {
                let (Some(me), Some(p)) = (weak.upgrade(), player.borrow().clone()) else { return };
                let conn = me.bus.borrow().clone();
                let weak = weak.clone();
                glib::spawn_future_local(async move {
                    if let Some(conn) = conn {
                        m3_widgets::mpris::control(&conn, &p, method).await;
                    }
                    if let Some(me) = weak.upgrade() {
                        me.refresh_media();
                    }
                });
            });
            b
        };
        let prev = control("skip_previous", "Previous", "Previous", false);
        let play = control("play_arrow", "Play", "PlayPause", true);
        let next = control("skip_next", "Next", "Next", false);
        let art_box = gtk::Overlay::new();
        art_box.add_css_class("m3-desk-art");
        let art = gtk::Picture::new();
        art.set_content_fit(gtk::ContentFit::Cover);
        art.set_size_request(112, 112);
        art.set_overflow(gtk::Overflow::Hidden);
        art.add_css_class("m3-desk-art-picture");
        let fallback = ShapeIcon::new(Shape::Cookie(9), "music_note", 112, true);
        fallback.widget().add_css_class("m3-desk-art-fallback");
        art_box.set_child(Some(fallback.widget()));
        art_box.add_overlay(&art);
        grid.attach(&prev, 0, 0, 1, 1);
        grid.attach(&play, 1, 0, 1, 1);
        grid.attach(&art_box, 0, 1, 1, 1);
        grid.attach(&next, 1, 1, 1, 1);
        prev.set_halign(gtk::Align::End);
        next.set_valign(gtk::Align::Center);
        card.append(&grid);
        let title = label("Nothing playing", &["m3-label-medium", "m3-desk-media-title"]);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(16);
        card.append(&title);
        live.media = Some(MediaParts { card: card.clone(), art, art_fallback: fallback, play, title, player });
        card
    }

    fn countdown(&self, cfg: &WidgetsConfig, live: &mut Live) -> gtk::Box {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
        col.add_css_class("m3-desk-countdown");
        let dome = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dome.add_css_class("m3-desk-card");
        dome.add_css_class("m3-desk-dome");
        let title = label(&cfg.countdown.title, &["m3-title-medium-emphasized"]);
        title.set_vexpand(true);
        title.set_valign(gtk::Align::Center);
        title.set_wrap(true);
        title.set_justify(gtk::Justification::Center);
        title.set_max_width_chars(14);
        dome.append(&title);
        col.append(&dome);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::Center);
        let n = ShapeIcon::new(Shape::Sunny(8), "0", 64, false);
        n.widget().add_css_class("m3-desk-sunny");
        n.label().add_css_class("m3-desk-sunny-num");
        let caption = label("Days to go", &["m3-desk-pill", "rose", "small"]);
        row.append(n.widget());
        row.append(&caption);
        col.append(&row);
        live.countdown = Some((col.clone(), n.label().clone(), caption));
        col
    }

    fn system(&self, cfg: &WidgetsConfig, live: &mut Live) -> gtk::Box {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
        if cfg.storage {
            let pill = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            pill.add_css_class("m3-desk-card");
            pill.add_css_class("m3-desk-storage");
            let total = label("—", &["m3-desk-stat"]);
            let free = label("—", &["m3-desk-stat"]);
            let side = |t: &str, v: &gtk::Label| {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.append(&label(t, &["m3-label-small"]));
                b.append(v);
                b
            };
            pill.append(&side("Total", &total));
            let icon = ShapeIcon::new(Shape::Cookie(9), "hard_drive", 52, true);
            icon.widget().add_css_class("m3-desk-storage-icon");
            pill.append(icon.widget());
            pill.append(&side("Free", &free));
            col.append(&pill);
            live.storage = Some((total, free));
        }
        if cfg.system {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_halign(gtk::Align::Center);
            let bubble = |name: &str, tone: &str| {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.add_css_class("m3-desk-bubble");
                b.add_css_class(tone);
                b.set_valign(gtk::Align::Center);
                let v = label("—", &["m3-desk-stat"]);
                b.append(&v);
                b.append(&label(name, &["m3-label-medium"]));
                (b, v)
            };
            let (rb, ram) = bubble("RAM", "rose");
            let (cb, cpu) = bubble("CPU", "gold");
            row.append(&rb);
            row.append(&cb);
            col.append(&row);
            live.ram = Some(ram);
            live.cpu = Some(cpu);
        }
        col
    }

    fn weather(&self, live: &mut Live) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
        card.add_css_class("m3-desk-card");
        card.add_css_class("m3-desk-weather");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = label("cloud", &["m3-icon", "m3-desk-weather-icon"]);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        let temp = label("—", &["m3-title-large-emphasized"]);
        temp.set_xalign(1.0);
        let cond = label("Loading…", &["m3-label-medium"]);
        cond.set_xalign(1.0);
        text.append(&temp);
        text.append(&cond);
        top.append(&icon);
        top.append(&text);
        card.append(&top);
        let days = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        days.set_homogeneous(true);
        card.append(&days);
        live.weather = Some(WeatherParts { card: card.clone(), icon, temp, cond, days });
        card
    }

    // ---- updates ---------------------------------------------------------------

    fn start_timers(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(5, move || match weak.upgrade() {
            Some(me) => {
                me.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(3, move || match weak.upgrade() {
            Some(me) => {
                me.refresh_system();
                me.refresh_media();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(120, move || match weak.upgrade() {
            Some(me) => {
                me.refresh_slow();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(30 * 60, move || match weak.upgrade() {
            Some(me) => {
                me.refresh_weather();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let bus = gio::bus_get_future(gio::BusType::Session).await.ok();
            if let Some(me) = weak.upgrade() {
                *me.bus.borrow_mut() = bus;
                me.refresh_media();
            }
        });
        self.refresh_system();
        self.refresh_slow();
        self.refresh_weather();
    }

    /// Clock, date, part of day, countdown.
    fn tick(&self) {
        let now = Local::now();
        let live = self.live.borrow();
        let digits: Vec<char> = now.format("%H%M").to_string().chars().collect();
        for (l, d) in live.clock.iter().zip(digits) {
            let s = d.to_string();
            if l.text() != s {
                l.set_text(&s);
            }
        }
        if let Some(l) = &live.day_num {
            l.set_text(&format!("{:02}", now.day()));
        }
        if let Some(l) = &live.month_num {
            l.set_text(&format!("{:02}", now.month()));
        }
        if let Some(l) = &live.part {
            l.set_text(part_of_day(now.hour()));
        }
        if let Some(l) = &live.day_badge {
            l.set_text(&now.day().to_string());
        }
        if let Some((card, n, caption)) = &live.countdown {
            let cfg = self.cfg.borrow();
            let target = m3_core::config::parse_date(&cfg.countdown.date)
                .ok()
                .and_then(|(y, m, d)| chrono::NaiveDate::from_ymd_opt(y, m, d));
            match target.map(|t| days_until(now.date_naive(), t)) {
                Some(d) if d > 0 => {
                    card.set_visible(true);
                    n.set_text(&d.to_string());
                    caption.set_text(if d == 1 { "Day to go" } else { "Days to go" });
                }
                Some(0) => {
                    card.set_visible(true);
                    n.set_text("0");
                    caption.set_text("It's today!");
                }
                _ => card.set_visible(false),
            }
        }
    }

    fn refresh_system(&self) {
        let live = self.live.borrow();
        if let Some(l) = &live.ram {
            if let Some(p) = std::fs::read_to_string("/proc/meminfo").ok().as_deref().and_then(ram_percent) {
                l.set_text(&format!("{p:.0}%"));
            }
        }
        if let Some(l) = &live.cpu {
            if let Some(now) = std::fs::read_to_string("/proc/stat").ok().as_deref().and_then(cpu_times) {
                if let Some((busy0, total0)) = self.cpu_prev.replace(Some(now)) {
                    let (db, dt) = (now.0.saturating_sub(busy0), now.1.saturating_sub(total0));
                    if dt > 0 {
                        l.set_text(&format!("{:.0}%", db as f64 * 100.0 / dt as f64));
                    }
                }
            }
        }
    }

    /// Storage (changes slowly).
    fn refresh_slow(&self) {
        let live = self.live.borrow();
        let Some((total, free)) = &live.storage else { return };
        let root = gio::File::for_path("/");
        match root.query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE) {
            Ok(info) => {
                total.set_text(&disk_size(info.attribute_uint64("filesystem::size")));
                free.set_text(&disk_size(info.attribute_uint64("filesystem::free")));
            }
            Err(e) => tracing::debug!("filesystem info unavailable: {e}"),
        }
    }

    fn refresh_media(self: &Rc<Self>) {
        let Some(conn) = self.bus.borrow().clone() else { return };
        if self.live.borrow().media.is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let now = m3_widgets::mpris::query(&conn).await;
            let Some(me) = weak.upgrade() else { return };
            let live = me.live.borrow();
            let Some(m) = &live.media else { return };
            *m.player.borrow_mut() = now.as_ref().map(|n| n.bus_name.clone());
            m.card.set_sensitive(true);
            match now {
                Some(n) => {
                    let text =
                        if n.artist.is_empty() { n.title.clone() } else { format!("{} · {}", n.title, n.artist) };
                    m.title.set_text(&text);
                    m.card.set_tooltip_text(Some(&text));
                    if let Some(l) = m.play.child().and_downcast::<gtk::Label>() {
                        l.set_text(if n.playing { "pause" } else { "play_arrow" });
                    }
                    m.play.update_property(&[gtk::accessible::Property::Label(if n.playing {
                        "Pause"
                    } else {
                        "Play"
                    })]);
                    match &n.art {
                        Some(p) => {
                            if m.art.file().and_then(|f| f.path()).as_ref() != Some(p) {
                                m.art.set_filename(Some(p));
                            }
                            m.art.set_visible(true);
                        }
                        None => m.art.set_visible(false),
                    }
                    m.art_fallback.shape().set_spin(if n.playing && n.art.is_none() { 30.0 } else { 0.0 });
                }
                None => {
                    m.title.set_text("Nothing playing");
                    m.card.set_tooltip_text(None);
                    m.art.set_visible(false);
                    m.art_fallback.shape().set_spin(0.0);
                    if let Some(l) = m.play.child().and_downcast::<gtk::Label>() {
                        l.set_text("play_arrow");
                    }
                }
            }
        });
    }

    fn refresh_weather(self: &Rc<Self>) {
        let cfg = self.cfg.borrow().weather.clone();
        if !cfg.enabled || self.live.borrow().weather.is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        let fahrenheit = cfg.unit == TemperatureUnit::Fahrenheit;
        glib::spawn_future_local(async move {
            let r =
                gio::spawn_blocking(move || m3_services::weather::forecast(cfg.latitude, cfg.longitude, fahrenheit))
                    .await;
            let Some(me) = weak.upgrade() else { return };
            let live = me.live.borrow();
            let Some(w) = &live.weather else { return };
            match r {
                Ok(Ok(f)) => {
                    let (desc, icon) = m3_services::weather::describe(f.code, f.is_day);
                    w.icon.set_text(icon);
                    w.temp.set_text(&format!("{:.0}°", f.temperature));
                    w.cond.set_text(desc);
                    while let Some(c) = w.days.first_child() {
                        w.days.remove(&c);
                    }
                    for (i, d) in f.days.iter().skip(1).take(3).enumerate() {
                        let day = chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                            .map(|x| x.format("%a").to_string())
                            .unwrap_or_default();
                        let pill = gtk::Box::new(gtk::Orientation::Vertical, 4);
                        pill.add_css_class("m3-desk-day-pill");
                        pill.add_css_class(if i == 1 { "gold" } else { "rose" });
                        pill.append(&label(&day, &["m3-label-small"]));
                        pill.append(&label(m3_services::weather::describe(d.code, true).1, &["m3-icon", "small"]));
                        pill.append(&label(&format!("{:.0}°", d.max), &["m3-label-small"]));
                        pill.set_tooltip_text(Some(&format!(
                            "{} — {:.0}° / {:.0}°",
                            m3_services::weather::describe(d.code, true).0,
                            d.max,
                            d.min
                        )));
                        w.days.append(&pill);
                    }
                    w.card.set_tooltip_text(Some(&me.cfg.borrow().weather.place));
                }
                Ok(Err(e)) => {
                    tracing::info!("{e}");
                    w.cond.set_text("Offline");
                }
                Err(_) => w.cond.set_text("Offline"),
            }
        });
    }
}

/// First name from the account's real name, else the login name.
fn default_name() -> String {
    let real = glib::real_name().to_string_lossy().into_owned();
    let first = real.split_whitespace().next().unwrap_or_default();
    if first.is_empty() || first == "Unknown" {
        glib::user_name().to_string_lossy().into_owned()
    } else {
        first.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_of_day() {
        assert_eq!(part_of_day(6), "Morning");
        assert_eq!(part_of_day(13), "Afternoon");
        assert_eq!(part_of_day(19), "Evening");
        assert_eq!(part_of_day(2), "Night");
    }

    #[test]
    fn countdown_days() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).expect("date");
        assert_eq!(days_until(d(2026, 11, 11), d(2026, 11, 18)), 7);
        assert_eq!(days_until(d(2026, 12, 31), d(2027, 1, 1)), 1);
        assert_eq!(days_until(d(2026, 11, 11), d(2026, 11, 10)), -1);
    }

    #[test]
    fn system_stats() {
        let mem = "MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    9120000 kB\n";
        assert_eq!(ram_percent(mem).map(|p| p.round()), Some(43.0));
        assert_eq!(cpu_times("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2"), Some((150, 1000)));
        assert_eq!(cpu_times("garbage"), None);
        assert_eq!(disk_size(476_000_000_000), "476GB");
        assert_eq!(disk_size(1_900_000_000_000), "1.9TB");
    }
}
