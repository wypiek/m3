//! Input: keyboard, mouse, touchpad, accessibility.

use std::rc::Rc;

use gtk::prelude::*;
use m3_core::config::AccelProfile;

use crate::{Ctx, ui};

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Input", "Keyboard, mouse, touchpad and accessibility.");
    let c0 = ctx.compositor.get();

    let card = ui::card(&content, "Keyboard");
    for (title, sub, value, which) in [
        ("Layouts", "XKB layout codes, comma separated (e.g. us,de)", c0.input.keyboard.layout.clone(), 0),
        ("Variant", "Optional, e.g. dvorak", c0.input.keyboard.variant.clone(), 1),
        (
            "Options",
            "Optional XKB options, e.g. grp:alt_shift_toggle,caps:escape",
            c0.input.keyboard.options.clone(),
            2,
        ),
    ] {
        let e = gtk::Entry::new();
        e.set_text(&value);
        e.set_width_chars(18);
        let c = ctx.clone();
        e.connect_changed(move |e| {
            let v = e.text().trim().to_owned();
            // Basic shape check; the compositor falls back to "us" (and logs)
            // if XKB rejects a layout.
            let ok = v.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ',' | '_' | '-' | ':' | '(' | ')'));
            if !ok || (which == 0 && v.is_empty()) {
                e.add_css_class("error");
                return;
            }
            e.remove_css_class("error");
            c.compositor.update(|x| match which {
                0 => x.input.keyboard.layout = v,
                1 => x.input.keyboard.variant = v,
                _ => x.input.keyboard.options = v,
            });
        });
        ui::row(&card, title, sub, &e);
    }
    {
        let c = ctx.clone();
        ui::slider(
            &card,
            "Repeat delay",
            "Before a held key starts repeating",
            (150.0, 1000.0, 10.0),
            c0.input.keyboard.repeat_delay as f64,
            |v| format!("{v:.0} ms"),
            move |v| {
                c.compositor.update(|x| x.input.keyboard.repeat_delay = v as i32);
            },
        );
    }
    {
        let c = ctx.clone();
        ui::slider(
            &card,
            "Repeat rate",
            "Characters per second",
            (5.0, 80.0, 1.0),
            c0.input.keyboard.repeat_rate as f64,
            ui::plain,
            move |v| {
                c.compositor.update(|x| x.input.keyboard.repeat_rate = v as i32);
            },
        );
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Num Lock on at start", "", c0.input.keyboard.numlock, move |v| {
            c.compositor.update(|x| x.input.keyboard.numlock = v)
        });
    }
    let test = gtk::Entry::new();
    test.set_placeholder_text(Some("Type here to test"));
    ui::row(&card, "Try it", "", &test);

    let card = ui::card(&content, "Mouse");
    {
        let c = ctx.clone();
        ui::slider(
            &card,
            "Pointer speed",
            "",
            (-1.0, 1.0, 0.05),
            c0.input.pointer.accel_speed,
            |v| format!("{v:+.2}"),
            move |v| c.compositor.update(|x| x.input.pointer.accel_speed = v),
        );
    }
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Pointer acceleration",
            "Faster movements travel further",
            c0.input.pointer.accel_profile == AccelProfile::Adaptive,
            move |v| {
                c.compositor.update(|x| {
                    x.input.pointer.accel_profile = if v { AccelProfile::Adaptive } else { AccelProfile::Flat }
                });
            },
        );
    }
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Natural scrolling",
            "Content moves with the wheel",
            c0.input.pointer.natural_scroll,
            move |v| c.compositor.update(|x| x.input.pointer.natural_scroll = v),
        );
    }
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Left-handed",
            "Swap the primary and secondary buttons",
            c0.input.pointer.left_handed,
            move |v| c.compositor.update(|x| x.input.pointer.left_handed = v),
        );
    }

    let card = ui::card(&content, "Touchpad");
    let t = &c0.input.touchpad;
    {
        let c = ctx.clone();
        ui::switch(&card, "Tap to click", "", t.tap_to_click, move |v| {
            c.compositor.update(|x| x.input.touchpad.tap_to_click = v)
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Natural scrolling", "", t.natural_scroll, move |v| {
            c.compositor.update(|x| x.input.touchpad.natural_scroll = v)
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Disable while typing", "", t.disable_while_typing, move |v| {
            c.compositor.update(|x| x.input.touchpad.disable_while_typing = v)
        });
    }
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Two-finger secondary click",
            "Otherwise click the bottom-right corner",
            t.clickfinger,
            move |v| c.compositor.update(|x| x.input.touchpad.clickfinger = v),
        );
    }
    {
        let c = ctx.clone();
        ui::slider(
            &card,
            "Speed",
            "",
            (-1.0, 1.0, 0.05),
            t.accel_speed,
            |v| format!("{v:+.2}"),
            move |v| c.compositor.update(|x| x.input.touchpad.accel_speed = v),
        );
    }
    {
        let c = ctx.clone();
        ui::slider(&card, "Scrolling speed", "", (0.1, 4.0, 0.1), t.scroll_factor, ui::times, move |v| {
            c.compositor.update(|x| x.input.touchpad.scroll_factor = v)
        });
    }

    let th = ctx.theme.get();
    let card = ui::card(&content, "Accessibility");
    {
        let c = ctx.clone();
        ui::slider(&card, "Larger text", "", (0.8, 2.0, 0.05), th.text_scale, ui::pct, move |v| {
            c.theme.update(|t| t.text_scale = v);
            c.restyle();
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "High contrast", "Stronger colours and outlines", th.high_contrast, move |v| {
            c.theme.update(|t| t.high_contrast = v);
            c.restyle();
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Reduce motion", "", th.reduced_motion, move |v| {
            c.theme.update(|t| t.reduced_motion = v);
            c.restyle();
        });
    }
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Pointer size",
            "",
            &["Default", "Large", "Larger", "Largest"],
            [24, 32, 48, 64].iter().position(|s| *s == c0.general.cursor_size).unwrap_or(0),
            move |i| {
                c.compositor.update(|x| x.general.cursor_size = [24, 32, 48, 64][i]);
            },
        );
    }
    ui::note(
        &content,
        "Screen readers work through the AT-SPI accessibility bus; m3's own interfaces expose labels and roles to it.",
    );
    page.upcast()
}
