//! The window menu (right-click a window's title bar): minimize,
//! maximize/restore, fullscreen, move to another workspace, close.

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_protocol::{Action, Request, WorkspaceInfo};

use crate::{icons, ipc};

pub struct MenuRequest {
    pub id: u64,
    pub output: Option<String>,
    pub x: f64,
    pub y: f64,
    pub maximized: bool,
    pub fullscreen: bool,
    pub workspace: usize,
}

/// Focus the window, then run `action` on it (in order).
fn on_window(id: u64, action: Action) {
    ipc::send_all(vec![Request::Action { action: Action::FocusWindow { id } }, Request::Action { action }]);
}

pub fn show(app: &gtk::Application, req: MenuRequest, workspaces: &[WorkspaceInfo]) {
    let window = gtk::Window::builder().application(app).decorated(false).build();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("m3-window-menu"));
    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(e, true);
    }
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.add_css_class("m3-layer");
    window.add_css_class("m3-transparent");
    if let Some(name) = &req.output {
        let monitor = gdk::Display::default().and_then(|d| {
            let list = d.monitors();
            (0..list.n_items())
                .filter_map(|i| list.item(i).and_downcast::<gdk::Monitor>())
                .find(|m| m.connector().as_deref() == Some(name.as_str()))
        });
        if let Some(m) = monitor {
            window.set_monitor(Some(&m));
        }
    }
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    window.set_child(Some(&area));

    let pop = gtk::Popover::new();
    pop.add_css_class("menu");
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    pop.set_parent(&area);
    pop.set_pointing_to(Some(&gdk::Rectangle::new(req.x as i32, req.y as i32, 1, 1)));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let item = |icon: &str, text: &str| {
        let b = gtk::Button::new();
        b.add_css_class("m3-menu-item");
        b.add_css_class("flat");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.append(&icons::symbol_with(icon, &["small"]));
        let l = gtk::Label::new(Some(text));
        l.set_xalign(0.0);
        row.append(&l);
        b.set_child(Some(&row));
        b
    };
    let id = req.id;
    let mut entries: Vec<(gtk::Button, Action)> = vec![
        (item("minimize", "Minimize"), Action::Minimize),
        (
            item(
                if req.maximized { "close_fullscreen" } else { "open_in_full" },
                if req.maximized { "Restore" } else { "Maximize" },
            ),
            Action::ToggleMaximize,
        ),
        (
            item(
                if req.fullscreen { "fullscreen_exit" } else { "fullscreen" },
                if req.fullscreen { "Leave full screen" } else { "Full screen" },
            ),
            Action::ToggleFullscreen,
        ),
    ];
    for ws in workspaces.iter().filter(|w| w.index != req.workspace).take(9) {
        entries.push((
            item("move_item", &format!("Move to {}", ws.name)),
            Action::MoveWindowToWorkspace { id, index: ws.index },
        ));
    }
    for (i, (b, action)) in entries.into_iter().enumerate() {
        if i == 3 {
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let (w, action) = (window.clone(), action);
        b.connect_clicked(move |_| {
            w.close();
            on_window(id, action.clone());
        });
        list.append(&b);
    }
    list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let close = item("close", "Close");
    close.add_css_class("m3-menu-danger");
    let w = window.clone();
    close.connect_clicked(move |_| {
        w.close();
        ipc::action(Action::CloseWindowById { id });
    });
    list.append(&close);
    pop.set_child(Some(&list));

    // Closing the menu (Esc, click outside, a choice) closes the overlay.
    let w = window.clone();
    pop.connect_closed(move |_| {
        let w = w.clone();
        glib::idle_add_local_once(move || w.close());
    });
    let w = window.clone();
    window.connect_close_request(move |_| {
        if let Some(c) = w.child() {
            if let Some(p) = c.first_child().and_downcast::<gtk::Popover>() {
                p.unparent();
            }
        }
        glib::Propagation::Proceed
    });
    // Pop up once the overlay has its size, so the menu opens at the pointer.
    let shown = std::rc::Rc::new(std::cell::Cell::new(false));
    let p = pop.clone();
    area.connect_resize(move |_, w, h| {
        if w > 0 && h > 0 && !shown.replace(true) {
            p.popup();
        }
    });
    window.present();
}
