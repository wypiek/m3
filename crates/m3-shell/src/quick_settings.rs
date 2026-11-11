//! Quick settings: toggles, sliders and session controls backed by real
//! system services. Controls only appear when the service behind them is
//! available (no inert tiles).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::{gio, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use m3_core::config::{self as cfg, ThemeConfig, ThemeMode};
use m3_protocol::Action;
use m3_services::{
    Command, Services,
    audio::{Audio, AudioCmd},
};

use crate::{
    icons, ipc,
    status::{self, SysState},
};

/// A large expressive toggle tile: icon, title, subtitle, optional chevron.
struct Tile {
    root: gtk::Box,
    button: gtk::ToggleButton,
    subtitle: gtk::Label,
    icon: m3_widgets::ShapeIcon,
    more: Option<gtk::Button>,
}

impl Tile {
    fn new(icon: &str, title: &str, expandable: bool) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("m3-tile");
        root.set_hexpand(true);
        let button = gtk::ToggleButton::new();
        button.add_css_class("m3-tile-main");
        button.set_hexpand(true);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        // The icon sits in a container that is round when off and a soft
        // square when on (M3 Expressive: selected toggles become square).
        let ic = m3_widgets::ShapeIcon::new(m3_widgets::Shape::Circle, icon, 40, true);
        ic.label().remove_css_class("filled");
        ic.widget().add_css_class("m3-tile-icon");
        row.append(ic.widget());
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col.set_valign(gtk::Align::Center);
        let t = gtk::Label::new(Some(title));
        t.set_xalign(0.0);
        t.add_css_class("m3-label-large");
        let subtitle = gtk::Label::new(None);
        subtitle.set_xalign(0.0);
        subtitle.add_css_class("m3-body-small");
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.set_max_width_chars(18);
        col.append(&t);
        col.append(&subtitle);
        row.append(&col);
        button.set_child(Some(&row));
        root.append(&button);
        let more = expandable.then(|| {
            let m = icons::icon_button("chevron_right", &format!("{title} settings"));
            m.add_css_class("m3-tile-more");
            root.append(&m);
            m
        });
        Self { root, button, subtitle, icon: ic, more }
    }

    /// Show the tile as on/off right away (optimistic feedback while the
    /// service applies the change; `set` reconciles with the real state).
    fn show_active(&self, active: bool) {
        let was = self.root.has_css_class("active");
        if active {
            self.root.add_css_class("active");
            self.icon.label().add_css_class("filled");
        } else {
            self.root.remove_css_class("active");
            self.icon.label().remove_css_class("filled");
        }
        if was != active {
            let shape = if active { m3_widgets::Shape::Square } else { m3_widgets::Shape::Circle };
            self.icon.shape().morph_to(shape, 0.0, m3_widgets::SpringSpec::FAST);
        }
    }

    fn set(&self, active: bool, subtitle: &str) {
        // Avoid re-triggering the toggled handler.
        if self.button.is_active() != active {
            self.button.set_active(active);
        }
        self.show_active(active);
        self.subtitle.set_text(subtitle);
    }
}

pub struct QuickSettings {
    window: gtk::Window,
    revealer: gtk::Revealer,
    services: Services,
    audio: Audio,
    state: RefCell<SysState>,
    updating: Cell<bool>,

    battery: gtk::Label,
    grid: gtk::Grid,
    /// Radios that were on before airplane mode, restored when it's turned off.
    airplane_restore: Cell<(bool, bool)>,
    /// `theme.toml` mode, cached by file modification time.
    dark_cache: RefCell<Option<(std::time::SystemTime, ThemeMode)>>,
    night_cache: RefCell<Option<(std::time::SystemTime, cfg::NightLight)>>,
    /// Data the detail list was last built from (rebuilt only on change).
    detail_sig: RefCell<String>,
    /// Sink names in the output chooser (model replaced only on change).
    sink_names: RefCell<Vec<String>>,
    clock_12h: Rc<Cell<bool>>,
    clock_tick: Rc<dyn Fn()>,
    wifi: Tile,
    bluetooth: Tile,
    dnd: Tile,
    dark: Tile,
    profile: Tile,
    airplane: Tile,
    night: Tile,
    ethernet: gtk::Box,
    ethernet_label: gtk::Label,
    detail: gtk::Revealer,
    detail_title: gtk::Label,
    detail_list: gtk::Box,
    detail_kind: RefCell<Option<&'static str>>,
    volume_row: gtk::Box,
    volume_icon: gtk::Button,
    volume: gtk::Scale,
    output_chooser: gtk::DropDown,
    mic_row: gtk::Box,
    mic_icon: gtk::Button,
    mic: gtk::Scale,
    brightness_row: gtk::Box,
    brightness: gtk::Scale,
}

fn slider_row(icon: &str, label: &str) -> (gtk::Box, gtk::Button, gtk::Scale) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("m3-slider-row");
    let b = icons::icon_button(icon, label);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    scale.set_hexpand(true);
    scale.set_draw_value(false);
    scale.add_css_class("m3-slider-large");
    scale.update_property(&[gtk::accessible::Property::Label(label)]);
    row.append(&b);
    row.append(&scale);
    (row, b, scale)
}

