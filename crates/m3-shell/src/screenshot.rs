//! Screenshots: the region selector (Shift+Print, Super+Shift+S) and the
//! notification shown after every screenshot.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_protocol::Request;

use crate::ipc;

/// `~/Pictures/Screenshots/Screenshot-<date>.png` (XDG pictures directory).
pub fn new_path() -> Option<PathBuf> {
    let pictures =
        glib::user_special_dir(glib::UserDirectory::Pictures).unwrap_or_else(|| glib::home_dir().join("Pictures"));
    let dir = pictures.join("Screenshots");
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let mut p = dir.join(format!("Screenshot {stamp}.png"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("Screenshot {stamp} ({n}).png"));
        n += 1;
    }
    Some(p)
}

/// Normalise a drag from `a` to `b` into `[x, y, w, h]` (pure; tested).
pub fn drag_rect(a: (f64, f64), b: (f64, f64)) -> [i32; 4] {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    [x0.round() as i32, y0.round() as i32, (x1 - x0).round() as i32, (y1 - y0).round() as i32]
}

/// Show the region selector on the output under the pointer, then capture.
pub fn select_region(app: &gtk::Application) {
    let window = gtk::Window::builder().application(app).decorated(false).build();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("m3-screenshot"));
    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(e, true);
    }
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.add_css_class("m3-layer");
    window.add_css_class("m3-transparent");

    let start: Rc<Cell<Option<(f64, f64)>>> = Rc::default();
    let end: Rc<Cell<Option<(f64, f64)>>> = Rc::default();
    let area = gtk::DrawingArea::new();
    area.add_css_class("m3-screenshot-area");
    area.set_cursor_from_name(Some("crosshair"));
    {
        let (start, end) = (start.clone(), end.clone());
        area.set_draw_func(move |a, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            let sel = start.get().zip(end.get()).map(|(s, e)| drag_rect(s, e));
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.38);
            match sel {
                None => {
                    cr.rectangle(0.0, 0.0, w, h);
                    let _ = cr.fill();
                }
                Some([x, y, sw, sh]) => {
                    let (x, y, sw, sh) = (f64::from(x), f64::from(y), f64::from(sw), f64::from(sh));
                    // Dim everything but the selection.
                    cr.rectangle(0.0, 0.0, w, y);
                    cr.rectangle(0.0, y + sh, w, h - y - sh);
                    cr.rectangle(0.0, y, x, sh);
                    cr.rectangle(x + sw, y, w - x - sw, sh);
                    let _ = cr.fill();
                    let c = a.color();
                    cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), 1.0);
                    cr.set_line_width(2.0);
                    cr.rectangle(x + 1.0, y + 1.0, (sw - 2.0).max(0.0), (sh - 2.0).max(0.0));
                    let _ = cr.stroke();
                    let label = format!("{} × {}", sw as i32, sh as i32);
                    cr.set_font_size(13.0);
                    let ty = if y > 24.0 { y - 8.0 } else { y + sh + 18.0 };
                    cr.move_to(x, ty);
                    let _ = cr.show_text(&label);
                }
            }
        });
    }
    let hint = gtk::Label::new(Some("Drag to select  ·  Enter: whole screen  ·  Esc: cancel"));
    hint.add_css_class("m3-screenshot-hint");
    hint.set_halign(gtk::Align::Center);
    hint.set_valign(gtk::Align::Start);
    hint.set_margin_top(24);
    hint.set_can_target(false);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&hint);
    window.set_child(Some(&overlay));

    // Finish: close the overlay first so it isn't in the picture, then ask
    // the compositor for the capture.
    let finish = {
        let window = window.clone();
        Rc::new(move |region: Option<[i32; 4]>| {
            let output = window
                .surface()
                .and_then(|s| gdk::Display::default().and_then(|d| d.monitor_at_surface(&s)))
                .and_then(|m| m.connector())
                .map(|c| c.to_string());
            window.close();
            let Some(path) = new_path() else {
                tracing::warn!("no folder for screenshots");
                return;
            };
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                ipc::send(Request::Screenshot { output, path, region, notify: true });
            });
        })
    };

    let drag = gtk::GestureDrag::new();
    {
        let (start, end, area) = (start.clone(), end.clone(), area.clone());
        drag.connect_drag_begin(move |_, x, y| {
            start.set(Some((x, y)));
            end.set(Some((x, y)));
            area.queue_draw();
        });
    }
    {
        let (start, end, area) = (start.clone(), end.clone(), area.clone());
        drag.connect_drag_update(move |_, dx, dy| {
            if let Some((sx, sy)) = start.get() {
                end.set(Some((sx + dx, sy + dy)));
                area.queue_draw();
            }
        });
    }
    {
        let (start, end, finish) = (start.clone(), end.clone(), finish.clone());
        drag.connect_drag_end(move |_, _, _| {
            let rect = start.get().zip(end.get()).map(|(s, e)| drag_rect(s, e));
            match rect {
                // A click without dragging captures the whole output.
                Some([_, _, w, h]) if w < 4 || h < 4 => finish(None),
                Some(r) => finish(Some(r)),
                None => {}
            }
        });
    }
    area.add_controller(drag);

    let keys = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Escape => {
                window.close();
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                finish(None);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
    }
    window.add_controller(keys);
    window.present();
}

/// Open a screenshot, or show it in the file manager.
pub fn open(path: &Path, in_folder: bool) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
    let path = path.to_owned();
    let done = move |r: Result<(), glib::Error>| {
        if let Err(e) = r {
            tracing::warn!("cannot open {}: {e}", path.display());
        }
    };
    if in_folder {
        launcher.open_containing_folder(None::<&gtk::Window>, gio::Cancellable::NONE, done);
    } else {
        launcher.launch(None::<&gtk::Window>, gio::Cancellable::NONE, done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags_in_any_direction() {
        assert_eq!(drag_rect((10.0, 20.0), (110.0, 70.0)), [10, 20, 100, 50]);
        assert_eq!(drag_rect((110.0, 70.0), (10.0, 20.0)), [10, 20, 100, 50]);
        assert_eq!(drag_rect((5.0, 5.0), (5.0, 5.0)), [5, 5, 0, 0]);
    }
}
