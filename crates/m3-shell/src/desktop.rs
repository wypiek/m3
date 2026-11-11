//! Desktop widgets: a big expressive clock and greeting on one side, a
//! column of shaped widgets (day, shortcuts, now playing, countdown,
//! storage, RAM/CPU, weather) on the other. Drawn on the bottom layer, above
//! the wallpaper and below windows. Configured in `widgets.toml`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use chrono::{Datelike, Local, Timelike};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_core::config::{TemperatureUnit, WIDGETS, WidgetPlace, WidgetSide, WidgetsConfig};
use m3_widgets::{Shape, ShapeIcon};

use crate::icons;
use crate::shell::Cmd;

/// "Morning" / "Afternoon" / "Evening" / "Night" (pure; tested).
pub fn part_of_day(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Morning",
        12..=16 => "Afternoon",
        17..=21 => "Evening",
        _ => "Night",
    }
}

/// Days from `today` until `target` (negative once past; pure; tested).
pub fn days_until(today: chrono::NaiveDate, target: chrono::NaiveDate) -> i64 {
    (target - today).num_days()
}

/// Used memory in percent from `/proc/meminfo` text (pure; tested).
pub fn ram_percent(meminfo: &str) -> Option<f64> {
    let field =
        |name: &str| meminfo.lines().find_map(|l| l.strip_prefix(name)?.split_whitespace().next()?.parse::<f64>().ok());
    let (total, avail) = (field("MemTotal:")?, field("MemAvailable:")?);
    (total > 0.0).then(|| (1.0 - avail / total) * 100.0)
}

/// `(busy, total)` jiffies from the first line of `/proc/stat` (pure; tested).
pub fn cpu_times(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() < 5 {
        return None;
    }
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v[4];
    Some((total.saturating_sub(idle), total))
}

