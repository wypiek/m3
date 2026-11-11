//! Desktop widgets: which are shown, the greeting name, shortcuts, the
//! countdown and the (opt-in) weather place.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use m3_core::config::{TemperatureUnit, WidgetSide};

use crate::{Ctx, ui};

/// An entry that saves on Enter and when it loses focus.
fn entry_row(
    list: &gtk::Box,
    title: &str,
    subtitle: &str,
    text: &str,
    placeholder: &str,
    max: i32,
    save: impl Fn(String) + 'static,
) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_text(text);
    e.set_max_length(max);
    e.set_placeholder_text(Some(placeholder));
    e.set_width_chars(22);
    let save = Rc::new(save);
    let s = save.clone();
    e.connect_activate(move |e| s(e.text().to_string()));
    let focus = gtk::EventControllerFocus::new();
    let e2 = e.clone();
    focus.connect_leave(move |_| save(e2.text().to_string()));
    e.add_controller(focus);
    ui::row(list, title, subtitle, &e);
    e
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Widgets", "Clock, greeting and glanceable widgets on the desktop.");
    let w = ctx.widgets.get();

    let card = ui::card(&content, "Desktop");
    {
        let c = ctx.clone();
        ui::switch(&card, "Show widgets", "", w.enabled, move |v| c.widgets.update(|x| x.enabled = v));
    }
    {
        let c = ctx.clone();
        let current = if w.side == WidgetSide::Left { 0 } else { 1 };
        ui::segmented(&card, "Widget column", &["Left", "Right"], current, move |i| {
            c.widgets.update(|x| x.side = if i == 0 { WidgetSide::Left } else { WidgetSide::Right })
        });
    }

    let card = ui::card(&content, "Clock and greeting");
    {
        let c = ctx.clone();
        ui::switch(&card, "Big clock", "", w.clock, move |v| c.widgets.update(|x| x.clock = v));
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Greeting", "“Good …” with the date and your name", w.greeting, move |v| {
            c.widgets.update(|x| x.greeting = v)
        });
    }
    {
        let c = ctx.clone();
        // Same fallback as the widget: first real name, else the login name.
        let real = glib::real_name().to_string_lossy().split_whitespace().next().unwrap_or_default().to_owned();
        let real =
            if real.is_empty() || real == "Unknown" { glib::user_name().to_string_lossy().into_owned() } else { real };
        entry_row(&card, "Name", "Leave empty to use your account name", &w.name, &real, 40, move |t| {
            let t = t.trim().to_owned();
            if c.widgets.get().name != t {
                c.widgets.update(|x| x.name = t);
            }
        });
    }

    let card = ui::card(&content, "Widgets");
    for (title, subtitle, get, set) in [
        (
            "Day",
            "Today's date on a flower",
            (|x: &m3_core::config::WidgetsConfig| x.day) as fn(&_) -> bool,
            (|x: &mut m3_core::config::WidgetsConfig, v| x.day = v) as fn(&mut _, bool),
        ),
        ("Shortcuts and search", "", |x| x.shortcuts, |x, v| x.shortcuts = v),
        ("Now playing", "Controls for any media player", |x| x.media, |x, v| x.media = v),
        ("Storage", "Size and free space of the system disk", |x| x.storage, |x, v| x.storage = v),
        ("RAM and CPU", "", |x| x.system, |x, v| x.system = v),
    ] {
        let c = ctx.clone();
        ui::switch(&card, title, subtitle, get(&w), move |v| c.widgets.update(|x| set(x, v)));
    }

    // Shortcut apps: three dropdowns over installed apps.
    let card = ui::card(&content, "Shortcut apps");
    let mut apps: Vec<(String, String)> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .filter_map(|a| a.id().map(|id| (id.to_string(), a.display_name().to_string())))
        .collect();
    apps.sort_by_key(|a| a.1.to_lowercase());
    let mut names: Vec<String> = vec!["None".into()];
    names.extend(apps.iter().map(|a| a.1.clone()));
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let apps = Rc::new(apps);
    for slot in 0..3 {
        let current =
            w.shortcut_apps.get(slot).and_then(|id| apps.iter().position(|a| &a.0 == id)).map_or(0, |i| i + 1);
        let c = ctx.clone();
        let apps = apps.clone();
        ui::dropdown(&card, &format!("Shortcut {}", slot + 1), "", &refs, current, move |i| {
            let id = i.checked_sub(1).and_then(|i| apps.get(i)).map(|a| a.0.clone());
            c.widgets.update(|x| {
                let mut list: Vec<Option<String>> = (0..3).map(|s| x.shortcut_apps.get(s).cloned()).collect();
                list[slot] = id.clone();
                x.shortcut_apps = list.into_iter().flatten().collect();
            });
        });
    }

    let card = ui::card(&content, "Countdown");
    {
        let c = ctx.clone();
        entry_row(
            &card,
            "Title",
            "Leave empty to hide the countdown",
            &w.countdown.title,
            "e.g. Summer holiday",
            60,
            move |t| {
                let t = t.trim().to_owned();
                if c.widgets.get().countdown.title != t {
                    c.widgets.update(|x| x.countdown.title = t);
                }
            },
        );
    }
    {
        let c = ctx.clone();
        let date = entry_row(&card, "Date", "YYYY-MM-DD", &w.countdown.date, "2027-07-01", 10, move |t| {
            let t = t.trim().to_owned();
            if m3_core::config::parse_date(&t).is_ok() && c.widgets.get().countdown.date != t {
                c.widgets.update(|x| x.countdown.date = t);
            }
        });
        date.connect_changed(|e| {
            if e.text().is_empty() || m3_core::config::parse_date(&e.text()).is_ok() {
                e.remove_css_class("error");
            } else {
                e.add_css_class("error");
            }
        });
    }

    weather_card(ctx, &content);
    page.upcast()
}

