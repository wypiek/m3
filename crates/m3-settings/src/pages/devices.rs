//! Network, Bluetooth and Sound — backed by NetworkManager, BlueZ and the
//! sound server.

use std::rc::Rc;

use gtk::prelude::*;
use m3_services::{Command, audio::AudioCmd};

use crate::{Ctx, ui};

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn unavailable(parent: &gtk::Box, what: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(what));
    l.add_css_class("m3-secondary-text");
    l.set_wrap(true);
    l.set_xalign(0.0);
    parent.append(&l);
    l
}

pub fn network(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Network", "Wi-Fi and wired connections, managed by NetworkManager.");
    let status = unavailable(&content, "Looking for NetworkManager…");
    let card = ui::card(&content, "Wi-Fi");
    let wifi = gtk::Switch::new();
    ui::row(&card, "Wi-Fi", "", &wifi);
    {
        let s = ctx.services.clone();
        wifi.connect_state_set(move |_, on| {
            s.send(Command::SetWifiEnabled(on));
            gtk::glib::Propagation::Proceed
        });
    }
    let scan = gtk::Button::with_label("Scan");
    scan.add_css_class("tonal");
    {
        let s = ctx.services.clone();
        scan.connect_clicked(move |_| s.send(Command::ScanWifi));
    }
    ui::row(&card, "Available networks", "", &scan);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.append(&list);
    let wired = ui::card(&content, "Wired");
    let wired_label = ui::info(&wired, "Ethernet", "");
    if gtk::gio::AppInfo::all().iter().any(|a| a.id().is_some_and(|i| i == "nm-connection-editor.desktop")) {
        let adv = gtk::Button::with_label("Advanced connection settings");
        adv.add_css_class("outlined");
        adv.connect_clicked(|_| {
            let _ = gtk::gio::AppInfo::create_from_commandline(
                "nm-connection-editor",
                None,
                gtk::gio::AppInfoCreateFlags::NONE,
            )
            .and_then(|a| a.launch(&[], None::<&gtk::gio::AppLaunchContext>));
        });
        content.append(&adv);
    }
    let c = ctx.clone();
    let card2 = card.clone();
    ctx.on_sys(move |sys| {
        let Some(n) = &sys.network else { return };
        if !n.available {
            status.set_text("NetworkManager is not running, so networks cannot be managed here.");
            status.set_visible(true);
            card2.set_visible(false);
            wired.set_visible(false);
            return;
        }
        status.set_visible(false);
        card2.set_visible(n.has_wifi_device);
        wired.set_visible(n.ethernet_present);
        wired_label.set_text(if n.ethernet_connected { "Connected" } else { "Cable unplugged" });
        if wifi.is_active() != n.wifi_enabled {
            wifi.set_state(n.wifi_enabled);
        }
        clear(&list);
        for ap in &n.access_points {
            let b = gtk::Button::with_label(if ap.active { "Disconnect" } else { "Connect" });
            b.add_css_class(if ap.active { "outlined" } else { "tonal" });
            let sub = format!(
                "{}% · {}{}",
                ap.strength,
                if ap.secure { "secured" } else { "open" },
                if ap.saved { " · saved" } else { "" }
            );
            let ssid = ap.ssid.clone();
            let (active, needs_pw) = (ap.active, ap.secure && !ap.saved && !ap.active);
            let c2 = c.clone();
            b.connect_clicked(move |_| {
                if active {
                    c2.services.send(Command::DisconnectWifi);
                } else if needs_pw {
                    password_dialog(&c2, &ssid);
                } else {
                    c2.services.send(Command::ConnectWifi { ssid: ssid.clone(), password: None });
                }
            });
            ui::row(&list, &ap.ssid, &sub, &b);
        }
        if n.access_points.is_empty() {
            unavailable(&list, if n.wifi_enabled { "No networks found yet." } else { "Wi-Fi is off." });
        }
    });
    page.upcast()
}