/// "476GB", "1.9TB" — decimal units like disk labels (pure; tested).
pub fn disk_size(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 { format!("{:.1}TB", gb / 1000.0) } else { format!("{gb:.0}GB") }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// The pieces that update over time.
#[derive(Default)]
struct Live {
    clock: Vec<gtk::Label>,
    day_num: Option<gtk::Label>,
    month_num: Option<gtk::Label>,
    part: Option<gtk::Label>,
    day_badge: Option<gtk::Label>,
    countdown: Option<(gtk::Box, gtk::Label, gtk::Label)>,
    storage: Option<(gtk::Label, gtk::Label)>,
    ram: Option<gtk::Label>,
    cpu: Option<gtk::Label>,
    media: Option<MediaParts>,
    weather: Option<WeatherParts>,
}

struct MediaParts {
    card: gtk::Box,
    art: gtk::Picture,
    art_fallback: ShapeIcon,
    play: gtk::Button,
    title: gtk::Label,
    player: Rc<RefCell<Option<String>>>,
}

struct WeatherParts {
    card: gtk::Box,
    icon: gtk::Label,
    temp: gtk::Label,
    cond: gtk::Label,
    days: gtk::Box,
}

/// One placed widget on the desktop.
struct Item {
    id: &'static str,
    /// Outer box that is moved/scaled; holds the widget and edit handles.
    wrapper: gtk::Overlay,
    content: gtk::Widget,
    /// Remove and resize buttons, shown only while editing.
    handles: [gtk::Button; 2],
    place: Cell<WidgetPlace>,
    /// Natural size of the content (no stretch, unscaled), measured once.
    nat: Cell<(f64, f64)>,
    /// Not placed by the user: positioned by the default layout.
    auto: Cell<bool>,
}

impl Item {
    /// Size on screen for `p`: the stretched box, scaled.
    fn screen_size(&self, p: WidgetPlace) -> (f64, f64) {
        let (w, h) = self.nat.get();
        (w * p.width * p.scale, h * p.height * p.scale)
    }
}

/// New place for a resize to a `w`×`h` px box on screen, given the
/// content's natural size: the content scales to fit the shorter side and
/// the card stretches in the other direction (pure; tested).
pub fn resized(p: WidgetPlace, nat: (f64, f64), w: f64, h: f64) -> WidgetPlace {
    let (nw, nh) = (nat.0.max(1.0), nat.1.max(1.0));
    let (w, h) = (w.max(nw * 0.5), h.max(nh * 0.5));
    let scale = (w / nw).min(h / nh).clamp(0.5, 2.5);
    WidgetPlace { scale, width: (w / (nw * scale)).clamp(1.0, 4.0), height: (h / (nh * scale)).clamp(1.0, 4.0), ..p }
}

/// What a drag on the desktop is doing (edit mode).
#[derive(Clone, Copy)]
enum DragOp {
    Move {
        item: usize,
        x: f64,
        y: f64,
    },
    /// Box size on screen when the resize started.
    Resize {
        item: usize,
        w0: f64,
        h0: f64,
    },
}

pub struct DesktopWidgets {
    pub window: gtk::Window,
    monitor: gdk::Monitor,
    cfg: RefCell<WidgetsConfig>,
    live: RefCell<Live>,
    dispatch: Rc<dyn Fn(Cmd)>,
    cpu_prev: Cell<Option<(u64, u64)>>,
    bus: RefCell<Option<gio::DBusConnection>>,
    fixed: gtk::Fixed,
    items: RefCell<Vec<Item>>,
    editing: Cell<bool>,
    toolbar: gtk::Box,
    add_menu: gtk::MenuButton,
    drag: Cell<Option<DragOp>>,
}

/// Keep a widget of `size` px fully inside an area of `area` px (pure; tested).
pub fn clamp_pos(pos: (f64, f64), size: (f64, f64), area: (f64, f64)) -> (f64, f64) {
    (pos.0.clamp(0.0, (area.0 - size.0).max(0.0)), pos.1.clamp(0.0, (area.1 - size.1).max(0.0)))
}

impl DesktopWidgets {
    pub fn new(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        cfg: &WidgetsConfig,
        dispatch: Rc<dyn Fn(Cmd)>,
    ) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Bottom);
        window.set_namespace(Some("m3-widgets"));
        window.set_monitor(Some(monitor));
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::None);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-transparent");
        window.add_css_class("m3-desktop");

        // Background area (resize → relayout), the freely placed widgets and
        // the edit-mode toolbar on top.
        let area = gtk::DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        let fixed = gtk::Fixed::new();
        fixed.add_css_class("m3-desktop-root");
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.add_css_class("m3-desk-toolbar");
        toolbar.set_halign(gtk::Align::Center);
        toolbar.set_valign(gtk::Align::Start);
        toolbar.set_margin_top(24);
        toolbar.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&area));
        overlay.add_overlay(&fixed);
        overlay.add_overlay(&toolbar);
        window.set_child(Some(&overlay));

        let me = Rc::new(Self {
            window,
            monitor: monitor.clone(),
            cfg: RefCell::new(cfg.clone()),
            live: RefCell::new(Live::default()),
            dispatch,
            cpu_prev: Cell::new(None),
            bus: RefCell::new(None),
            fixed,
            items: RefCell::new(Vec::new()),
            editing: Cell::new(false),
            toolbar,
            add_menu: gtk::MenuButton::new(),
            drag: Cell::new(None),
        });
        {
            let weak = Rc::downgrade(&me);
            area.connect_resize(move |_, _, _| {
                if let Some(me) = weak.upgrade() {
                    me.relayout();
                }
            });
        }
        me.build_toolbar();
        me.connect_input();
        me.rebuild();
        me.start_timers();
        me.window.set_visible(cfg.enabled);
        me
    }

    pub fn apply_config(self: &Rc<Self>, cfg: &WidgetsConfig) {
        if *self.cfg.borrow() == *cfg {
            return;
        }
        *self.cfg.borrow_mut() = cfg.clone();
        self.rebuild();
        self.window.set_visible(cfg.enabled);
        self.refresh_slow();
        self.refresh_weather();
    }

    // ---- layout and editing ------------------------------------------------------

    /// Desktop area in logical pixels.
    fn area(&self) -> (f64, f64) {
        let (w, h) = (self.window.width(), self.window.height());
        if w > 0 && h > 0 {
            (f64::from(w), f64::from(h))
        } else {
            let g = self.monitor.geometry();
            (f64::from(g.width()), f64::from(g.height()))
        }
    }

    fn natural_size(w: &impl IsA<gtk::Widget>) -> (f64, f64) {
        let (_, nw, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
        let (_, nh, _, _) = w.measure(gtk::Orientation::Vertical, nw);
        (f64::from(nw), f64::from(nh))
    }

    /// Position every item from its saved place, or the default layout.
    fn relayout(&self) {
        let area = self.area();
        self.auto_layout(area);
        for item in self.items.borrow().iter() {
            self.apply_place(item, area);
        }
    }

    /// Default layout for widgets the user hasn't placed: the clock and
    /// greeting stacked on one side, the rest in two columns on the other,
    /// stacked from their real sizes, centred vertically and scaled down
    /// if the screen is too short — so nothing overlaps at any resolution.
    fn auto_layout(&self, area: (f64, f64)) {
        let items = self.items.borrow();
        if !items.iter().any(|i| i.auto.get()) {
            return;
        }
        let right = self.cfg.borrow().side == WidgetSide::Right;
        let (margin, gap) = (area.0 * 0.05, 18.0);
        let size = |id: &str| items.iter().find(|i| i.id == id).map(|i| i.nat.get());
        let hero: Vec<&str> = ["clock", "greeting"].into_iter().filter(|id| size(id).is_some()).collect();
        let col0: Vec<&str> =
            ["day", "media", "storage", "system"].into_iter().filter(|id| size(id).is_some()).collect();
        let col1: Vec<&str> =
            ["shortcuts", "countdown", "weather"].into_iter().filter(|id| size(id).is_some()).collect();
        let height = |ids: &[&str]| -> f64 {
            ids.iter().filter_map(|id| size(id)).map(|s| s.1).sum::<f64>() + gap * ids.len().saturating_sub(1) as f64
        };
        let width = |ids: &[&str]| ids.iter().filter_map(|id| size(id)).map(|s| s.0).fold(0.0, f64::max);
        let tallest = height(&hero).max(height(&col0)).max(height(&col1));
        let avail = area.1 * 0.9;
        let scale = if tallest > avail { (avail / tallest).max(0.5) } else { 1.0 };
        let place_column = |ids: &[&str], x: f64| {
            let mut y = ((area.1 - height(ids) * scale) / 2.0).max(0.0);
            for id in ids {
                let Some(it) = items.iter().find(|i| i.id == *id) else { continue };
                let (w, h) = it.nat.get();
                // Centre narrower widgets in their column.
                let cx = x + (width(ids) - w) * scale / 2.0;
                if it.auto.get() {
                    it.place.set(WidgetPlace { x: cx / area.0, y: y / area.1, scale, ..Default::default() });
                }
                y += (h + gap) * scale;
            }
        };
        let (w0, w1) = (width(&col0) * scale, width(&col1) * scale);
        if right {
            place_column(&hero, margin);
            place_column(&col1, area.0 - margin - w1);
            place_column(&col0, area.0 - margin - w1 - gap - w0);
        } else {
            place_column(&hero, area.0 - margin - width(&hero) * scale);
            place_column(&col0, margin);
            place_column(&col1, margin + w0 + gap);
        }
    }

    fn apply_place(&self, item: &Item, area: (f64, f64)) {
        let p = item.place.get();
        let (nw, nh) = item.nat.get();
        // The card stretches; the content inside stays centred.
        item.content.set_size_request((nw * p.width).round() as i32, (nh * p.height).round() as i32);
        let s = p.scale.clamp(0.5, 2.5);
        let (x, y) = clamp_pos((p.x * area.0, p.y * area.1), item.screen_size(p), area);
        let t = gtk::gsk::Transform::new()
            .translate(&gtk::graphene::Point::new(x as f32, y as f32))
            .scale(s as f32, s as f32);
        self.fixed.set_child_transform(&item.wrapper, Some(&t));
    }

    /// Save layout / visibility to `widgets.toml` (the shell's file watcher
    /// then sees an unchanged config and does nothing).
    fn save(&self) {
        let c = self.cfg.borrow().clone();
        match m3_core::config::default_dir() {
            Ok(dir) => {
                if let Err(e) = m3_core::config::save(&dir, &c) {
                    tracing::warn!("cannot save widgets.toml: {e}");
                }
            }
            Err(e) => tracing::warn!("cannot save widgets.toml: {e}"),
        }
    }

    fn build_toolbar(self: &Rc<Self>) {
        let tb = &self.toolbar;
        tb.append(&label("Edit widgets", &["m3-title-small-emphasized", "m3-desk-toolbar-title"]));
        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        add_row.append(&icons::symbol_with("add", &["small"]));
        add_row.append(&gtk::Label::new(Some("Add widget")));
        self.add_menu.set_child(Some(&add_row));
        self.add_menu.add_css_class("m3-tonal");
        tb.append(&self.add_menu);
        let button = |icon: &str, text: &str, class: &str| {
            let b = gtk::Button::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(&icons::symbol_with(icon, &["small"]));
            row.append(&gtk::Label::new(Some(text)));
            b.set_child(Some(&row));
            b.add_css_class(class);
            b
        };
        let reset = button("restart_alt", "Reset layout", "tonal");
        let settings = button("tune", "Settings", "tonal");
        let done = button("check", "Done", "filled");
        let weak = Rc::downgrade(self);
        reset.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.cfg.borrow_mut().layout.clear();
                me.save();
                me.rebuild();
            }
        });
        settings.connect_clicked(|_| open_settings());
        let weak = Rc::downgrade(self);
        done.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.set_editing(false);
            }
        });
        tb.append(&reset);
        tb.append(&settings);
        tb.append(&done);
    }

    /// Refill the "Add widget" menu with the widgets that are hidden.
    fn refresh_add_menu(self: &Rc<Self>) {
        let pop = gtk::Popover::new();
        pop.add_css_class("menu");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let cfg = self.cfg.borrow().clone();
        let hidden: Vec<(&'static str, &'static str)> =
            WIDGETS.iter().copied().filter(|(id, _)| !cfg.shown(id) || !cfg.configured(id)).collect();
        for (id, name) in hidden.iter().copied() {
            let b = gtk::Button::new();
            b.add_css_class("m3-menu-item");
            b.add_css_class("flat");
            let text = if cfg.configured(id) { name.to_owned() } else { format!("{name} — set up in Settings") };
            b.set_child(Some(&label(&text, &[])));
            let (weak, p) = (Rc::downgrade(self), pop.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                let Some(me) = weak.upgrade() else { return };
                me.cfg.borrow_mut().set_shown(id, true);
                me.save();
                let ready = me.cfg.borrow().configured(id);
                if ready {
                    me.rebuild();
                } else {
                    open_settings();
                }
            });
            list.append(&b);
        }
        if hidden.is_empty() {
            list.append(&label("All widgets are on the desktop", &["m3-secondary-text", "m3-desk-menu-note"]));
        }
        pop.set_child(Some(&list));
        self.add_menu.set_popover(Some(&pop));
    }

    pub fn set_editing(self: &Rc<Self>, on: bool) {
        if self.editing.replace(on) == on {
            return;
        }
        // Above windows while editing, so every widget can be reached.
        self.window.set_layer(if on { Layer::Top } else { Layer::Bottom });
        self.window.set_keyboard_mode(if on { KeyboardMode::Exclusive } else { KeyboardMode::None });
        self.toolbar.set_visible(on);
        if on {
            self.window.add_css_class("editing");
            self.refresh_add_menu();
            m3_widgets::animate(&self.toolbar, "m3-enter");
        } else {
            self.window.remove_css_class("editing");
        }
        for item in self.items.borrow().iter() {
            // While editing, a press moves the widget instead of using it.
            item.content.set_can_target(!on);
            for h in &item.handles {
                h.set_visible(on);
            }
        }
    }

    /// The item under (x, y) in desktop coordinates and whether the press is
    /// on its resize handle.
    fn item_at(&self, x: f64, y: f64) -> Option<(usize, bool)> {
        let picked = self.fixed.pick(x, y, gtk::PickFlags::DEFAULT)?;
        let items = self.items.borrow();
        let i = items.iter().position(|it| {
            let w: &gtk::Widget = it.wrapper.upcast_ref();
            picked == *w || picked.is_ancestor(w)
        })?;
        let on_handle = picked.has_css_class("m3-desk-resize")
            || picked.ancestor(gtk::Button::static_type()).is_some_and(|b| b.has_css_class("m3-desk-resize"));
        let on_close = picked.has_css_class("m3-desk-remove")
            || picked.ancestor(gtk::Button::static_type()).is_some_and(|b| b.has_css_class("m3-desk-remove"));
        if on_close {
            return None;
        }
        Some((i, on_handle))
    }

    fn connect_input(self: &Rc<Self>) {
        // Move / resize in edit mode.
        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |g, x, y| {
            let Some(me) = weak.upgrade() else { return };
            let hit = if me.editing.get() { me.item_at(x, y) } else { None };
            let Some((i, handle)) = hit else {
                g.set_state(gtk::EventSequenceState::Denied);
                return;
            };
            g.set_state(gtk::EventSequenceState::Claimed);
            let items = me.items.borrow();
            let it = &items[i];
            let p = it.place.get();
            let area = me.area();
            let size = it.screen_size(p);
            let (px, py) = clamp_pos((p.x * area.0, p.y * area.1), size, area);
            me.drag.set(Some(if handle {
                DragOp::Resize { item: i, w0: size.0, h0: size.1 }
            } else {
                DragOp::Move { item: i, x: px, y: py }
            }));
            it.wrapper.add_css_class("dragging");
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |_, dx, dy| {
            let Some(me) = weak.upgrade() else { return };
            let area = me.area();
            let items = me.items.borrow();
            match me.drag.get() {
                Some(DragOp::Move { item, x, y }) => {
                    let it = &items[item];
                    let mut p = it.place.get();
                    let (nx, ny) = clamp_pos((x + dx, y + dy), it.screen_size(p), area);
                    p.x = nx / area.0;
                    p.y = ny / area.1;
                    it.place.set(p);
                    me.apply_place(it, area);
                }
                Some(DragOp::Resize { item, w0, h0 }) => {
                    // The corner follows the pointer in both directions.
                    let it = &items[item];
                    let p = resized(it.place.get(), it.nat.get(), w0 + dx, h0 + dy);
                    it.place.set(p);
                    me.apply_place(it, area);
                }
                None => {}
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            let Some(me) = weak.upgrade() else { return };
            let Some(op) = me.drag.take() else { return };
            let i = match op {
                DragOp::Move { item, .. } | DragOp::Resize { item, .. } => item,
            };
            let entry = me.items.borrow().get(i).map(|it| {
                it.wrapper.remove_css_class("dragging");
                (it.id, it.place.get())
            });
            if let Some(it) = me.items.borrow().get(i) {
                it.auto.set(false);
            }
            if let Some((id, place)) = entry {
                // Tidy numbers in widgets.toml.
                let round = |v: f64, k: f64| (v * k).round() / k;
                let place = WidgetPlace {
                    x: round(place.x, 1e4),
                    y: round(place.y, 1e4),
                    scale: round(place.scale, 100.0),
                    width: round(place.width, 100.0),
                    height: round(place.height, 100.0),
                };
                me.cfg.borrow_mut().layout.insert(id.to_owned(), place);
                me.save();
            }
        });
        self.fixed.add_controller(drag);

        // Right-click on the desktop: Edit widgets / Settings.
        let click = gtk::GestureClick::new();
        click.set_button(3);
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |g, _, x, y| {
            let Some(me) = weak.upgrade() else { return };
            if me.editing.get() {
                return;
            }
            g.set_state(gtk::EventSequenceState::Claimed);
            me.show_desktop_menu(x, y);
        });
        self.fixed.add_controller(click);

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(me) = weak.upgrade() {
                    me.set_editing(false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);
    }

    fn show_desktop_menu(self: &Rc<Self>, x: f64, y: f64) {
        let pop = gtk::Popover::new();
        pop.add_css_class("menu");
        pop.set_has_arrow(false);
        pop.set_parent(&self.fixed);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.set_position(gtk::PositionType::Bottom);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let item = |icon: &str, text: &str| {
            let b = gtk::Button::new();
            b.add_css_class("m3-menu-item");
            b.add_css_class("flat");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.append(&icons::symbol_with(icon, &["small"]));
            row.append(&gtk::Label::new(Some(text)));
            b.set_child(Some(&row));
            b
        };
        let edit = item("edit", "Edit widgets");
        let settings = item("widgets", "Widget settings");
        let style = item("palette", "Wallpaper and style");
        let (weak, p) = (Rc::downgrade(self), pop.clone());
        edit.connect_clicked(move |_| {
            p.popdown();
            if let Some(me) = weak.upgrade() {
                me.set_editing(true);
            }
        });
        let p = pop.clone();
        settings.connect_clicked(move |_| {
            p.popdown();
            open_settings();
        });
        let p = pop.clone();
        style.connect_clicked(move |_| {
            p.popdown();
            spawn_settings("appearance");
        });
        list.append(&edit);
        list.append(&settings);
        list.append(&style);
        pop.set_child(Some(&list));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Wrap a widget for free placement, with edit-mode handles on top.
    fn wrap(self: &Rc<Self>, id: &'static str, content: gtk::Widget) -> Item {
        // When the card is stretched, extra room goes to spacers at both
        // ends so the content stays centred.
        if let Some(b) = content.downcast_ref::<gtk::Box>() {
            let vertical = b.orientation() == gtk::Orientation::Vertical;
            let spacer = || {
                let s = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                if vertical {
                    s.set_vexpand(true)
                } else {
                    s.set_hexpand(true)
                }
                s.set_can_target(false);
                s
            };
            b.prepend(&spacer());
            b.append(&spacer());
        }
        let nat = Self::natural_size(&content);
        let wrapper = gtk::Overlay::new();
        wrapper.add_css_class("m3-desk-item");
        wrapper.set_child(Some(&content));
        let close = icons::icon_button("close", "Remove from desktop");
        close.add_css_class("m3-desk-remove");
        close.set_halign(gtk::Align::End);
        close.set_valign(gtk::Align::Start);
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.cfg.borrow_mut().set_shown(id, false);
                me.save();
                me.rebuild();
            }
        });
        let resize = icons::icon_button("open_in_full", "Drag to resize");
        resize.add_css_class("m3-desk-resize");
        resize.set_halign(gtk::Align::End);
        resize.set_valign(gtk::Align::End);
        let editing = self.editing.get();
        close.set_visible(editing);
        resize.set_visible(editing);
        wrapper.add_overlay(&close);
        wrapper.add_overlay(&resize);
        let saved = self.cfg.borrow().layout.get(id).copied();
        Item {
            id,
            wrapper,
            content,
            handles: [close, resize],
            place: Cell::new(saved.unwrap_or_default()),
            nat: Cell::new(nat),
            auto: Cell::new(saved.is_none()),
        }
    }

    fn rebuild(self: &Rc<Self>) {
        let cfg = self.cfg.borrow().clone();
        let mut live = Live::default();
        for item in self.items.borrow_mut().drain(..) {
            self.fixed.remove(&item.wrapper);
        }
        let mut built: Vec<(&'static str, gtk::Widget)> = Vec::new();
        for (id, _) in WIDGETS {
            if !cfg.shown(id) || !cfg.configured(id) {
                continue;
            }
            let w: gtk::Widget = match *id {
                "clock" => self.clock(&mut live).upcast(),
                "greeting" => self.greeting(&cfg, &mut live).upcast(),
                "day" => self.day_badge(&mut live).upcast(),
                "shortcuts" => self.shortcuts(&cfg).upcast(),
                "media" => self.media(&mut live).upcast(),
                "countdown" => self.countdown(&cfg, &mut live).upcast(),
                "storage" => self.system(&WidgetsConfig { system: false, ..cfg.clone() }, &mut live).upcast(),
                "system" => self.system(&WidgetsConfig { storage: false, ..cfg.clone() }, &mut live).upcast(),
                "weather" => self.weather(&mut live).upcast(),
                _ => continue,
            };
            built.push((id, w));
        }
        let editing = self.editing.get();
        let mut items = Vec::new();
        for (id, w) in built {
            let item = self.wrap(id, w);
            item.content.set_can_target(!editing);
            self.fixed.put(&item.wrapper, 0.0, 0.0);
            // Measure once styled (CSS applies after it joins the window).
            item.nat.set(Self::natural_size(&item.content));
            items.push(item);
        }
        *self.items.borrow_mut() = items;
        *self.live.borrow_mut() = live;
        self.tick();
        self.relayout();
        if editing {
            self.refresh_add_menu();
        }
        m3_widgets::animate(&self.fixed, "m3-fade");
    }

    // ---- hero: clock + greeting ---------------------------------------------

    fn clock(&self, live: &mut Live) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        b.add_css_class("m3-desk-clock");
        b.set_halign(gtk::Align::Start);
        for i in 0..4 {
            if i == 2 {
                let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                gap.set_size_request(28, -1);
                b.append(&gap);
            }
            // Two-tone: the last digit of each pair in the accent colour.
            let d = label("0", &["m3-desk-digit", if i % 2 == 1 { "accent" } else { "ink" }]);
            b.append(&d);
            live.clock.push(d);
        }
        b.update_property(&[gtk::accessible::Property::Label("Time")]);
        b
    }

    fn greeting(&self, cfg: &WidgetsConfig, live: &mut Live) -> gtk::Box {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 14);
        col.set_halign(gtk::Align::Start);
        // "G (day) (month) D" — the O's of "GOOD" are the date.
        let word = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        word.add_css_class("m3-desk-good");
        word.append(&label("G", &["m3-desk-letter"]));
        let day = ShapeIcon::new(Shape::Cookie(12), "1", 92, false);
        day.widget().add_css_class("m3-desk-date-cookie");
        day.label().add_css_class("m3-desk-date-num");
        let month = ShapeIcon::new(Shape::Cookie(12), "1", 92, false);
        month.widget().add_css_class("m3-desk-date-cookie");
        month.label().add_css_class("m3-desk-date-num");
        word.append(day.widget());
        word.append(month.widget());
        word.append(&label("D", &["m3-desk-letter"]));
        col.append(&word);
        let pills = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pills.add_css_class("m3-desk-pills");
        let part = label("", &["m3-desk-pill", "rose"]);
        let name = if cfg.name.trim().is_empty() { default_name() } else { cfg.name.trim().to_owned() };
        pills.append(&part);
        if !name.is_empty() {
            pills.append(&label(&name, &["m3-desk-pill", "gold"]));
        }
        col.append(&pills);
        live.day_num = Some(day.label().clone());
        live.month_num = Some(month.label().clone());
        live.part = Some(part);
        col
    }

    // ---- column widgets ------------------------------------------------------

    fn day_badge(&self, live: &mut Live) -> gtk::Overlay {
        let o = ShapeIcon::new(Shape::Flower(8), "", 176, false);
        o.widget().add_css_class("m3-desk-day");
        o.label().add_css_class("m3-desk-day-num");
        live.day_badge = Some(o.label().clone());
        o.widget().clone()
    }

    fn shortcuts(&self, cfg: &WidgetsConfig) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("m3-desk-card");
        card.add_css_class("m3-desk-shortcuts");
        let apps: Vec<gio_unix::DesktopAppInfo> =
            cfg.shortcut_apps.iter().filter_map(|id| gio_unix::DesktopAppInfo::new(id)).take(3).collect();
        let circle = |app: &gio_unix::DesktopAppInfo| {
            let b = gtk::Button::new();
            b.add_css_class("m3-desk-circle");
            b.add_css_class("rose");
            b.set_child(Some(&icons::app_icon(gio::prelude::AppInfoExt::icon(app), 26)));
            let name = gio::prelude::AppInfoExt::display_name(app);
            b.set_tooltip_text(Some(&name));
            b.update_property(&[gtk::accessible::Property::Label(&name)]);
            let app = app.clone();
            b.connect_clicked(move |_| {
                let ctx = gdk::Display::default().map(|d| d.app_launch_context());
                if let Err(e) = gio::prelude::AppInfoExt::launch(&app, &[], ctx.as_ref()) {
                    tracing::warn!("cannot start {}: {e}", gio::prelude::AppInfoExt::name(&app));
                }
            });
            b
        };
        if let Some(first) = apps.first() {
            let top = circle(first);
            top.set_halign(gtk::Align::Center);
            card.append(&top);
        }
        if apps.len() > 1 {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_halign(gtk::Align::Center);
            for a in &apps[1..] {
                row.append(&circle(a));
            }
            card.append(&row);
        }
        let search = gtk::Button::new();
        search.add_css_class("m3-desk-search");
        let sb = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        sb.append(&icons::symbol_with("search", &["small"]));
        sb.append(&label("Search", &["m3-label-large-emphasized"]));
        search.set_child(Some(&sb));
        let d = self.dispatch.clone();
        search.connect_clicked(move |_| d(Cmd::ToggleLauncher));
        card.append(&search);
        card
    }

    fn media(self: &Rc<Self>, live: &mut Live) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("m3-desk-card");
        card.add_css_class("m3-desk-media");
        let grid = gtk::Grid::new();
        grid.set_column_spacing(4);
        grid.set_row_spacing(4);
        let player: Rc<RefCell<Option<String>>> = Rc::default();
        let control = |icon: &str, tip: &str, method: &'static str, big: bool| {
            let b = icons::icon_button(icon, tip);
            b.add_css_class("m3-desk-circle");
            b.add_css_class(if big { "rose" } else { "ink" });
            if big {
                b.add_css_class("big");
            }
            let (weak, player) = (Rc::downgrade(self), player.clone());
            b.connect_clicked(move |_| {
                let (Some(me), Some(p)) = (weak.upgrade(), player.borrow().clone()) else { return };
                let conn = me.bus.borrow().clone();
                let weak = weak.clone();
                glib::spawn_future_local(async move {
                    if let Some(conn) = conn {
                        m3_widgets::mpris::control(&conn, &p, method).await;
                    }
                    if let Some(me) = weak.upgrade() {
                        me.refresh_media();
                    }
                });
            });
            b
        };
        let prev = control("skip_previous", "Previous", "Previous", false);
        let play = control("play_arrow", "Play", "PlayPause", true);
        let next = control("skip_next", "Next", "Next", false);
        let art_box = gtk::Overlay::new();
        art_box.add_css_class("m3-desk-art");
        let art = gtk::Picture::new();
        art.set_content_fit(gtk::ContentFit::Cover);
        art.set_size_request(112, 112);
        art.set_overflow(gtk::Overflow::Hidden);
        art.add_css_class("m3-desk-art-picture");
        let fallback = ShapeIcon::new(Shape::Cookie(9), "music_note", 112, true);
        fallback.widget().add_css_class("m3-desk-art-fallback");
        art_box.set_child(Some(fallback.widget()));
        art_box.add_overlay(&art);
        grid.attach(&prev, 0, 0, 1, 1);
        grid.attach(&play, 1, 0, 1, 1);
        grid.attach(&art_box, 0, 1, 1, 1);
        grid.attach(&next, 1, 1, 1, 1);
        prev.set_halign(gtk::Align::End);
        next.set_valign(gtk::Align::Center);
        card.append(&grid);
        let title = label("Nothing playing", &["m3-label-medium", "m3-desk-media-title"]);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(16);
        card.append(&title);
        live.media = Some(MediaParts { card: card.clone(), art, art_fallback: fallback, play, title, player });
        card
    }

    fn countdown(&self, cfg: &WidgetsConfig, live: &mut Live) -> gtk::Box {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
        col.add_css_class("m3-desk-countdown");
        let dome = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dome.add_css_class("m3-desk-card");
        dome.add_css_class("m3-desk-dome");
        let title = label(&cfg.countdown.title, &["m3-title-medium-emphasized"]);
        title.set_vexpand(true);
        title.set_valign(gtk::Align::Center);
        title.set_wrap(true);
        title.set_justify(gtk::Justification::Center);
        title.set_max_width_chars(14);
        dome.append(&title);
        col.append(&dome);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::Center);
        let n = ShapeIcon::new(Shape::Sunny(8), "0", 64, false);
        n.widget().add_css_class("m3-desk-sunny");
        n.label().add_css_class("m3-desk-sunny-num");
        let caption = label("Days to go", &["m3-desk-pill", "rose", "small"]);
        row.append(n.widget());
        row.append(&caption);
        col.append(&row);
        live.countdown = Some((col.clone(), n.label().clone(), caption));
        col
    }

    fn system(&self, cfg: &WidgetsConfig, live: &mut Live) -> gtk::Box {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 12);
        if cfg.storage {
            let pill = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            pill.add_css_class("m3-desk-card");
            pill.add_css_class("m3-desk-storage");
            let total = label("—", &["m3-desk-stat"]);
            let free = label("—", &["m3-desk-stat"]);
            let side = |t: &str, v: &gtk::Label| {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.append(&label(t, &["m3-label-small"]));
                b.append(v);
                b
            };
            pill.append(&side("Total", &total));
            let icon = ShapeIcon::new(Shape::Cookie(9), "hard_drive", 52, true);
            icon.widget().add_css_class("m3-desk-storage-icon");
            pill.append(icon.widget());
            pill.append(&side("Free", &free));
            col.append(&pill);
            live.storage = Some((total, free));
        }
        if cfg.system {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_halign(gtk::Align::Center);
            let bubble = |name: &str, tone: &str| {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.add_css_class("m3-desk-bubble");
                b.add_css_class(tone);
                b.set_valign(gtk::Align::Center);
                let v = label("—", &["m3-desk-stat"]);
                b.append(&v);
                b.append(&label(name, &["m3-label-medium"]));
                (b, v)
            };
            let (rb, ram) = bubble("RAM", "rose");
            let (cb, cpu) = bubble("CPU", "gold");
            row.append(&rb);
            row.append(&cb);
            col.append(&row);
            live.ram = Some(ram);
            live.cpu = Some(cpu);
        }
        col
    }

    fn weather(&self, live: &mut Live) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
        card.add_css_class("m3-desk-card");
        card.add_css_class("m3-desk-weather");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let icon = label("cloud", &["m3-icon", "m3-desk-weather-icon"]);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        let temp = label("—", &["m3-title-large-emphasized"]);
        temp.set_xalign(1.0);
        let cond = label("Loading…", &["m3-label-medium"]);
        cond.set_xalign(1.0);
        text.append(&temp);
        text.append(&cond);
        top.append(&icon);
        top.append(&text);
        card.append(&top);
        let days = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        days.set_homogeneous(true);
        card.append(&days);
        live.weather = Some(WeatherParts { card: card.clone(), icon, temp, cond, days });
        card
    }

    // ---- updates ---------------------------------------------------------------

    fn start_timers(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(5, move || match weak.upgrade() {
            Some(me) => {
                me.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(3, move || match weak.upgrade() {
            Some(me) => {
                me.refresh_system();
                me.refresh_media();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(120, move || match weak.upgrade() {
            Some(me) => {
                me.refresh_slow();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(30 * 60, move || match weak.upgrade() {
            Some(me) => {
                me.refresh_weather();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let bus = gio::bus_get_future(gio::BusType::Session).await.ok();
            if let Some(me) = weak.upgrade() {
                *me.bus.borrow_mut() = bus;
                me.refresh_media();
            }
        });
        self.refresh_system();
        self.refresh_slow();
        self.refresh_weather();
    }

    /// Clock, date, part of day, countdown.
    fn tick(&self) {
        let now = Local::now();
        let live = self.live.borrow();
        let digits: Vec<char> = now.format("%H%M").to_string().chars().collect();
        for (l, d) in live.clock.iter().zip(digits) {
            let s = d.to_string();
            if l.text() != s {
                l.set_text(&s);
            }
        }
        if let Some(l) = &live.day_num {
            l.set_text(&format!("{:02}", now.day()));
        }
        if let Some(l) = &live.month_num {
            l.set_text(&format!("{:02}", now.month()));
        }
        if let Some(l) = &live.part {
            l.set_text(part_of_day(now.hour()));
        }
        if let Some(l) = &live.day_badge {
            l.set_text(&now.day().to_string());
        }
        if let Some((card, n, caption)) = &live.countdown {
            let cfg = self.cfg.borrow();
            let target = m3_core::config::parse_date(&cfg.countdown.date)
                .ok()
                .and_then(|(y, m, d)| chrono::NaiveDate::from_ymd_opt(y, m, d));
            match target.map(|t| days_until(now.date_naive(), t)) {
                Some(d) if d > 0 => {
                    card.set_visible(true);
                    n.set_text(&d.to_string());
                    caption.set_text(if d == 1 { "Day to go" } else { "Days to go" });
                }
                Some(0) => {
                    card.set_visible(true);
                    n.set_text("0");
                    caption.set_text("It's today!");
                }
                _ => card.set_visible(false),
            }
        }
    }

    fn refresh_system(&self) {
        let live = self.live.borrow();
        if let Some(l) = &live.ram {
            if let Some(p) = std::fs::read_to_string("/proc/meminfo").ok().as_deref().and_then(ram_percent) {
                l.set_text(&format!("{p:.0}%"));
            }
        }
        if let Some(l) = &live.cpu {
            if let Some(now) = std::fs::read_to_string("/proc/stat").ok().as_deref().and_then(cpu_times) {
                if let Some((busy0, total0)) = self.cpu_prev.replace(Some(now)) {
                    let (db, dt) = (now.0.saturating_sub(busy0), now.1.saturating_sub(total0));
                    if dt > 0 {
                        l.set_text(&format!("{:.0}%", db as f64 * 100.0 / dt as f64));
                    }
                }
            }
        }
    }

    /// Storage (changes slowly).
    fn refresh_slow(&self) {
        let live = self.live.borrow();
        let Some((total, free)) = &live.storage else { return };
        let root = gio::File::for_path("/");
        match root.query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE) {
            Ok(info) => {
                total.set_text(&disk_size(info.attribute_uint64("filesystem::size")));
                free.set_text(&disk_size(info.attribute_uint64("filesystem::free")));
            }
            Err(e) => tracing::debug!("filesystem info unavailable: {e}"),
        }
    }

    fn refresh_media(self: &Rc<Self>) {
        let Some(conn) = self.bus.borrow().clone() else { return };
        if self.live.borrow().media.is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let now = m3_widgets::mpris::query(&conn).await;
            let Some(me) = weak.upgrade() else { return };
            let live = me.live.borrow();
            let Some(m) = &live.media else { return };
            *m.player.borrow_mut() = now.as_ref().map(|n| n.bus_name.clone());
            m.card.set_sensitive(true);
            match now {
                Some(n) => {
                    let text =
                        if n.artist.is_empty() { n.title.clone() } else { format!("{} · {}", n.title, n.artist) };
                    m.title.set_text(&text);
                    m.card.set_tooltip_text(Some(&text));
                    if let Some(l) = m.play.child().and_downcast::<gtk::Label>() {
                        l.set_text(if n.playing { "pause" } else { "play_arrow" });
                    }
                    m.play.update_property(&[gtk::accessible::Property::Label(if n.playing {
                        "Pause"
                    } else {
                        "Play"
                    })]);
                    match &n.art {
                        Some(p) => {
                            if m.art.file().and_then(|f| f.path()).as_ref() != Some(p) {
                                m.art.set_filename(Some(p));
                            }
                            m.art.set_visible(true);
                        }
                        None => m.art.set_visible(false),
                    }
                    m.art_fallback.shape().set_spin(if n.playing && n.art.is_none() { 30.0 } else { 0.0 });
                }
                None => {
                    m.title.set_text("Nothing playing");
                    m.card.set_tooltip_text(None);
                    m.art.set_visible(false);
                    m.art_fallback.shape().set_spin(0.0);
                    if let Some(l) = m.play.child().and_downcast::<gtk::Label>() {
                        l.set_text("play_arrow");
                    }
                }
            }
        });
    }

    fn refresh_weather(self: &Rc<Self>) {
        let cfg = self.cfg.borrow().weather.clone();
        if !cfg.enabled || self.live.borrow().weather.is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        let fahrenheit = cfg.unit == TemperatureUnit::Fahrenheit;
        glib::spawn_future_local(async move {
            let r =
                gio::spawn_blocking(move || m3_services::weather::forecast(cfg.latitude, cfg.longitude, fahrenheit))
                    .await;
            let Some(me) = weak.upgrade() else { return };
            let live = me.live.borrow();
            let Some(w) = &live.weather else { return };
            match r {
                Ok(Ok(f)) => {
                    let (desc, icon) = m3_services::weather::describe(f.code, f.is_day);
                    w.icon.set_text(icon);
                    w.temp.set_text(&format!("{:.0}°", f.temperature));
                    w.cond.set_text(desc);
                    while let Some(c) = w.days.first_child() {
                        w.days.remove(&c);
                    }
                    for (i, d) in f.days.iter().skip(1).take(3).enumerate() {
                        let day = chrono::NaiveDate::parse_from_str(&d.date, "%Y-%m-%d")
                            .map(|x| x.format("%a").to_string())
                            .unwrap_or_default();
                        let pill = gtk::Box::new(gtk::Orientation::Vertical, 4);
                        pill.add_css_class("m3-desk-day-pill");
                        pill.add_css_class(if i == 1 { "gold" } else { "rose" });
                        pill.append(&label(&day, &["m3-label-small"]));
                        pill.append(&label(m3_services::weather::describe(d.code, true).1, &["m3-icon", "small"]));
                        pill.append(&label(&format!("{:.0}°", d.max), &["m3-label-small"]));
                        pill.set_tooltip_text(Some(&format!(
                            "{} — {:.0}° / {:.0}°",
                            m3_services::weather::describe(d.code, true).0,
                            d.max,
                            d.min
                        )));
                        w.days.append(&pill);
                    }
                    w.card.set_tooltip_text(Some(&me.cfg.borrow().weather.place));
                }
                Ok(Err(e)) => {
                    tracing::info!("{e}");
                    w.cond.set_text("Offline");
                }
                Err(_) => w.cond.set_text("Offline"),
            }
        });
    }
}

/// Start the Settings app on `page` (argv, no shell).
fn spawn_settings(page: &str) {
    let argv = [std::ffi::OsStr::new("m3-settings"), std::ffi::OsStr::new("--page"), std::ffi::OsStr::new(page)];
    if let Err(e) = gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        tracing::warn!("cannot start Settings: {e}");
    }
}

fn open_settings() {
    spawn_settings("widgets");
}

/// First name from the account's real name, else the login name.
fn default_name() -> String {
    let real = glib::real_name().to_string_lossy().into_owned();
    let first = real.split_whitespace().next().unwrap_or_default();
    if first.is_empty() || first == "Unknown" {
        glib::user_name().to_string_lossy().into_owned()
    } else {
        first.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_stretches_one_way_and_scales_the_other() {
        let p = WidgetPlace::default();
        // Twice as tall: content stays 1×, the card stretches vertically.
        let r = resized(p, (200.0, 100.0), 200.0, 200.0);
        assert_eq!((r.scale, r.width, r.height), (1.0, 1.0, 2.0));
        // Both bigger: content scales by the smaller factor.
        let r = resized(p, (200.0, 100.0), 400.0, 300.0);
        assert_eq!((r.scale, r.width), (2.0, 1.0));
        assert!((r.height - 1.5).abs() < 1e-9);
        // Never below half size.
        assert_eq!(resized(p, (200.0, 100.0), 10.0, 10.0).scale, 0.5);
    }

    #[test]
    fn widgets_stay_on_screen() {
        assert_eq!(clamp_pos((100.0, 50.0), (200.0, 100.0), (1920.0, 1080.0)), (100.0, 50.0));
        assert_eq!(clamp_pos((1900.0, -20.0), (200.0, 100.0), (1920.0, 1080.0)), (1720.0, 0.0));
        assert_eq!(clamp_pos((10.0, 10.0), (3000.0, 100.0), (1920.0, 1080.0)), (0.0, 10.0));
    }

    #[test]
    fn parts_of_day() {
        assert_eq!(part_of_day(6), "Morning");
        assert_eq!(part_of_day(13), "Afternoon");
        assert_eq!(part_of_day(19), "Evening");
        assert_eq!(part_of_day(2), "Night");
    }

    #[test]
    fn countdown_days() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).expect("date");
        assert_eq!(days_until(d(2026, 11, 11), d(2026, 11, 18)), 7);
        assert_eq!(days_until(d(2026, 12, 31), d(2027, 1, 1)), 1);
        assert_eq!(days_until(d(2026, 11, 11), d(2026, 11, 10)), -1);
    }

    #[test]
    fn system_stats() {
        let mem = "MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    9120000 kB\n";
        assert_eq!(ram_percent(mem).map(|p| p.round()), Some(43.0));
        assert_eq!(cpu_times("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2"), Some((150, 1000)));
        assert_eq!(cpu_times("garbage"), None);
        assert_eq!(disk_size(476_000_000_000), "476GB");
        assert_eq!(disk_size(1_900_000_000_000), "1.9TB");
    }
}
