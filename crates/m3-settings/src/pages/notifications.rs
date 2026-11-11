//! Notifications: popups, Do Not Disturb (now and scheduled), per-app.

use std::collections::BTreeSet;
use std::rc::Rc;

use gtk::prelude::*;
use m3_services::Command;

use crate::{Ctx, ui};

/// Apps that have sent notifications (from the shell's history).
fn known_apps() -> BTreeSet<String> {
    let path = m3_core::paths::state_dir().map(|d| d.join("notifications.json"));
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else { return BTreeSet::new() };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|n| n.get("app_name").and_then(|s| s.as_str()))
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Notifications", "Choose what can interrupt you.");
    let n = ctx.notifications.get();
    let card = ui::card(&content, "General");
    let dnd = gtk::Switch::new();
    ui::row(&card, "Do Not Disturb", "Only critical notifications pop up", &dnd);
    {
        let s = ctx.services.clone();
        dnd.connect_state_set(move |_, on| {
            s.send(Command::SetDoNotDisturb(on));
            gtk::glib::Propagation::Proceed
        });
    }
    ctx.on_sys(move |sys| {
        if dnd.is_active() != sys.dnd {
            dnd.set_state(sys.dnd);
        }
    });
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Show popups",
            "Otherwise notifications only go to the notification centre",
            n.popups,
            move |v| c.notifications.update(|x| x.popups = v),
        );
    }
    let card = ui::card(&content, "Schedule");
    let times = gtk::Box::new(gtk::Orientation::Vertical, 0);
    {
        let c = ctx.clone();
        let t = times.clone();
        ui::switch(&card, "Do Not Disturb on a schedule", "", n.dnd_schedule, move |v| {
            c.notifications.update(|x| x.dnd_schedule = v);
            t.set_visible(v);
        });
    }
    times.set_visible(n.dnd_schedule);
    for (label, from) in [("From", true), ("Until", false)] {
        let e = gtk::Entry::new();
        e.set_text(if from { &n.dnd_from } else { &n.dnd_until });
        e.set_width_chars(6);
        let c = ctx.clone();
        e.connect_changed(move |e| {
            let v = e.text().to_string();
            if m3_core::config::parse_hhmm(&v).is_ok() {
                e.remove_css_class("error");
                c.notifications.update(|x| if from { x.dnd_from = v } else { x.dnd_until = v });
            } else {
                e.add_css_class("error");
            }
        });
        ui::row(&times, label, "24-hour time", &e);
    }
    card.append(&times);

    let card = ui::card(&content, "Applications");
    let mut apps = known_apps();
    apps.extend(n.apps.keys().cloned());
    if apps.is_empty() {
        ui::note(&card, "Apps appear here after they send their first notification.");
    }
    for app in apps {
        let prefs = n.app(&app);
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let enabled = gtk::Switch::new();
        enabled.set_active(prefs.enabled);
        enabled.set_tooltip_text(Some("Allow notifications"));
        let popups = gtk::CheckButton::with_label("Popups");
        popups.set_active(prefs.popups);
        popups.set_sensitive(prefs.enabled);
        b.append(&popups);
        b.append(&enabled);
        {
            let (c, app, p) = (ctx.clone(), app.clone(), popups.clone());
            enabled.connect_active_notify(move |s| {
                p.set_sensitive(s.is_active());
                c.notifications.update(|x| x.apps.entry(app.clone()).or_default().enabled = s.is_active());
            });
        }
        {
            let (c, app) = (ctx.clone(), app.clone());
            popups.connect_toggled(move |s| {
                c.notifications.update(|x| x.apps.entry(app.clone()).or_default().popups = s.is_active())
            });
        }
        ui::row(&card, &app, "", &b);
    }
    page.upcast()
}