fn password_dialog(ctx: &Rc<Ctx>, ssid: &str) {
    let d = gtk::Window::builder()
        .transient_for(&ctx.window)
        .modal(true)
        .title("Wi-Fi password")
        .default_width(400)
        .build();
    d.add_css_class("m3");
    let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
    b.add_css_class("m3-dialog");
    let t = gtk::Label::new(Some(&format!("Connect to “{ssid}”")));
    t.add_css_class("m3-dialog-title");
    let e = gtk::PasswordEntry::new();
    e.set_show_peek_icon(true);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("text");
    let ok = gtk::Button::with_label("Connect");
    ok.add_css_class("filled");
    row.append(&cancel);
    row.append(&ok);
    b.append(&t);
    b.append(&e);
    b.append(&row);
    d.set_child(Some(&b));
    {
        let d = d.clone();
        cancel.connect_clicked(move |_| d.close());
    }
    let submit = {
        let (s, ssid, d, e) = (ctx.services.clone(), ssid.to_owned(), d.clone(), e.clone());
        Rc::new(move || {
            let pw = e.text().to_string();
            if pw.len() < 8 {
                e.add_css_class("error");
                return;
            }
            s.send(Command::ConnectWifi { ssid: ssid.clone(), password: Some(pw) });
            d.close();
        })
    };
    let s2 = submit.clone();
    ok.connect_clicked(move |_| s2());
    e.connect_activate(move |_| submit());
    d.present();
}

pub fn bluetooth(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Bluetooth", "Pair and connect devices through BlueZ.");
    let status = unavailable(&content, "Looking for Bluetooth…");
    let card = ui::card(&content, "Bluetooth");
    let power = gtk::Switch::new();
    ui::row(&card, "Bluetooth", "", &power);
    {
        let s = ctx.services.clone();
        power.connect_state_set(move |_, on| {
            s.send(Command::SetBluetoothPowered(on));
            gtk::glib::Propagation::Proceed
        });
    }
    let discover = gtk::ToggleButton::with_label("Search for devices");
    discover.add_css_class("tonal");
    {
        let s = ctx.services.clone();
        discover.connect_toggled(move |b| s.send(Command::SetBluetoothDiscovery(b.is_active())));
    }
    ui::row(&card, "Visible nearby devices", "", &discover);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.append(&list);
    ui::note(
        &content,
        "Devices that require entering a PIN are not supported yet; headphones, speakers, mice and most keyboards pair without one.",
    );
    let c = ctx.clone();
    let card2 = card.clone();
    ctx.on_sys(move |sys| {
        let Some(b) = &sys.bluetooth else { return };
        if !b.available {
            status.set_text("No Bluetooth adapter was found, or BlueZ is not running.");
            status.set_visible(true);
            card2.set_visible(false);
            return;
        }
        status.set_visible(false);
        card2.set_visible(true);
        if power.is_active() != b.powered {
            power.set_state(b.powered);
        }
        discover.set_sensitive(b.powered);
        clear(&list);
        for d in &b.devices {
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let main = gtk::Button::with_label(if d.connected {
                "Disconnect"
            } else if d.paired {
                "Connect"
            } else {
                "Pair"
            });
            main.add_css_class(if d.connected { "outlined" } else { "tonal" });
            let (path, connected, paired) = (d.path.clone(), d.connected, d.paired);
            let s = c.services.clone();
            main.connect_clicked(move |_| {
                s.send(if connected {
                    Command::DisconnectBluetooth { path: path.clone() }
                } else if paired {
                    Command::ConnectBluetooth { path: path.clone() }
                } else {
                    Command::PairBluetooth { path: path.clone() }
                })
            });
            actions.append(&main);
            if d.paired {
                let forget = gtk::Button::with_label("Forget");
                forget.add_css_class("text");
                let (s, path) = (c.services.clone(), d.path.clone());
                forget.connect_clicked(move |_| s.send(Command::ForgetBluetooth { path: path.clone() }));
                actions.append(&forget);
            }
            let mut sub = if d.connected {
                "Connected".to_owned()
            } else if d.paired {
                "Paired".into()
            } else {
                "Not paired".into()
            };
            if let Some(bat) = d.battery {
                sub.push_str(&format!(" · battery {bat}%"));
            }
            ui::row(&list, &d.name, &sub, &actions);
        }
    });
    page.upcast()
}