impl QuickSettings {
    pub fn new(app: &gtk::Application, services: Services, audio: Audio) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_namespace(Some("m3-quick-settings"));
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-qs-window");

        let sheet = gtk::Box::new(gtk::Orientation::Vertical, 12);
        sheet.add_css_class("m3-quick-settings");
        sheet.add_css_class("m3-panel-surface");
        sheet.set_size_request(400, -1);

        // Header: a large clock and date with battery, and a connected group
        // of tonal session buttons.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("m3-qs-header");
        let when = gtk::Box::new(gtk::Orientation::Vertical, 0);
        when.set_hexpand(true);
        let clock = gtk::Label::new(None);
        clock.add_css_class("m3-display-small-emphasized");
        clock.add_css_class("m3-qs-clock");
        clock.set_xalign(0.0);
        let date = gtk::Label::new(None);
        date.add_css_class("m3-title-small");
        date.add_css_class("m3-secondary-text");
        date.set_xalign(0.0);
        let battery = gtk::Label::new(None);
        battery.add_css_class("m3-label-large");
        battery.add_css_class("m3-qs-battery");
        battery.set_xalign(0.0);
        when.append(&clock);
        when.append(&date);
        when.append(&battery);
        header.append(&when);
        let clock_12h = Rc::new(Cell::new(false));
        let tick: Rc<dyn Fn()> = {
            let (clock, date, h12) = (clock.clone(), date.clone(), clock_12h.clone());
            Rc::new(move || {
                let now = chrono::Local::now();
                let fmt = if h12.get() { "%-I:%M" } else { "%H:%M" };
                clock.set_text(&now.format(fmt).to_string());
                date.set_text(&now.format("%A, %-d %B").to_string());
            })
        };
        tick();
        {
            let tick = tick.clone();
            glib::timeout_add_seconds_local(5, move || {
                tick();
                glib::ControlFlow::Continue
            });
        }
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        actions.add_css_class("m3-button-group");
        actions.add_css_class("m3-qs-actions");
        actions.set_valign(gtk::Align::Start);
        let settings_btn = icons::icon_button("settings", "Settings");
        settings_btn.connect_clicked(|_| launch_settings());
        let lock_btn = icons::icon_button("lock", "Lock screen");
        lock_btn.connect_clicked(|_| ipc::action(Action::Lock));
        let power_btn = gtk::MenuButton::new();
        power_btn.set_child(Some(&icons::symbol_with("power_settings_new", &["filled"])));
        power_btn.add_css_class("m3-qs-power");
        power_btn.set_valign(gtk::Align::Start);
        power_btn.set_tooltip_text(Some("Power"));
        actions.append(&settings_btn);
        actions.append(&lock_btn);
        header.append(&actions);
        header.append(&power_btn);
        sheet.append(&header);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        grid.set_column_homogeneous(true);
        let wifi = Tile::new("wifi", "Wi-Fi", true);
        let bluetooth = Tile::new("bluetooth", "Bluetooth", true);
        let dnd = Tile::new("do_not_disturb_on", "Do Not Disturb", false);
        let dark = Tile::new("dark_mode", "Dark theme", false);
        let profile = Tile::new("balance", "Power mode", false);
        let airplane = Tile::new("airplanemode_active", "Airplane mode", false);
        grid.attach(&wifi.root, 0, 0, 1, 1);
        grid.attach(&bluetooth.root, 1, 0, 1, 1);
        grid.attach(&dnd.root, 0, 1, 1, 1);
        grid.attach(&dark.root, 1, 1, 1, 1);
        grid.attach(&profile.root, 0, 2, 1, 1);
        grid.attach(&airplane.root, 1, 2, 1, 1);
        let night = Tile::new("nightlight", "Night light", false);
        grid.attach(&night.root, 0, 3, 1, 1);
        sheet.append(&grid);

        let ethernet = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        ethernet.add_css_class("m3-status-row");
        ethernet.append(&icons::symbol_with("lan", &["small"]));
        let ethernet_label = gtk::Label::new(None);
        ethernet_label.add_css_class("m3-body-medium");
        ethernet.append(&ethernet_label);
        sheet.append(&ethernet);

