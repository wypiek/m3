//! Power, and Privacy & Security (screen locking).

use std::rc::Rc;

use gtk::prelude::*;
use m3_services::{Command, power::ChargeState};

use crate::{Ctx, store, ui};

fn duration(s: i64) -> String {
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 { format!("{h} h {m} min") } else { format!("{m} min") }
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Power", "Battery, performance and automatic suspend.");
    let bat_card = ui::card(&content, "Battery");
    let level = ui::info(&bat_card, "Charge", "");
    let state = ui::info(&bat_card, "Status", "");
    let prof_card = ui::card(&content, "Power mode");
    let prof_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    prof_box.add_css_class("m3-button-group");
    ui::row(&prof_card, "Mode", "Balance performance against battery life", &prof_box);

    let c = ctx.clone();
    ctx.on_sys(move |sys| {
        match sys.battery.as_ref().filter(|b| b.present) {
            Some(b) => {
                bat_card.set_visible(true);
                level.set_text(&format!("{:.0}%", b.percentage));
                state.set_text(&match b.state {
                    ChargeState::Charging => b
                        .time_to_full
                        .map(|t| format!("Charging — full in {}", duration(t)))
                        .unwrap_or_else(|| "Charging".into()),
                    ChargeState::Discharging => b
                        .time_to_empty
                        .map(|t| format!("On battery — {} left", duration(t)))
                        .unwrap_or_else(|| "On battery".into()),
                    ChargeState::Full => "Fully charged".into(),
                    ChargeState::NotCharging => "Plugged in, not charging".into(),
                    ChargeState::Unknown => "Unknown".into(),
                });
            }
            None => bat_card.set_visible(false),
        }
        match &sys.profiles {
            Some(p) if !p.available.is_empty() => {
                prof_card.set_visible(true);
                while let Some(ch) = prof_box.first_child() {
                    prof_box.remove(&ch);
                }
                let mut first: Option<gtk::ToggleButton> = None;
                for name in &p.available {
                    let label = match name.as_str() {
                        "power-saver" => "Power saver",
                        "performance" => "Performance",
                        _ => "Balanced",
                    };
                    let t = gtk::ToggleButton::with_label(label);
                    if let Some(f) = &first {
                        t.set_group(Some(f));
                    } else {
                        first = Some(t.clone());
                    }
                    t.set_active(*name == p.active);
                    let (s, n) = (c.services.clone(), name.clone());
                    t.connect_toggled(move |t| {
                        if t.is_active() {
                            s.send(Command::SetPowerProfile(n.clone()));
                        }
                    });
                    prof_box.append(&t);
                }
            }
            _ => prof_card.set_visible(false),
        }
    });

    let comp = ctx.compositor.get();
    let card = ui::card(&content, "Screen");
    {
        let c = ctx.clone();
        let labels: Vec<&str> = ui::TIMEOUTS.iter().map(|t| t.1).collect();
        ui::dropdown(
            &card,
            "Turn off the screen when inactive for",
            "Any key or pointer movement turns it back on",
            &labels,
            ui::timeout_index(comp.idle.screen_off_after),
            move |i| {
                c.compositor.update(|x| x.idle.screen_off_after = ui::TIMEOUTS[i].0);
            },
        );
    }
    let card = ui::card(&content, "Automatic suspend");
    let labels: Vec<&str> = ui::TIMEOUTS.iter().map(|t| t.1).collect();
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Suspend when inactive for",
            "Video players and presentations keep the computer awake",
            &labels,
            ui::timeout_index(comp.idle.suspend_after),
            move |i| {
                c.compositor.update(|x| x.idle.suspend_after = ui::TIMEOUTS[i].0);
            },
        );
    }
    let card = ui::card(&content, "Session");
    for (label, cmd) in
        [("Suspend now", Command::Suspend), ("Restart", Command::Reboot), ("Shut down", Command::PowerOff)]
    {
        let b = gtk::Button::with_label(label);
        b.add_css_class(if matches!(cmd, Command::PowerOff) { "danger" } else { "tonal" });
        let s = ctx.services.clone();
        b.connect_clicked(move |_| s.send(cmd.clone()));
        ui::row(&card, label, "", &b);
    }
    page.upcast()
}

pub fn privacy(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Privacy & Security", "Screen locking and session security.");
    let comp = ctx.compositor.get();
    let card = ui::card(&content, "Screen lock");
    let labels: Vec<&str> = ui::TIMEOUTS.iter().map(|t| t.1).collect();
    {
        let c = ctx.clone();
        ui::dropdown(&card, "Lock when inactive for", "", &labels, ui::timeout_index(comp.idle.lock_after), move |i| {
            c.compositor.update(|x| x.idle.lock_after = ui::TIMEOUTS[i].0);
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Lock before suspending", "", comp.idle.lock_on_suspend, move |v| {
            c.compositor.update(|x| x.idle.lock_on_suspend = v)
        });
    }
    let lock = gtk::Button::with_label("Lock now");
    lock.add_css_class("filled");
    lock.connect_clicked(|_| store::compositor(m3_protocol::Request::Action { action: m3_protocol::Action::Lock }));
    ui::row(&card, "Lock the screen", "Super+L", &lock);

    let card = ui::card(&content, "How m3 protects your session");
    for (t, s) in [
        (
            "Secure lock screen",
            "Uses the ext-session-lock protocol: nothing else is shown or receives input while locked, and the session stays locked even if the lock screen crashes.",
        ),
        ("Password checks", "Done by PAM (/etc/pam.d/m3-lock); your password is wiped from memory after each attempt."),
        (
            "Sandboxed apps",
            "Flatpak apps cannot read the clipboard history, inject input, list other windows or lock the session.",
        ),
        ("Clipboard", "X11 apps can only read the clipboard while focused."),
    ] {
        let l = gtk::Label::new(None);
        l.set_visible(false);
        ui::row(&card, t, s, &l);
    }
    ui::note(
        &content,
        "Screenshot tools and screen sharing use the wlroots screencopy protocol: install xdg-desktop-portal-wlr \
         for sharing in browsers and video calls. Sandboxed (Flatpak) apps can only capture through the portal.",
    );
    page.upcast()
}