pub fn sound(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Sound", "Speakers, microphones and application volumes.");
    let status = unavailable(&content, "Connecting to the sound server…");
    let out_card = ui::card(&content, "Output");
    let out_dev = gtk::DropDown::from_strings(&[]);
    ui::row(&out_card, "Output device", "", &out_dev);
    let out_vol = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    out_vol.set_draw_value(false);
    out_vol.set_size_request(260, -1);
    ui::row(&out_card, "Volume", "", &out_vol);
    let out_mute = gtk::Switch::new();
    ui::row(&out_card, "Mute", "", &out_mute);
    let in_card = ui::card(&content, "Input");
    let in_dev = gtk::DropDown::from_strings(&[]);
    ui::row(&in_card, "Microphone", "", &in_dev);
    let in_vol = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    in_vol.set_draw_value(false);
    in_vol.set_size_request(260, -1);
    ui::row(&in_card, "Input volume", "", &in_vol);
    let in_mute = gtk::Switch::new();
    ui::row(&in_card, "Mute microphone", "", &in_mute);
    let apps_card = ui::card(&content, "Applications");
    let apps = gtk::Box::new(gtk::Orientation::Vertical, 0);
    apps_card.append(&apps);

    let updating = Rc::new(std::cell::Cell::new(false));
    let a = ctx.audio.clone();
    let u = updating.clone();
    out_vol.connect_value_changed(move |s| {
        if !u.get() {
            a.send(AudioCmd::SetOutputVolume(s.value()));
        }
    });
    let a = ctx.audio.clone();
    let u = updating.clone();
    out_mute.connect_active_notify(move |s| {
        if !u.get() {
            a.send(AudioCmd::SetOutputMute(s.is_active()));
        }
    });
    let a = ctx.audio.clone();
    let u = updating.clone();
    in_vol.connect_value_changed(move |s| {
        if !u.get() {
            a.send(AudioCmd::SetInputVolume(s.value()));
        }
    });
    let a = ctx.audio.clone();
    let u = updating.clone();
    in_mute.connect_active_notify(move |_| {
        if !u.get() {
            a.send(AudioCmd::ToggleInputMute);
        }
    });
    let (c, u) = (ctx.clone(), updating.clone());
    out_dev.connect_selected_notify(move |d| {
        if u.get() {
            return;
        }
        let name =
            c.sys.borrow().audio.as_ref().and_then(|a| a.sinks.get(d.selected() as usize).map(|s| s.name.clone()));
        if let Some(n) = name {
            c.audio.send(AudioCmd::SetDefaultOutput(n));
        }
    });
    let (c, u) = (ctx.clone(), updating.clone());
    in_dev.connect_selected_notify(move |d| {
        if u.get() {
            return;
        }
        let name =
            c.sys.borrow().audio.as_ref().and_then(|a| a.sources.get(d.selected() as usize).map(|s| s.name.clone()));
        if let Some(n) = name {
            c.audio.send(AudioCmd::SetDefaultInput(n));
        }
    });

    let c = ctx.clone();
    ctx.on_sys(move |sys| {
        let Some(au) = &sys.audio else { return };
        if !au.available {
            status.set_text("No sound server is running (PipeWire with pipewire-pulse, or PulseAudio).");
            status.set_visible(true);
            return;
        }
        status.set_visible(false);
        updating.set(true);
        let names: Vec<String> = au.sinks.iter().map(|d| d.description.clone()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        out_dev.set_model(Some(&gtk::StringList::new(&refs)));
        if let Some(i) = au.sinks.iter().position(|d| Some(&d.name) == au.default_sink.as_ref()) {
            out_dev.set_selected(i as u32);
        }
        if let Some(o) = au.output() {
            out_vol.set_value(o.volume.min(1.0));
            out_mute.set_active(o.muted);
        }
        out_card.set_visible(!au.sinks.is_empty());
        let names: Vec<String> = au.sources.iter().map(|d| d.description.clone()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        in_dev.set_model(Some(&gtk::StringList::new(&refs)));
        if let Some(i) = au.sources.iter().position(|d| Some(&d.name) == au.default_source.as_ref()) {
            in_dev.set_selected(i as u32);
        }
        if let Some(i) = au.input() {
            in_vol.set_value(i.volume.min(1.0));
            in_mute.set_active(i.muted);
        }
        in_card.set_visible(!au.sources.is_empty());
        clear(&apps);
        for st in &au.streams {
            let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
            s.set_draw_value(false);
            s.set_size_request(220, -1);
            s.set_value(st.volume.min(1.0));
            let (a, idx) = (c.audio.clone(), st.index);
            s.connect_value_changed(move |s| a.send(AudioCmd::SetStreamVolume { index: idx, volume: s.value() }));
            ui::row(&apps, &st.app_name, if st.muted { "Muted" } else { "" }, &s);
        }
        if au.streams.is_empty() {
            unavailable(&apps, "No application is playing sound.");
        }
        updating.set(false);
    });
    page.upcast()
}