fn weather_card(ctx: &Rc<Ctx>, content: &gtk::Box) {
    let w = ctx.widgets.get().weather;
    let card = ui::card(content, "Weather");
    ui::note(
        content,
        "Weather comes from Open-Meteo (open-meteo.com). Only the coordinates of the place you choose are sent; \
         nothing is sent while weather is off.",
    );
    let place_label = gtk::Label::new(Some(if w.place.is_empty() { "Not set" } else { &w.place }));
    place_label.add_css_class("m3-secondary-text");
    place_label.set_wrap(true);
    place_label.set_max_width_chars(30);
    ui::row(&card, "Place", "", &place_label);
    let switch = {
        let c = ctx.clone();
        ui::switch(&card, "Show weather", "", w.enabled, move |v| c.widgets.update(|x| x.weather.enabled = v))
    };
    switch.set_sensitive(!w.place.is_empty());
    {
        let c = ctx.clone();
        let current = if w.unit == TemperatureUnit::Fahrenheit { 1 } else { 0 };
        ui::segmented(&card, "Units", &["°C", "°F"], current, move |i| {
            c.widgets.update(|x| {
                x.weather.unit = if i == 1 { TemperatureUnit::Fahrenheit } else { TemperatureUnit::Celsius }
            })
        });
    }

    // Search: type a city, pick one of the results.
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Find a city"));
    let results = gtk::Box::new(gtk::Orientation::Vertical, 2);
    results.add_css_class("m3-group");
    let status = gtk::Label::new(None);
    status.add_css_class("m3-secondary-text");
    let r = ui::row(&card, "Choose a place", "", &search);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.append(&status);
    col.append(&results);
    card.append(&col);
    let _ = r;
    let c = ctx.clone();
    search.connect_activate(move |e| {
        let q = e.text().to_string();
        if q.trim().is_empty() {
            return;
        }
        status.set_text("Searching…");
        while let Some(ch) = results.first_child() {
            results.remove(&ch);
        }
        let (status, results, c, place_label, switch) =
            (status.clone(), results.clone(), c.clone(), place_label.clone(), switch.clone());
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || m3_services::weather::search_places(&q)).await;
            match found {
                Ok(Ok(places)) if !places.is_empty() => {
                    status.set_text("");
                    for p in places {
                        let b = gtk::Button::with_label(&p.label());
                        b.add_css_class("flat");
                        let (c, place_label, switch, list, status) =
                            (c.clone(), place_label.clone(), switch.clone(), results.clone(), status.clone());
                        b.connect_clicked(move |_| {
                            let label = p.label();
                            c.widgets.update(|x| {
                                x.weather.place = label.clone();
                                x.weather.latitude = (p.latitude * 100.0).round() / 100.0;
                                x.weather.longitude = (p.longitude * 100.0).round() / 100.0;
                                x.weather.enabled = true;
                            });
                            place_label.set_text(&label);
                            switch.set_sensitive(true);
                            switch.set_active(true);
                            while let Some(ch) = list.first_child() {
                                list.remove(&ch);
                            }
                            status.set_text("");
                        });
                        results.append(&b);
                    }
                }
                Ok(Ok(_)) => status.set_text("No places found."),
                Ok(Err(e)) => status.set_text(&e),
                Err(_) => status.set_text("Search failed."),
            }
        });
    });
}