        // Detail panel (Wi-Fi networks / Bluetooth devices).
        let detail = gtk::Revealer::new();
        detail.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        let dbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
        dbox.add_css_class("m3-qs-detail");
        let dhead = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let detail_title = gtk::Label::new(None);
        detail_title.add_css_class("m3-title-medium");
        detail_title.set_hexpand(true);
        detail_title.set_xalign(0.0);
        dhead.append(&detail_title);
        let refresh = icons::icon_button("refresh", "Scan");
        dhead.append(&refresh);
        let close = icons::icon_button("close", "Close");
        dhead.append(&close);
        dbox.append(&dhead);
        let detail_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        detail_list.add_css_class("m3-group");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_max_content_height(260);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&detail_list));
        dbox.append(&scroller);
        detail.set_child(Some(&dbox));
        sheet.append(&detail);

        let (volume_row, volume_icon, volume) = slider_row("volume_up", "Volume");
        let output_chooser = gtk::DropDown::from_strings(&[]);
        output_chooser.set_tooltip_text(Some("Output device"));
        volume_row.append(&output_chooser);
        let (mic_row, mic_icon, mic) = slider_row("mic", "Microphone");
        let (brightness_row, _b_icon, brightness) = slider_row("brightness_6", "Brightness");
        sheet.append(&volume_row);
        sheet.append(&mic_row);
        sheet.append(&brightness_row);

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_transition_duration(250);
        revealer.set_child(Some(&sheet));
        // Layer surfaces not anchored on opposite edges must never be 0 px
        // wide/tall (protocol error), even while a revealer is collapsed.
        revealer.set_size_request(400, 1);
        window.set_child(Some(&revealer));
        let pos = crate::placement::panel_position();
        crate::placement::anchor_sheet(&window, &revealer, pos);

        let qs = Rc::new(Self {
            window,
            revealer,
            services,
            audio,
            state: RefCell::new(SysState::default()),
            updating: Cell::new(false),
            battery,
            grid: grid.clone(),
            airplane_restore: Cell::new((true, true)),
            dark_cache: RefCell::new(None),
            night_cache: RefCell::new(None),
            detail_sig: RefCell::new(String::new()),
            sink_names: RefCell::new(Vec::new()),
            clock_12h,
            clock_tick: tick,
            wifi,
            bluetooth,
            dnd,
            dark,
            profile,
            airplane,
            night,
            ethernet,
            ethernet_label,
            detail,
            detail_title,
            detail_list,
            detail_kind: RefCell::new(None),
            volume_row,
            volume_icon,
            volume,
            output_chooser,
            mic_row,
            mic_icon,
            mic,
            brightness_row,
            brightness,
        });
        qs.build_power_menu(&power_btn);
        {
            let me = Rc::downgrade(&qs);
            crate::placement::dismiss_on_outside_click(&qs.window, &sheet, move || {
                if let Some(q) = me.upgrade() {
                    q.hide();
                }
            });
        }
        qs.connect(&refresh, &close);
        qs
    }

    fn build_power_menu(self: &Rc<Self>, button: &gtk::MenuButton) {
        let pop = gtk::Popover::new();
        pop.add_css_class("menu");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        type Entry<'a> = (&'a str, &'a str, Box<dyn Fn()>);
        let entries: [Entry; 4] = [
            ("bedtime", "Suspend", {
                let s = self.services.clone();
                Box::new(move || s.send(Command::Suspend))
            }),
            ("restart_alt", "Restart", {
                let s = self.services.clone();
                Box::new(move || s.send(Command::Reboot))
            }),
            ("power_settings_new", "Shut down", {
                let s = self.services.clone();
                Box::new(move || s.send(Command::PowerOff))
            }),
            ("logout", "Log out", Box::new(|| ipc::action(Action::Quit))),
        ];
        for (icon, label, f) in entries {
            let b = gtk::Button::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.append(&icons::symbol_with(icon, &["small"]));
            row.append(&gtk::Label::new(Some(label)));
            b.set_child(Some(&row));
            b.add_css_class("m3-menu-item");
            b.add_css_class("flat");
            let pop2 = pop.clone();
            b.connect_clicked(move |_| {
                pop2.popdown();
                f();
            });
            list.append(&b);
        }
        pop.set_child(Some(&list));
        button.set_popover(Some(&pop));
    }

    fn connect(self: &Rc<Self>, refresh: &gtk::Button, close: &gtk::Button) {
        let s = self.services.clone();
        let me = Rc::downgrade(self);
        self.wifi.button.connect_toggled(move |b| {
            if let Some(q) = me.upgrade().filter(|q| !q.updating.get()) {
                q.wifi.show_active(b.is_active());
                s.send(Command::SetWifiEnabled(b.is_active()));
                q.reconcile_later();
            }
        });
        let s = self.services.clone();
        let me = Rc::downgrade(self);
        self.bluetooth.button.connect_toggled(move |b| {
            if let Some(q) = me.upgrade().filter(|q| !q.updating.get()) {
                q.bluetooth.show_active(b.is_active());
                s.send(Command::SetBluetoothPowered(b.is_active()));
                q.reconcile_later();
            }
        });
        let s = self.services.clone();
        let me = Rc::downgrade(self);
        self.dnd.button.connect_toggled(move |b| {
            if let Some(q) = me.upgrade().filter(|q| !q.updating.get()) {
                q.dnd.show_active(b.is_active());
                s.send(Command::SetDoNotDisturb(b.is_active()));
                q.reconcile_later();
            }
        });
        let me = Rc::downgrade(self);
        self.night.button.connect_toggled(move |b| {
            if let Some(q) = me.upgrade().filter(|q| !q.updating.get()) {
                q.night.show_active(b.is_active());
                set_night_light(b.is_active());
            }
        });
        let me = Rc::downgrade(self);
        self.dark.button.connect_toggled(move |b| {
            if let Some(q) = me.upgrade().filter(|q| !q.updating.get()) {
                q.dark.show_active(b.is_active());
                set_dark(b.is_active());
            }
        });
        let s = self.services.clone();
        let me = Rc::downgrade(self);
        self.profile.button.connect_clicked(move |_| {
            let Some(q) = me.upgrade() else { return };
            if q.updating.get() {
                return;
            }
            let next = q.state.borrow().profiles.as_ref().and_then(|p| {
                let i = p.available.iter().position(|x| *x == p.active).unwrap_or(0);
                p.available.get((i + 1) % p.available.len().max(1)).cloned()
            });
            if let Some(next) = next {
                // Show the new mode immediately.
                let (icon, label) = status::profile_label(&next);
                q.profile.icon.set_icon(icon);
                q.profile.show_active(next != "balanced");
                q.profile.subtitle.set_text(label);
                s.send(Command::SetPowerProfile(next));
                q.reconcile_later();
            }
        });
        let s = self.services.clone();
        let me = Rc::downgrade(self);
        self.airplane.button.connect_toggled(move |b| {
            let Some(q) = me.upgrade().filter(|q| !q.updating.get()) else { return };
            let on = b.is_active();
            q.airplane.show_active(on);
            let st = q.state.borrow().clone();
            let wifi_present = st.network.as_ref().is_some_and(|n| n.has_wifi_device);
            let bt_present = st.bluetooth.as_ref().is_some_and(|b| b.available);
            if on {
                // Remember which radios were on, so turning airplane mode
                // off restores exactly those.
                let wifi_on = st.network.as_ref().is_some_and(|n| n.wifi_enabled);
                let bt_on = st.bluetooth.as_ref().is_some_and(|b| b.powered);
                q.airplane_restore.set((wifi_on, bt_on));
                if wifi_present {
                    s.send(Command::SetWifiEnabled(false));
                }
                if bt_present {
                    s.send(Command::SetBluetoothPowered(false));
                }
            } else {
                let (wifi_on, bt_on) = q.airplane_restore.get();
                // Nothing was on before: bring Wi-Fi back at least.
                let wifi_on = wifi_on || !bt_on;
                if wifi_present && wifi_on {
                    s.send(Command::SetWifiEnabled(true));
                }
                if bt_present && bt_on {
                    s.send(Command::SetBluetoothPowered(true));
                }
            }
            q.reconcile_later();
        });
        for (tile, kind) in [(&self.wifi, "wifi"), (&self.bluetooth, "bluetooth")] {
            if let Some(m) = &tile.more {
                let me = Rc::downgrade(self);
                m.connect_clicked(move |_| {
                    if let Some(q) = me.upgrade() {
                        q.open_detail(kind);
                    }
                });
            }
        }
        let me = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(q) = me.upgrade() {
                *q.detail_kind.borrow_mut() = None;
                q.detail.set_reveal_child(false);
                q.services.send(Command::SetBluetoothDiscovery(false));
            }
        });
        let me = Rc::downgrade(self);
        refresh.connect_clicked(move |_| {
            if let Some(q) = me.upgrade() {
                match *q.detail_kind.borrow() {
                    Some("wifi") => q.services.send(Command::ScanWifi),
                    Some("bluetooth") => q.services.send(Command::SetBluetoothDiscovery(true)),
                    _ => {}
                }
            }
        });

        let a = self.audio.clone();
        let me = Rc::downgrade(self);
        let send = throttled(move |v| a.send(AudioCmd::SetOutputVolume(v)));
        self.volume.connect_value_changed(move |sc| {
            if me.upgrade().is_some_and(|q| !q.updating.get()) {
                send(sc.value());
            }
        });
        let a = self.audio.clone();
        self.volume_icon.connect_clicked(move |_| a.send(AudioCmd::ToggleOutputMute));
        let a = self.audio.clone();
        let me = Rc::downgrade(self);
        let send = throttled(move |v| a.send(AudioCmd::SetInputVolume(v)));
        self.mic.connect_value_changed(move |sc| {
            if me.upgrade().is_some_and(|q| !q.updating.get()) {
                send(sc.value());
            }
        });
        let a = self.audio.clone();
        self.mic_icon.connect_clicked(move |_| a.send(AudioCmd::ToggleInputMute));
        let s = self.services.clone();
        let me = Rc::downgrade(self);
        let send = throttled(move |v| s.send(Command::SetBrightness(v)));
        self.brightness.connect_value_changed(move |sc| {
            if me.upgrade().is_some_and(|q| !q.updating.get()) {
                send(sc.value());
            }
        });
        let a = self.audio.clone();
        let me = Rc::downgrade(self);
        self.output_chooser.connect_selected_notify(move |dd| {
            let Some(q) = me.upgrade() else { return };
            if q.updating.get() {
                return;
            }
            let name = q
                .state
                .borrow()
                .audio
                .as_ref()
                .and_then(|st| st.sinks.get(dd.selected() as usize).map(|d| d.name.clone()));
            if let Some(n) = name {
                a.send(AudioCmd::SetDefaultOutput(n));
            }
        });
        // Escape closes.
        let keys = gtk::EventControllerKey::new();
        let me = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gtk::gdk::Key::Escape {
                if let Some(q) = me.upgrade() {
                    q.hide();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);
    }

    fn open_detail(self: &Rc<Self>, kind: &'static str) {
        *self.detail_kind.borrow_mut() = Some(kind);
        match kind {
            "wifi" => self.services.send(Command::ScanWifi),
            _ => self.services.send(Command::SetBluetoothDiscovery(true)),
        }
        self.rebuild_detail();
        self.detail.set_reveal_child(true);
    }

    fn rebuild_detail(self: &Rc<Self>) {
        let list = &self.detail_list;
        while let Some(c) = list.first_child() {
            list.remove(&c);
        }
        let st = self.state.borrow();
        // Grouped-list rows: the icon sits on a shape (cookie when
        // connected), an optional trailing icon (lock for secured networks).
        let row = |icon: &str, title: &str, sub: &str, active: bool, trailing: Option<&str>| {
            let b = gtk::Button::new();
            b.add_css_class("m3-qs-item");
            if active {
                b.add_css_class("active");
            }
            let r = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            let shape = if active { m3_widgets::Shape::Cookie(9) } else { m3_widgets::Shape::Circle };
            let ic = m3_widgets::ShapeIcon::new(shape, icon, 40, true);
            if !active {
                ic.label().remove_css_class("filled");
            }
            ic.widget().add_css_class("m3-qs-item-icon");
            r.append(ic.widget());
            let c = gtk::Box::new(gtk::Orientation::Vertical, 0);
            c.set_hexpand(true);
            c.set_valign(gtk::Align::Center);
            let t = gtk::Label::new(Some(title));
            t.set_xalign(0.0);
            t.set_ellipsize(gtk::pango::EllipsizeMode::End);
            t.add_css_class("m3-body-large");
            c.append(&t);
            if !sub.is_empty() {
                let s = gtk::Label::new(Some(sub));
                s.set_xalign(0.0);
                s.add_css_class("m3-body-small");
                s.add_css_class("m3-secondary-text");
                c.append(&s);
            }
            r.append(&c);
            if let Some(tr) = trailing {
                r.append(&icons::symbol_with(tr, &["small", "m3-secondary-text"]));
            }
            b.set_child(Some(&r));
            b
        };
        let placeholder = |text: &str, busy: bool| {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.add_css_class("m3-qs-placeholder");
            b.set_halign(gtk::Align::Center);
            if busy {
                b.append(m3_widgets::LoadingIndicator::new(32).widget());
            }
            let l = gtk::Label::new(Some(text));
            l.add_css_class("m3-secondary-text");
            b.append(&l);
            b
        };
        match *self.detail_kind.borrow() {
            Some("wifi") => {
                self.detail_title.set_text("Wi-Fi networks");
                let Some(n) = &st.network else { return };
                if !n.wifi_enabled {
                    list.append(&placeholder("Wi-Fi is off", false));
                } else if n.access_points.is_empty() {
                    list.append(&placeholder("Searching for networks…", true));
                }
                for ap in &n.access_points {
                    let icon = match ap.strength {
                        75.. => "network_wifi",
                        50..=74 => "network_wifi_3_bar",
                        25..=49 => "network_wifi_2_bar",
                        _ => "network_wifi_1_bar",
                    };
                    let sub = if ap.active {
                        "Connected"
                    } else if ap.saved {
                        "Saved"
                    } else if ap.secure {
                        "Secured"
                    } else {
                        "Open"
                    };
                    let lock = (ap.secure && !ap.active).then_some("lock");
                    let b = row(icon, &ap.ssid, sub, ap.active, lock);
                    let ssid = ap.ssid.clone();
                    let needs_password = ap.secure && !ap.saved && !ap.active;
                    let active = ap.active;
                    let me = Rc::downgrade(self);
                    b.connect_clicked(move |btn| {
                        let Some(q) = me.upgrade() else { return };
                        if active {
                            q.services.send(Command::DisconnectWifi);
                        } else if needs_password {
                            q.ask_password(btn, &ssid);
                        } else {
                            q.services.send(Command::ConnectWifi { ssid: ssid.clone(), password: None });
                        }
                    });
                    list.append(&b);
                }
            }
            Some("bluetooth") => {
                self.detail_title.set_text("Bluetooth devices");
                let Some(bt) = &st.bluetooth else { return };
                if !bt.powered {
                    list.append(&placeholder("Bluetooth is off", false));
                } else if bt.devices.is_empty() {
                    list.append(&placeholder("Looking for devices…", true));
                }
                for d in &bt.devices {
                    let mut sub = if d.connected {
                        "Connected".to_owned()
                    } else if d.paired {
                        "Paired".to_owned()
                    } else {
                        "Available — select to pair".to_owned()
                    };
                    if let Some(b) = d.battery {
                        sub.push_str(&format!(" · {b}%"));
                    }
                    let icon = match d.icon.as_str() {
                        i if i.contains("headset") || i.contains("headphone") => "headphones",
                        i if i.contains("mouse") => "mouse",
                        i if i.contains("keyboard") => "keyboard",
                        i if i.contains("phone") => "smartphone",
                        i if i.contains("audio") => "speaker",
                        _ => "bluetooth",
                    };
                    let b = row(icon, &d.name, &sub, d.connected, None);
                    let (path, connected, paired) = (d.path.clone(), d.connected, d.paired);
                    let s = self.services.clone();
                    b.connect_clicked(move |_| {
                        s.send(if connected {
                            Command::DisconnectBluetooth { path: path.clone() }
                        } else if paired {
                            Command::ConnectBluetooth { path: path.clone() }
                        } else {
                            Command::PairBluetooth { path: path.clone() }
                        });
                    });
                    list.append(&b);
                }
            }
            _ => {}
        }
    }

    fn ask_password(self: &Rc<Self>, anchor: &gtk::Button, ssid: &str) {
        let pop = gtk::Popover::new();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        b.set_margin_start(12);
        b.set_margin_end(12);
        b.set_margin_top(8);
        b.set_margin_bottom(8);
        let entry = gtk::PasswordEntry::new();
        entry.set_show_peek_icon(true);
        entry.set_property("placeholder-text", format!("Password for {ssid}"));
        let ok = gtk::Button::with_label("Connect");
        ok.add_css_class("filled");
        b.append(&entry);
        b.append(&ok);
        pop.set_child(Some(&b));
        pop.set_parent(anchor);
        let s = self.services.clone();
        let ssid = ssid.to_owned();
        let submit = {
            let entry = entry.clone();
            let pop = pop.clone();
            move || {
                let pw = entry.text().to_string();
                if pw.len() >= 8 {
                    s.send(Command::ConnectWifi { ssid: ssid.clone(), password: Some(pw) });
                    pop.popdown();
                } else {
                    entry.add_css_class("error");
                }
            }
        };
        let submit = Rc::new(submit);
        let s2 = submit.clone();
        ok.connect_clicked(move |_| s2());
        entry.connect_activate(move |_| submit());
        pop.connect_closed(|p| p.unparent());
        pop.popup();
        entry.grab_focus();
    }

    pub fn update(self: &Rc<Self>, st: &SysState) {
        *self.state.borrow_mut() = st.clone();
        self.updating.set(true);

        // Battery
        match &st.battery {
            Some(b) if b.present => self.battery.set_text(&status::battery_text(b)),
            _ => self.battery.set_text(""),
        }
        // Network
        match &st.network {
            Some(n) if n.available && n.has_wifi_device => {
                self.wifi.root.set_visible(true);
                let sub = if !n.wifi_enabled {
                    "Off".to_owned()
                } else {
                    n.connected_ssid.clone().unwrap_or_else(|| "Not connected".into())
                };
                self.wifi.set(n.wifi_enabled, &sub);
                self.wifi.button.set_sensitive(n.wifi_hardware_enabled);
            }
            _ => self.wifi.root.set_visible(false),
        }
        match &st.network {
            // Only worth a row when a cable is actually in use.
            Some(n) if n.available && n.ethernet_present && n.ethernet_connected => {
                self.ethernet.set_visible(true);
                self.ethernet_label.set_text("Ethernet connected");
            }
            _ => self.ethernet.set_visible(false),
        }
        // Bluetooth
        match &st.bluetooth {
            Some(b) if b.available => {
                self.bluetooth.root.set_visible(true);
                let connected: Vec<&str> = b.devices.iter().filter(|d| d.connected).map(|d| d.name.as_str()).collect();
                let sub = if !b.powered {
                    "Off".to_owned()
                } else if connected.is_empty() {
                    "On".into()
                } else {
                    connected.join(", ")
                };
                self.bluetooth.set(b.powered, &sub);
            }
            _ => self.bluetooth.root.set_visible(false),
        }
        let radios = st.network.as_ref().is_some_and(|n| n.has_wifi_device)
            || st.bluetooth.as_ref().is_some_and(|b| b.available);
        self.airplane.root.set_visible(radios);
        let wifi_off = st.network.as_ref().is_none_or(|n| !n.has_wifi_device || !n.wifi_enabled);
        let bt_off = st.bluetooth.as_ref().is_none_or(|b| !b.available || !b.powered);
        self.airplane.set(radios && wifi_off && bt_off, if radios && wifi_off && bt_off { "On" } else { "Off" });

        self.dnd.set(st.dnd, if st.dnd { "On" } else { "Off" });
        let nl = self.night_light();
        let now = m3_core::time::local_minutes();
        let night_on = nl.active_at(now);
        let night_sub = match (nl.mode, night_on) {
            (cfg::NightLightMode::Scheduled, true) => format!("Until {}", nl.until),
            (cfg::NightLightMode::Scheduled, false) => format!("From {}", nl.from),
            (_, true) => "On".to_owned(),
            (_, false) => "Off".to_owned(),
        };
        self.night.set(night_on, &night_sub);
        let dark = self.theme_mode();
        self.dark.set(
            dark == ThemeMode::Dark,
            match dark {
                ThemeMode::Dark => "On",
                ThemeMode::Light => "Off",
                ThemeMode::Auto => "Automatic",
            },
        );
        match &st.profiles {
            Some(p) if !p.available.is_empty() => {
                self.profile.root.set_visible(true);
                let (icon, label) = status::profile_label(&p.active);
                self.profile.icon.set_icon(icon);
                self.profile.set(p.active != "balanced", label);
            }
            _ => self.profile.root.set_visible(false),
        }
        self.relayout_tiles();
        // Audio
        match &st.audio {
            Some(a) if a.available => {
                self.volume_row.set_visible(a.output().is_some());
                if let Some(o) = a.output() {
                    set_unless_dragging(&self.volume, o.volume.min(1.0));
                    if let Some(l) = self.volume_icon.child().and_downcast::<gtk::Label>() {
                        l.set_text(status::volume_icon(o.volume, o.muted));
                    }
                }
                let names: Vec<String> = a.sinks.iter().map(|d| d.description.clone()).collect();
                if *self.sink_names.borrow() != names {
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    self.output_chooser.set_model(Some(&gtk::StringList::new(&refs)));
                    *self.sink_names.borrow_mut() = names;
                }
                if let Some(i) = a.sinks.iter().position(|d| Some(&d.name) == a.default_sink.as_ref()) {
                    if self.output_chooser.selected() != i as u32 {
                        self.output_chooser.set_selected(i as u32);
                    }
                }
                self.output_chooser.set_visible(a.sinks.len() > 1);
                self.mic_row.set_visible(a.input().is_some());
                if let Some(i) = a.input() {
                    set_unless_dragging(&self.mic, i.volume.min(1.0));
                    if let Some(l) = self.mic_icon.child().and_downcast::<gtk::Label>() {
                        l.set_text(if i.muted { "mic_off" } else { "mic" });
                    }
                }
            }
            _ => {
                self.volume_row.set_visible(false);
                self.mic_row.set_visible(false);
            }
        }
        self.brightness_row.set_visible(st.brightness.is_some());
        if let Some(b) = st.brightness {
            set_unless_dragging(&self.brightness, b);
        }
        if self.detail.reveals_child() {
            let sig = self.detail_signature(st);
            if *self.detail_sig.borrow() != sig {
                *self.detail_sig.borrow_mut() = sig;
                self.rebuild_detail();
            }
        }
        self.updating.set(false);
    }

    /// What the open detail list shows; it is rebuilt only when this changes
    /// (not on every volume or battery event).
    fn detail_signature(&self, st: &SysState) -> String {
        match *self.detail_kind.borrow() {
            Some("wifi") => format!("{:?}", st.network.as_ref().map(|n| (n.wifi_enabled, &n.access_points))),
            Some("bluetooth") => format!("{:?}", st.bluetooth.as_ref().map(|b| (b.powered, &b.devices))),
            _ => String::new(),
        }
    }

    /// Night-light settings from `config.toml` (re-read only when changed).
    fn night_light(&self) -> cfg::NightLight {
        let Ok(dir) = cfg::default_dir() else { return cfg::NightLight::default() };
        let mtime = std::fs::metadata(dir.join("config.toml")).and_then(|m| m.modified()).ok();
        if let (Some(m), Some((cached_m, nl))) = (mtime, self.night_cache.borrow().as_ref()) {
            if m == *cached_m {
                return nl.clone();
            }
        }
        let nl = cfg::load::<cfg::CompositorConfig>(&dir).map(|l| l.value.night_light).unwrap_or_default();
        if let Some(m) = mtime {
            *self.night_cache.borrow_mut() = Some((m, nl.clone()));
        }
        nl
    }

    /// The theme mode from `theme.toml`, re-read only when the file changed.
    fn theme_mode(&self) -> ThemeMode {
        let Ok(dir) = cfg::default_dir() else { return ThemeMode::default() };
        let mtime = std::fs::metadata(dir.join("theme.toml")).and_then(|m| m.modified()).ok();
        if let (Some(m), Some((cached_m, mode))) = (mtime, *self.dark_cache.borrow()) {
            if m == cached_m {
                return mode;
            }
        }
        let mode = cfg::load::<ThemeConfig>(&dir).map(|l| l.value.mode).unwrap_or_default();
        if let Some(m) = mtime {
            *self.dark_cache.borrow_mut() = Some((m, mode));
        }
        mode
    }

    /// After a toggle, ask the services for fresh state so a change that
    /// failed doesn't leave the tile showing the wrong thing.
    fn reconcile_later(&self) {
        let s = self.services.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || s.send(Command::Refresh));
    }

    /// Place the visible tiles two per row in a fixed order, so hidden tiles
    /// (no Bluetooth adapter, no power profiles, …) never leave gaps. An odd
    /// last tile spans the whole row.
    fn relayout_tiles(&self) {
        let tiles = [&self.wifi, &self.bluetooth, &self.dnd, &self.dark, &self.night, &self.profile, &self.airplane];
        let visible: Vec<&gtk::Box> = tiles.iter().map(|t| &t.root).filter(|r| r.is_visible()).collect();
        let placed: Vec<(i32, i32, i32)> = visible
            .iter()
            .map(|r| {
                let (c, row, w, _) = self.grid.query_child(*r);
                (c, row, w)
            })
            .collect();
        let wanted: Vec<(i32, i32, i32)> = (0..visible.len())
            .map(|i| {
                let last_alone = i == visible.len() - 1 && visible.len() % 2 == 1;
                ((i % 2) as i32, (i / 2) as i32, if last_alone { 2 } else { 1 })
            })
            .collect();
        if placed == wanted {
            return;
        }
        for t in tiles {
            if t.root.parent().is_some() {
                self.grid.remove(&t.root);
            }
        }
        for (r, (c, row, w)) in visible.iter().zip(wanted) {
            self.grid.attach(*r, c, row, w, 1);
        }
        // Hidden tiles stay in the grid (invisible) so they can reappear.
        let mut next_row = visible.len().div_ceil(2) as i32;
        for t in tiles {
            if t.root.parent().is_none() {
                self.grid.attach(&t.root, 0, next_row, 1, 1);
                next_row += 1;
            }
        }
    }

    /// Follow the panel's 12/24-hour clock setting.
    pub fn set_clock_12h(&self, h12: bool) {
        self.clock_12h.set(h12);
        (self.clock_tick)();
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.window.is_visible() { self.hide() } else { self.show() }
    }

    pub fn show(self: &Rc<Self>) {
        self.services.send(Command::Refresh);
        // Take the keyboard while open: Escape works straight away.
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        let pos = crate::placement::panel_position();
        crate::placement::anchor_sheet(&self.window, &self.revealer, pos);
        if let Some(sheet) = self.revealer.child() {
            sheet.remove_css_class("m3-enter");
            sheet.remove_css_class("m3-enter-up");
            m3_widgets::animate(&sheet, crate::placement::enter_class(pos));
        }
        self.window.present();
        self.revealer.set_reveal_child(true);
    }

    pub fn hide(&self) {
        if !self.window.is_visible() {
            return;
        }
        self.window.set_keyboard_mode(KeyboardMode::None);
        self.revealer.set_reveal_child(false);
        self.detail.set_reveal_child(false);
        let w = self.window.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(260), move || w.set_visible(false));
    }
}

