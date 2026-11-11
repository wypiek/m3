//! Lock screen: background, clock, user picture, message, media, battery.

use std::rc::Rc;

use gtk::{gio, prelude::*};
use m3_core::config::{
    AvatarShape, ClockFormat, DateFormat, LOCK_MESSAGE_MAX, LockBackground, LockClockPosition, LockClockSize,
    LockClockStyle,
};

use crate::{Ctx, ui};

/// Index of `v` in `all` (0 when absent).
fn index_of<T: PartialEq>(all: &[T], v: &T) -> usize {
    all.iter().position(|x| x == v).unwrap_or(0)
}

/// Short label for a chosen image path.
fn image_label(p: &std::path::Path) -> String {
    p.file_name().and_then(|n| n.to_str()).map(str::to_owned).unwrap_or_else(|| "Choose image…".into())
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Lock screen", "How the screen looks while it's locked. Unlocking is unaffected.");
    let l = ctx.lock.get();

    // Preview: saves pending changes first, then shows m3-lock in a window.
    let preview = gtk::Button::new();
    let pb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    pb.append(&ui::icon("visibility"));
    pb.append(&gtk::Label::new(Some("Preview")));
    preview.set_child(Some(&pb));
    preview.add_css_class("filled");
    preview.add_css_class("medium");
    preview.set_halign(gtk::Align::Start);
    {
        let c = ctx.clone();
        preview.connect_clicked(move |_| {
            c.lock.flush();
            if let Err(e) = std::process::Command::new("m3-lock").arg("--preview").spawn() {
                c.toast(&format!("Could not start the preview: {e}"));
            }
        });
    }
    content.append(&preview);

    // ---- background -------------------------------------------------------
    let card = ui::card(&content, "Background");
    let image_btn = gtk::Button::with_label(&image_label(&l.image));
    image_btn.add_css_class("tonal");
    let image_row_btn = image_btn.clone();
    let sources = [LockBackground::Wallpaper, LockBackground::Image, LockBackground::Color];
    {
        let c = ctx.clone();
        let ib = image_btn.clone();
        ui::segmented(
            &card,
            "Background",
            &["Wallpaper", "Image", "Colour"],
            index_of(&sources, &l.background),
            move |i| {
                let has_image = !c.lock.get().image.as_os_str().is_empty();
                if sources[i] == LockBackground::Image && !has_image {
                    // An image background needs an image: ask for one first.
                    ib.emit_clicked();
                    return;
                }
                c.lock.update(|x| x.background = sources[i]);
            },
        );
    }
    {
        let c = ctx.clone();
        let win = ctx.window.clone();
        image_btn.connect_clicked(move |b| {
            let filter = gtk::FileFilter::new();
            for t in ["image/png", "image/jpeg", "image/webp"] {
                filter.add_mime_type(t);
            }
            filter.set_name(Some("Images"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder().title("Lock screen image").filters(&filters).modal(true).build();
            let c = c.clone();
            let b = b.clone();
            dialog.open(Some(&win), gio::Cancellable::NONE, move |r| {
                if let Some(p) = r.ok().and_then(|f| f.path()) {
                    b.set_label(&image_label(&p));
                    c.lock.update(|x| {
                        x.image = p;
                        x.background = LockBackground::Image;
                    });
                }
            });
        });
    }
    ui::row(&card, "Image", "Used when the background is Image", &image_row_btn);
    {
        let c = ctx.clone();
        ui::slider(&card, "Blur", "Soften the background", (0.0, 64.0, 1.0), f64::from(l.blur), ui::plain, move |v| {
            c.lock.update(|x| x.blur = v.round() as u32);
        });
    }
    {
        let c = ctx.clone();
        ui::slider(&card, "Dim", "Darken the background", (0.0, 0.9, 0.01), l.dim, ui::pct, move |v| {
            c.lock.update(|x| x.dim = (v * 100.0).round() / 100.0);
        });
    }

    // ---- clock ------------------------------------------------------------
    let card = ui::card(&content, "Clock");
    {
        let formats = [ClockFormat::H24, ClockFormat::H12];
        let c = ctx.clone();
        ui::segmented(&card, "Format", &["24-hour", "12-hour"], index_of(&formats, &l.clock_format), move |i| {
            c.lock.update(|x| x.clock_format = formats[i]);
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Show seconds", "", l.clock_seconds, move |v| c.lock.update(|x| x.clock_seconds = v));
    }
    {
        let sizes = [LockClockSize::Small, LockClockSize::Medium, LockClockSize::Large];
        let c = ctx.clone();
        ui::segmented(&card, "Size", &["Small", "Medium", "Large"], index_of(&sizes, &l.clock_size), move |i| {
            c.lock.update(|x| x.clock_size = sizes[i]);
        });
    }
    {
        let styles = [LockClockStyle::Expressive, LockClockStyle::Light, LockClockStyle::Classic];
        let c = ctx.clone();
        ui::segmented(
            &card,
            "Style",
            &["Expressive", "Light", "Classic"],
            index_of(&styles, &l.clock_style),
            move |i| {
                c.lock.update(|x| x.clock_style = styles[i]);
            },
        );
    }
    {
        let positions = [LockClockPosition::Center, LockClockPosition::Top, LockClockPosition::Left];
        let c = ctx.clone();
        ui::segmented(
            &card,
            "Position",
            &["Centre", "Top", "Left"],
            index_of(&positions, &l.clock_position),
            move |i| c.lock.update(|x| x.clock_position = positions[i]),
        );
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Show the date", "", l.show_date, move |v| c.lock.update(|x| x.show_date = v));
    }
    {
        let formats = [DateFormat::Short, DateFormat::Long];
        let c = ctx.clone();
        ui::segmented(
            &card,
            "Date format",
            &["Tue 10 Nov", "Tuesday, 10 November"],
            index_of(&formats, &l.date_format),
            move |i| c.lock.update(|x| x.date_format = formats[i]),
        );
    }

    // ---- you --------------------------------------------------------------
    let card = ui::card(&content, "You");
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Show your picture and name",
            "Your picture is ~/.face, or your initial",
            l.show_user,
            move |v| c.lock.update(|x| x.show_user = v),
        );
    }
    {
        let labels: Vec<&str> = AvatarShape::ALL.iter().map(|s| s.label()).collect();
        let c = ctx.clone();
        ui::dropdown(&card, "Picture shape", "", &labels, index_of(&AvatarShape::ALL, &l.avatar_shape), move |i| {
            c.lock.update(|x| x.avatar_shape = AvatarShape::ALL[i]);
        });
    }
    let message = gtk::Entry::new();
    message.set_text(&l.message);
    message.set_max_length(LOCK_MESSAGE_MAX as i32);
    message.set_placeholder_text(Some("e.g. If found, please call…"));
    message.set_width_chars(28);
    {
        let save = {
            let c = ctx.clone();
            move |e: &gtk::Entry| {
                let text = e.text().to_string();
                if c.lock.get().message != text {
                    c.lock.update(|x| x.message = text);
                }
            }
        };
        let s = save.clone();
        message.connect_activate(move |e| s(e));
        let focus = gtk::EventControllerFocus::new();
        let e = message.clone();
        focus.connect_leave(move |_| save(&e));
        message.add_controller(focus);
    }
    ui::row(&card, "Message", "Shown under the password field", &message);

    // ---- extras -------------------------------------------------------------
    let card = ui::card(&content, "While locked");
    {
        let c = ctx.clone();
        ui::switch(&card, "Media controls", "Show what's playing, with play/pause and next", l.show_media, move |v| {
            c.lock.update(|x| x.show_media = v)
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Battery level", "On devices with a battery", l.show_battery, move |v| {
            c.lock.update(|x| x.show_battery = v)
        });
    }
    ui::note(&content, "Changes apply the next time the screen locks.");
    page.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(index_of(&[1, 2, 3], &3), 2);
        assert_eq!(index_of(&[1, 2, 3], &9), 0);
        assert_eq!(image_label(std::path::Path::new("/a/b/lock.png")), "lock.png");
        assert_eq!(image_label(std::path::Path::new("")), "Choose image…");
    }
}
