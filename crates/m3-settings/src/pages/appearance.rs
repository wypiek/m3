//! Appearance: theme, colours, shape, motion, text, icons, wallpaper.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gdk, gio, glib, prelude::*};
use m3_core::config::{ColorSource, PRESETS, SchemeVariant, ThemeMode, WallpaperFit, parse_hex_color};

use m3_widgets::{Shape, ShapeView};

use crate::{Ctx, ui};

/// A colour swatch drawn as an expressive shape.
fn swatch(color: u32, size: i32, shape: Shape) -> ShapeView {
    let v = ShapeView::new(shape, size);
    v.set_fill(color);
    v
}

fn preview_row(ctx: &Rc<Ctx>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.add_css_class("m3-scheme-preview");
    b.set_halign(gtk::Align::Start);
    rebuild_preview(ctx, &b);
    b
}

fn rebuild_preview(ctx: &Rc<Ctx>, b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
    let t = ctx.theme_preview();
    let c = &t.colors;
    for (name, col, shape) in [
        ("Primary", c.primary, Shape::Cookie(9)),
        ("Secondary", c.secondary, Shape::Clover(4)),
        ("Tertiary", c.tertiary, Shape::Sunny(8)),
        ("Primary container", c.primary_container, Shape::Square),
        ("Surface", c.surface_container_highest, Shape::Circle),
        ("Error", c.error, Shape::Pentagon),
    ] {
        let s = swatch(col.to_u32(), 40, shape);
        s.widget().set_tooltip_text(Some(&format!("{name} {}", col.hex())));
        b.append(s.widget());
    }
}

fn icon_themes() -> Vec<String> {
    themes_with(|p| {
        p.join("index.theme").is_file() && !p.join("cursors").is_dir()
            || p.join("index.theme").is_file() && p.join("scalable").is_dir()
    })
}

fn cursor_themes() -> Vec<String> {
    themes_with(|p| p.join("cursors").is_dir())
}

