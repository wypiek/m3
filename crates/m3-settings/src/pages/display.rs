//! Display: per-output mode, scale, rotation, position; brightness.

use std::rc::Rc;

use gtk::prelude::*;
use m3_protocol::{OutputChange, OutputInfo, Request, Response, Transform};

use crate::{Ctx, store, ui};

const SCALES: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];
const ROTATIONS: &[(Transform, &str)] = &[
    (Transform::Normal, "Normal"),
    (Transform::Rotate90, "90°"),
    (Transform::Rotate180, "180°"),
    (Transform::Rotate270, "270°"),
];

fn persist(ctx: &Rc<Ctx>, name: &str, f: impl FnOnce(&mut m3_core::config::OutputConfig)) {
    let name = name.to_owned();
    // Persist without asking the compositor to reload: it already applied it.
    let mut c = ctx.compositor.get();
    let entry = c.outputs.entry(name).or_default();
    f(entry);
    let outputs = c.outputs;
    ctx.compositor.update(move |x| x.outputs = outputs);
}

fn output_card(ctx: &Rc<Ctx>, content: &gtk::Box, o: &OutputInfo) {
    let card = ui::card(content, &format!("{} — {} {}", o.name, o.make, o.model));
    // Resolutions (unique sizes), then refresh rates for the chosen size.
    let mut sizes: Vec<(i32, i32)> = o.modes.iter().map(|m| (m.width, m.height)).collect();
    sizes.sort_by_key(|s| std::cmp::Reverse(s.0 * s.1));
    sizes.dedup();
    let current = o.current_mode;
    if sizes.len() > 1 {
        let labels: Vec<String> = sizes.iter().map(|(w, h)| format!("{w} × {h}")).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let sel = current.and_then(|c| sizes.iter().position(|s| *s == (c.width, c.height))).unwrap_or(0);
        let name = o.name.clone();
        let c = ctx.clone();
        let sizes2 = sizes.clone();
        ui::dropdown(&card, "Resolution", "", &refs, sel, move |i| {
            let (w, h) = sizes2[i];
            store::compositor(Request::ConfigureOutput {
                output: name.clone(),
                change: OutputChange::Mode { width: w, height: h, refresh_mhz: None },
            });
            persist(&c, &name, |e| e.mode = Some(format!("{w}x{h}")));
        });
        if let Some(cur) = current {
            let mut rates: Vec<i32> = o
                .modes
                .iter()
                .filter(|m| m.width == cur.width && m.height == cur.height)
                .map(|m| m.refresh_mhz)
                .collect();
            rates.sort_unstable_by(|a, b| b.cmp(a));
            rates.dedup();
            if rates.len() > 1 {
                let labels: Vec<String> = rates.iter().map(|r| format!("{:.2} Hz", *r as f64 / 1000.0)).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let sel = rates.iter().position(|r| *r == cur.refresh_mhz).unwrap_or(0);
                let name = o.name.clone();
                let c = ctx.clone();
                ui::dropdown(&card, "Refresh rate", "", &refs, sel, move |i| {
                    let r = rates[i];
                    store::compositor(Request::ConfigureOutput {
                        output: name.clone(),
                        change: OutputChange::Mode { width: cur.width, height: cur.height, refresh_mhz: Some(r) },
                    });
                    persist(&c, &name, |e| {
                        e.mode = Some(format!("{}x{}@{:.3}", cur.width, cur.height, r as f64 / 1000.0))
                    });
                });
            }
        }
    } else if let Some(c) = current {
        ui::info(&card, "Resolution", &format!("{} × {} @ {:.2} Hz", c.width, c.height, c.refresh_mhz as f64 / 1000.0));
    }
    let labels: Vec<String> = SCALES.iter().map(|s| format!("{:.0}%", s * 100.0)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let sel = SCALES.iter().position(|s| (s - o.scale).abs() < 0.01).unwrap_or(0);
    {
        let name = o.name.clone();
        let c = ctx.clone();
        ui::dropdown(&card, "Scale", "Size of everything on this display", &refs, sel, move |i| {
            store::compositor(Request::ConfigureOutput {
                output: name.clone(),
                change: OutputChange::Scale { scale: SCALES[i] },
            });
            persist(&c, &name, |e| e.scale = Some(SCALES[i]));
        });
    }
    let rot: Vec<&str> = ROTATIONS.iter().map(|r| r.1).collect();
    {
        let name = o.name.clone();
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Rotation",
            "",
            &rot,
            ROTATIONS.iter().position(|r| r.0 == o.transform).unwrap_or(0),
            move |i| {
                store::compositor(Request::ConfigureOutput {
                    output: name.clone(),
                    change: OutputChange::Transform { transform: ROTATIONS[i].0 },
                });
                persist(&c, &name, |e| e.transform = ROTATIONS[i].0);
            },
        );
    }
    // Arrangement: logical position of the top-left corner.
    let pos = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let x = gtk::SpinButton::with_range(-16384.0, 16384.0, 1.0);
    let y = gtk::SpinButton::with_range(-16384.0, 16384.0, 1.0);
    x.set_value(o.logical.x as f64);
    y.set_value(o.logical.y as f64);
    let apply = gtk::Button::with_label("Move");
    apply.add_css_class("tonal");
    pos.append(&x);
    pos.append(&y);
    pos.append(&apply);
    {
        let name = o.name.clone();
        let c = ctx.clone();
        apply.connect_clicked(move |_| {
            let (px, py) = (x.value() as i32, y.value() as i32);
            store::compositor(Request::ConfigureOutput {
                output: name.clone(),
                change: OutputChange::Position { x: px, y: py },
            });
            persist(&c, &name, |e| e.position = Some([px, py]));
        });
    }
    ui::row(&card, "Position", "Logical pixels (x, y) of the top-left corner", &pos);
    let primary = ctx.compositor.get().outputs.get(&o.name).is_some_and(|e| e.primary);
    {
        let name = o.name.clone();
        let c = ctx.clone();
        ui::switch(&card, "Primary display", "Shows the panel when it is set to one display", primary, move |v| {
            let mut comp = c.compositor.get();
            for (k, e) in comp.outputs.iter_mut() {
                e.primary = v && *k == name;
            }
            comp.outputs.entry(name.clone()).or_default().primary = v;
            let outputs = comp.outputs;
            c.compositor.update(move |x| x.outputs = outputs);
        });
    }
}