/// Night light tile: on now (until turned off) or off. A schedule set in
/// Settings is kept when turning it on again from Settings.
fn set_night_light(on: bool) {
    let Ok(dir) = cfg::default_dir() else { return };
    let mut c = match cfg::load::<cfg::CompositorConfig>(&dir) {
        Ok(l) => l.value,
        Err(e) => {
            tracing::warn!("not changing night light: {e}");
            return;
        }
    };
    c.night_light.mode = if on { cfg::NightLightMode::Always } else { cfg::NightLightMode::Off };
    if let Err(e) = cfg::save(&dir, &c) {
        tracing::warn!("cannot save config.toml: {e}");
    }
    ipc::send(m3_protocol::Request::Reload);
}

fn set_dark(dark: bool) {
    let Ok(dir) = cfg::default_dir() else { return };
    let mut t = match cfg::load::<ThemeConfig>(&dir) {
        Ok(l) => l.value,
        Err(e) => {
            tracing::warn!("not changing the theme: {e}");
            return;
        }
    };
    t.mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
    if let Err(e) = cfg::save(&dir, &t) {
        tracing::warn!("cannot save theme.toml: {e}");
    }
    ipc::send(m3_protocol::Request::Reload);
}

pub fn launch_settings() {
    let ctx = gtk::gdk::Display::default().map(|d| d.app_launch_context());
    let app =
        gio_unix::DesktopAppInfo::new("org.m3.Settings.desktop").map(|a| a.upcast::<gio::AppInfo>()).or_else(|| {
            gio::AppInfo::create_from_commandline("m3-settings", Some("Settings"), gio::AppInfoCreateFlags::NONE).ok()
        });
    match app {
        Some(a) => {
            if let Err(e) = a.launch(&[], ctx.as_ref()) {
                tracing::warn!("cannot open Settings: {e}");
            }
        }
        None => tracing::warn!("m3-settings is not installed"),
    }
}

/// Coalesce rapid slider changes: send the latest value at most every 40 ms
/// (dragging a slider otherwise floods PipeWire / logind with requests).
fn throttled(send: impl Fn(f64) + 'static) -> impl Fn(f64) {
    let pending: Rc<Cell<Option<f64>>> = Rc::default();
    let send = Rc::new(send);
    move |v| {
        let first = pending.replace(Some(v)).is_none();
        if first {
            let pending = pending.clone();
            let send = send.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
                if let Some(v) = pending.take() {
                    send(v);
                }
            });
        }
    }
}

/// Update a slider from the service unless the user is dragging it.
fn set_unless_dragging(scale: &gtk::Scale, v: f64) {
    if !scale.state_flags().contains(gtk::StateFlags::ACTIVE) && (scale.value() - v).abs() > 0.004 {
        scale.set_value(v);
    }
}