fn themes_with(pred: impl Fn(&Path) -> bool) -> Vec<String> {
    let mut dirs: Vec<PathBuf> = m3_core::paths::system_data_dirs().into_iter().map(|d| d.join("icons")).collect();
    if let Some(d) = m3_core::paths::data_dir().and_then(|d| d.parent().map(|p| p.join("icons"))) {
        dirs.push(d);
    }
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join(".icons"));
    }
    let mut out: Vec<String> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter(|e| pred(&e.path()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "default" && n != "hicolor")
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Images offered as wallpapers: ~/Pictures/Wallpapers, ~/Pictures and the
/// system backgrounds directory.
fn wallpaper_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        let h = PathBuf::from(h);
        dirs.push(h.join("Pictures/Wallpapers"));
        dirs.push(h.join("Pictures"));
    }
    for d in m3_core::paths::system_data_dirs() {
        dirs.push(d.join("backgrounds"));
        dirs.push(d.join("m3/wallpapers"));
    }
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|e| e.to_str()).map(str::to_lowercase);
            if matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "webp")) {
                out.push(p);
            } else if p.is_dir() {
                // One level deeper (e.g. /usr/share/backgrounds/gnome/).
                if let Ok(rd2) = std::fs::read_dir(&p) {
                    for e2 in rd2.flatten() {
                        let p2 = e2.path();
                        let ext = p2.extension().and_then(|e| e.to_str()).map(str::to_lowercase);
                        if matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "webp")) {
                            out.push(p2);
                        }
                    }
                }
            }
            if out.len() > 48 {
                return out;
            }
        }
    }
    out
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) =
        ui::page("Appearance", "Make m3 yours: colours come from your wallpaper or a colour you pick.");
    let t = ctx.theme.get();
    if let Some(e) = &ctx.theme.load_error {
        ui::note(&content, &format!("theme.toml has a problem and defaults are shown: {e}"));
    }

    // ---- style
    let card = ui::card(&content, "Style");
    let modes = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::Auto];
    let sel = modes.iter().position(|m| *m == t.mode).unwrap_or(1);
    let schedule = gtk::Box::new(gtk::Orientation::Vertical, 0);
    {
        let c = ctx.clone();
        let sched = schedule.clone();
        ui::segmented(&card, "Theme", &["Light", "Dark", "Automatic"], sel, move |i| {
            c.theme.update(|t| t.mode = modes[i]);
            sched.set_visible(i == 2);
            c.restyle();
        });
    }
    schedule.set_visible(t.mode == ThemeMode::Auto);
    for (label, get_from) in [("Dark from", true), ("Light from", false)] {
        let e = gtk::Entry::new();
        e.set_text(if get_from { &t.dark_from } else { &t.dark_until });
        e.set_width_chars(6);
        e.set_max_width_chars(6);
        let c = ctx.clone();
        e.connect_changed(move |e| {
            let v = e.text().to_string();
            if m3_core::config::parse_hhmm(&v).is_ok() {
                e.remove_css_class("error");
                c.theme.update(|t| if get_from { t.dark_from = v } else { t.dark_until = v });
            } else {
                e.add_css_class("error");
            }
        });
        ui::row(&schedule, label, "24-hour time, e.g. 19:30", &e);
    }
    card.append(&schedule);

    // ---- colours
    let card = ui::card(&content, "Colours");
    let preview = preview_row(ctx);
    ui::row(&card, "Colour scheme", "Live preview of key colour roles", &preview);
    let refresh: Rc<dyn Fn()> = {
        let c = ctx.clone();
        let p = preview.clone();
        Rc::new(move || {
            c.restyle();
            rebuild_preview(&c, &p);
        })
    };
    let sources = [ColorSource::Wallpaper, ColorSource::Preset, ColorSource::Seed];
    let presets_row = gtk::FlowBox::new();
    presets_row.set_selection_mode(gtk::SelectionMode::None);
    presets_row.set_max_children_per_line(8);
    presets_row.set_min_children_per_line(4);
    for (id, label, hex) in PRESETS {
        let b = gtk::Button::new();
        b.add_css_class("m3-swatch-button");
        let sw = swatch(parse_hex_color(hex).unwrap_or(0), 40, Shape::Circle);
        m3_widgets::morph_on_interaction(&b, &sw, Shape::Circle, Shape::Cookie(9), Shape::Clover(4));
        b.set_child(Some(sw.widget()));
        b.set_tooltip_text(Some(label));
        b.update_property(&[gtk::accessible::Property::Label(label)]);
        let c = ctx.clone();
        let r = refresh.clone();
        let id = id.to_string();
        b.connect_clicked(move |_| {
            c.theme.update(|t| {
                t.preset = id.clone();
                t.source = ColorSource::Preset;
            });
            r();
        });
        presets_row.append(&b);
    }
    let color_btn = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
    if let Ok(v) = parse_hex_color(&t.seed_color) {
        color_btn.set_rgba(&gdk::RGBA::new(
            ((v >> 16) & 0xff) as f32 / 255.0,
            ((v >> 8) & 0xff) as f32 / 255.0,
            (v & 0xff) as f32 / 255.0,
            1.0,
        ));
    }
    {
        let c = ctx.clone();
        let r = refresh.clone();
        color_btn.connect_rgba_notify(move |b| {
            let rgba = b.rgba();
            let hex = format!(
                "#{:02x}{:02x}{:02x}",
                (rgba.red() * 255.0).round() as u8,
                (rgba.green() * 255.0).round() as u8,
                (rgba.blue() * 255.0).round() as u8
            );
            c.theme.update(|t| {
                t.seed_color = hex;
                t.source = ColorSource::Seed;
            });
            r();
        });
    }
    {
        let c = ctx.clone();
        let r = refresh.clone();
        ui::segmented(
            &card,
            "Colour source",
            &["Wallpaper", "Preset", "Custom"],
            sources.iter().position(|s| *s == t.source).unwrap_or(0),
            move |i| {
                c.theme.update(|t| t.source = sources[i]);
                r();
            },
        );
    }
    ui::row(&card, "Presets", "", &presets_row);
    ui::row(&card, "Custom colour", "Used when the source is Custom", &color_btn);
    let variants: Vec<&str> = SchemeVariant::ALL.iter().map(|v| v.label()).collect();
    {
        let c = ctx.clone();
        let r = refresh.clone();
        ui::dropdown(
            &card,
            "Style variant",
            "How colourful the scheme is",
            &variants,
            SchemeVariant::ALL.iter().position(|v| *v == t.variant).unwrap_or(0),
            move |i| {
                c.theme.update(|t| t.variant = SchemeVariant::ALL[i]);
                r();
            },
        );
    }
    for (title, sub, range, value, which) in [
        ("Accent intensity", "Saturation of accent colours", (0.0, 2.0, 0.05), t.accent_intensity, 0),
        ("Surface tint", "How much colour surfaces pick up", (0.0, 3.0, 0.1), t.surface_tint, 1),
        ("Contrast", "Higher contrast improves legibility", (-1.0, 1.0, 0.1), t.contrast, 2),
    ] {
        let c = ctx.clone();
        let r = refresh.clone();
        ui::slider(
            &card,
            title,
            sub,
            range,
            value,
            if which == 2 { |v| format!("{v:+.1}") } else { ui::times },
            move |v| {
                c.theme.update(|t| match which {
                    0 => t.accent_intensity = v,
                    1 => t.surface_tint = v,
                    _ => t.contrast = v,
                });
                r();
            },
        );
    }

    // ---- shape & motion
    let card = ui::card(&content, "Shape & motion");
    {
        let c = ctx.clone();
        let r = refresh.clone();
        ui::slider(
            &card,
            "Corner roundness",
            "From square to extra round",
            (0.0, 2.0, 0.1),
            t.corner_scale,
            ui::times,
            move |v| {
                c.theme.update(|t| t.corner_scale = v);
                r();
            },
        );
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Translucent panels", "Slightly see-through panel and menus", t.transparency, move |v| {
            c.theme.update(|t| t.transparency = v)
        });
    }
    {
        let c = ctx.clone();
        let r = refresh.clone();
        ui::switch(&card, "Reduce motion", "Turn off animations", t.reduced_motion, move |v| {
            c.theme.update(|t| t.reduced_motion = v);
            r();
        });
    }

    // ---- text
    let card = ui::card(&content, "Text");
    for (title, mono) in [("Interface font", false), ("Monospace font", true)] {
        let b = gtk::FontDialogButton::new(Some(gtk::FontDialog::new()));
        b.set_level(gtk::FontLevel::Family);
        let fam = if mono { t.monospace_font_family.clone() } else { t.font_family.clone() };
        b.set_font_desc(&gtk::pango::FontDescription::from_string(&fam));
        let c = ctx.clone();
        let r = refresh.clone();
        b.connect_font_desc_notify(move |b| {
            if let Some(f) = b.font_desc().and_then(|d| d.family()) {
                let f = f.to_string();
                c.theme.update(|t| if mono { t.monospace_font_family = f } else { t.font_family = f });
                r();
            }
        });
        ui::row(&card, title, "", &b);
    }
    {
        let c = ctx.clone();
        let r = refresh.clone();
        ui::slider(
            &card,
            "Text size",
            "Larger text everywhere in m3",
            (0.8, 2.0, 0.05),
            t.text_scale,
            ui::pct,
            move |v| {
                c.theme.update(|t| t.text_scale = v);
                r();
            },
        );
    }

    // ---- icons & cursor
    let card = ui::card(&content, "Icons & cursor");
    let icons = icon_themes();
    let icon_refs: Vec<&str> = icons.iter().map(String::as_str).collect();
    if !icons.is_empty() {
        let c = ctx.clone();
        let list = icons.clone();
        ui::dropdown(
            &card,
            "Icon theme",
            "",
            &icon_refs,
            icons.iter().position(|i| *i == t.icon_theme).unwrap_or(0),
            move |i| {
                let name = list[i].clone();
                c.theme.update(|t| t.icon_theme = name);
            },
        );
    }
    let cursors = cursor_themes();
    let comp = ctx.compositor.get();
    if !cursors.is_empty() {
        let refs: Vec<&str> = cursors.iter().map(String::as_str).collect();
        let c = ctx.clone();
        let list = cursors.clone();
        ui::dropdown(
            &card,
            "Cursor theme",
            "",
            &refs,
            cursors.iter().position(|i| *i == comp.general.cursor_theme).unwrap_or(0),
            move |i| {
                let name = list[i].clone();
                c.compositor.update(|x| x.general.cursor_theme = name);
            },
        );
    }
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Cursor size",
            "",
            &["Default (24)", "Large (32)", "Larger (48)", "Largest (64)"],
            [24, 32, 48, 64].iter().position(|s| *s == comp.general.cursor_size).unwrap_or(0),
            move |i| {
                c.compositor.update(|x| x.general.cursor_size = [24, 32, 48, 64][i]);
            },
        );
    }

    // ---- wallpaper
    let card = ui::card(&content, "Wallpaper");
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_max_children_per_line(4);
    grid.set_min_children_per_line(3);
    grid.set_column_spacing(8);
    grid.set_row_spacing(8);
    grid.add_css_class("m3-wallpaper-grid");
    let set_wallpaper: Rc<dyn Fn(Option<PathBuf>)> = {
        let c = ctx.clone();
        let r = refresh.clone();
        Rc::new(move |p: Option<PathBuf>| {
            c.theme.update(|t| t.wallpaper.path = p.clone().unwrap_or_default());
            r();
        })
    };
    let default_tile = gtk::Button::new();
    default_tile.add_css_class("m3-wallpaper-tile");
    let dl = gtk::Box::new(gtk::Orientation::Vertical, 4);
    dl.append(&ui::icon("auto_awesome"));
    dl.append(&gtk::Label::new(Some("m3 default")));
    default_tile.set_child(Some(&dl));
    {
        let s = set_wallpaper.clone();
        default_tile.connect_clicked(move |_| s(None));
    }
    grid.append(&default_tile);
    for p in wallpaper_candidates() {
        let b = gtk::Button::new();
        b.add_css_class("m3-wallpaper-tile");
        let pic = gtk::Picture::new();
        pic.set_content_fit(gtk::ContentFit::Cover);
        pic.set_can_shrink(true);
        pic.set_size_request(160, 100);
        // Thumbnail decoding off the main thread.
        let path = p.clone();
        let pic2 = pic.clone();
        glib::spawn_future_local(async move {
            let r = gio::spawn_blocking(move || {
                gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&path, 320, 200, true)
                    .ok()
                    .map(|pb| (pb.width(), pb.height(), pb.rowstride() as usize, pb.has_alpha(), pb.read_pixel_bytes()))
            })
            .await;
            if let Ok(Some((w, h, stride, alpha, bytes))) = r {
                let fmt = if alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
                pic2.set_paintable(Some(&gdk::MemoryTexture::new(w, h, fmt, &bytes, stride)));
            }
        });
        b.set_child(Some(&pic));
        b.set_tooltip_text(p.file_name().and_then(|n| n.to_str()));
        let s = set_wallpaper.clone();
        b.connect_clicked(move |_| s(Some(p.clone())));
        grid.append(&b);
    }
    card.append(&grid);
    let choose = gtk::Button::with_label("Choose image…");
    choose.add_css_class("tonal");
    {
        let s = set_wallpaper.clone();
        let win = ctx.window.clone();
        choose.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            for t in ["image/png", "image/jpeg", "image/webp"] {
                filter.add_mime_type(t);
            }
            filter.set_name(Some("Images"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder().title("Choose a wallpaper").filters(&filters).modal(true).build();
            let s = s.clone();
            dialog.open(Some(&win), gio::Cancellable::NONE, move |r| {
                if let Ok(f) = r {
                    if let Some(p) = f.path() {
                        s(Some(p));
                    }
                }
            });
        });
    }
    ui::row(&card, "Your own picture", "Any PNG, JPEG or WebP image", &choose);
    let fits = [WallpaperFit::Fill, WallpaperFit::Fit, WallpaperFit::Center, WallpaperFit::Stretch];
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Position",
            "",
            &["Fill screen", "Fit inside", "Centre", "Stretch"],
            fits.iter().position(|f| *f == t.wallpaper.fit).unwrap_or(0),
            move |i| {
                c.theme.update(|t| t.wallpaper.fit = fits[i]);
            },
        );
    }
    let folder = gtk::Button::with_label(
        if t.wallpaper.slideshow_dir.as_os_str().is_empty() {
            "Off".to_owned()
        } else {
            t.wallpaper.slideshow_dir.display().to_string()
        }
        .as_str(),
    );
    {
        let c = ctx.clone();
        let win = ctx.window.clone();
        folder.connect_clicked(move |b| {
            let dialog = gtk::FileDialog::builder().title("Slideshow folder").modal(true).build();
            let c = c.clone();
            let b = b.clone();
            dialog.select_folder(Some(&win), gio::Cancellable::NONE, move |r| {
                if let Ok(f) = r {
                    if let Some(p) = f.path() {
                        b.set_label(&p.display().to_string());
                        c.theme.update(|t| t.wallpaper.slideshow_dir = p);
                    }
                }
            });
        });
    }
    ui::row(&card, "Slideshow", "Rotate through the images in a folder", &folder);
    {
        let c = ctx.clone();
        ui::spin(
            &card,
            "Change every (minutes)",
            "",
            (1.0, 1440.0, 1.0),
            t.wallpaper.slideshow_minutes as f64,
            move |v| {
                c.theme.update(|t| t.wallpaper.slideshow_minutes = v as u32);
            },
        );
    }
    let stop = gtk::Button::with_label("Stop slideshow");
    stop.add_css_class("text");
    {
        let c = ctx.clone();
        let folder = folder.clone();
        stop.connect_clicked(move |_| {
            folder.set_label("Off");
            c.theme.update(|t| t.wallpaper.slideshow_dir = PathBuf::new());
        });
    }
    ui::row(&card, "", "", &stop);
    page.upcast()
}