/// Half-hour steps for the night-light schedule, `("HH:MM", "HH:MM")`.
fn half_hours() -> Vec<String> {
    (0..48).map(|i| format!("{:02}:{:02}", i / 2, (i % 2) * 30)).collect()
}

fn night_light_card(ctx: &Rc<Ctx>, content: &gtk::Box) {
    use m3_core::config::NightLightMode;
    let nl = ctx.compositor.get().night_light;
    let card = ui::card(content, "Night light");
    ui::note(
        content,
        "Warmer colours in the evening are easier on the eyes. Needs a real display (not available in nested sessions).",
    );
    let modes = [NightLightMode::Off, NightLightMode::Scheduled, NightLightMode::Always];
    let times = half_hours();
    let time_refs: Vec<&str> = times.iter().map(String::as_str).collect();
    let index_of = |t: &str| times.iter().position(|x| x == t).unwrap_or(0);
    let from = {
        let c = ctx.clone();
        let times = times.clone();
        ui::dropdown(&card, "Turns on at", "", &time_refs, index_of(&nl.from), move |i| {
            let t = times[i].clone();
            c.compositor.update(|x| x.night_light.from = t);
        })
    };
    let until = {
        let c = ctx.clone();
        let times = times.clone();
        ui::dropdown(&card, "Turns off at", "", &time_refs, index_of(&nl.until), move |i| {
            let t = times[i].clone();
            c.compositor.update(|x| x.night_light.until = t);
        })
    };
    let schedule_sensitive = {
        let (from, until) = (from.clone(), until.clone());
        move |m: NightLightMode| {
            let on = m == NightLightMode::Scheduled;
            for d in [&from, &until] {
                if let Some(row) = d.parent() {
                    row.set_sensitive(on);
                }
            }
        }
    };
    schedule_sensitive(nl.mode);
    {
        let c = ctx.clone();
        let current = modes.iter().position(|m| *m == nl.mode).unwrap_or(0);
        let buttons = ui::segmented(&card, "Night light", &["Off", "Scheduled", "Always on"], current, move |i| {
            c.compositor.update(|x| x.night_light.mode = modes[i]);
            schedule_sensitive(modes[i]);
        });
        // Mode first, then the schedule.
        if let Some(row) = buttons.first().and_then(|b| b.parent()).and_then(|b| b.parent()) {
            card.reorder_child_after(&row, None::<&gtk::Widget>);
        }
    }
    {
        let c = ctx.clone();
        ui::slider(
            &card,
            "Warmth",
            "Colour temperature while night light is on",
            (1500.0, 6000.0, 100.0),
            f64::from(nl.temperature),
            |v| format!("{v:.0} K"),
            move |v| c.compositor.update(|x| x.night_light.temperature = v.round() as u32),
        );
    }
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Display", "Resolution, scale and layout of your screens.");
    match store::query(Request::Outputs) {
        Some(Response::Outputs { outputs }) if !outputs.is_empty() => {
            for o in &outputs {
                output_card(ctx, &content, o);
            }
        }
        _ => ui::note(&content, "The m3 compositor is not reachable, so displays cannot be configured right now."),
    }
    night_light_card(ctx, &content);
    let card = ui::card(&content, "Brightness");
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.01, 1.0, 0.01);
    scale.set_draw_value(false);
    scale.set_size_request(260, -1);
    let r = ui::row(&card, "Screen brightness", "", &scale);
    {
        let s = ctx.services.clone();
        scale.connect_change_value(move |_, _, v| {
            s.send(m3_services::Command::SetBrightness(v));
            gtk::glib::Propagation::Proceed
        });
    }
    let card2 = card.clone();
    ctx.on_sys(move |sys| {
        r.set_visible(sys.brightness.is_some());
        card2.set_visible(sys.brightness.is_some());
        if let Some(b) = sys.brightness {
            scale.set_value(b);
        }
    });
    page.upcast()
}
