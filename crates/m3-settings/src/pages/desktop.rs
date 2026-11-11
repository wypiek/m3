//! Desktop: workspaces, windows, panel, hot corner, keyboard shortcuts.

use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};
use m3_core::config::{Binding, ClockFormat, KeyCombo, LayoutKind, PanelPosition, PanelVisibility, default_bindings};
use m3_protocol::Action;

use crate::{Ctx, ui};

fn action_label(a: &Action) -> String {
    use Action::*;
    match a {
        Spawn { command } => format!("Run {}", command.join(" ")),
        SpawnTerminal => "Open terminal".into(),
        CloseWindow => "Close window".into(),
        ToggleMaximize => "Maximize / restore".into(),
        ToggleFullscreen => "Fullscreen".into(),
        Minimize => "Minimize".into(),
        ToggleFloating => "Float / tile window".into(),
        Snap { direction } => format!("Snap {}", format!("{direction:?}").to_lowercase()),
        SwitchWorkspace { index } => format!("Go to workspace {index}"),
        MoveToWorkspace { index } => format!("Move window to workspace {index}"),
        NextWorkspace => "Next workspace".into(),
        PreviousWorkspace => "Previous workspace".into(),
        CycleLayout => "Cycle layout".into(),
        CycleWindows { reverse: false } => "Switch windows".into(),
        CycleWindows { reverse: true } => "Switch windows backwards".into(),
        FocusNext => "Focus next window".into(),
        FocusPrevious => "Focus previous window".into(),
        ToggleOverview => "Overview".into(),
        ToggleLauncher => "Applications".into(),
        ToggleQuickSettings => "Quick settings".into(),
        ToggleNotifications => "Notifications".into(),
        Lock => "Lock screen".into(),
        Screenshot => "Screenshot".into(),
        VolumeUp => "Volume up".into(),
        VolumeDown => "Volume down".into(),
        VolumeMute => "Mute".into(),
        MicMute => "Mute microphone".into(),
        BrightnessUp => "Brightness up".into(),
        BrightnessDown => "Brightness down".into(),
        MediaPlayPause => "Play / pause".into(),
        MediaNext => "Next track".into(),
        MediaPrevious => "Previous track".into(),
        other => format!("{other:?}"),
    }
}

/// Turn a key press into an m3 shortcut string like `Super+Shift+T`.
fn combo_from_event(key: gdk::Key, mods: gdk::ModifierType) -> Option<String> {
    let name = key.to_lower().name()?.to_string();
    if matches!(
        name.as_str(),
        "Super_L"
            | "Super_R"
            | "Shift_L"
            | "Shift_R"
            | "Control_L"
            | "Control_R"
            | "Alt_L"
            | "Alt_R"
            | "Meta_L"
            | "Meta_R"
    ) {
        return None;
    }
    let mut s = String::new();
    if mods.contains(gdk::ModifierType::SUPER_MASK) {
        s.push_str("Super+");
    }
    if mods.contains(gdk::ModifierType::CONTROL_MASK) {
        s.push_str("Ctrl+");
    }
    if mods.contains(gdk::ModifierType::ALT_MASK) {
        s.push_str("Alt+");
    }
    if mods.contains(gdk::ModifierType::SHIFT_MASK) {
        s.push_str("Shift+");
    }
    s.push_str(&name);
    KeyCombo::parse(&s).ok().map(|c| c.to_string())
}

fn capture(ctx: &Rc<Ctx>, button: &gtk::Button, old: KeyCombo, action: Action) {
    let dialog =
        gtk::Window::builder().transient_for(&ctx.window).modal(true).title("New shortcut").default_width(420).build();
    dialog.add_css_class("m3");
    let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
    b.add_css_class("m3-dialog");
    let t = gtk::Label::new(Some(&action_label(&action)));
    t.add_css_class("m3-dialog-title");
    let l =
        gtk::Label::new(Some("Press the new key combination, or Escape to cancel.\nBackspace removes the shortcut."));
    l.add_css_class("m3-dialog-body");
    l.set_justify(gtk::Justification::Center);
    b.append(&t);
    b.append(&l);
    dialog.set_child(Some(&b));
    let keys = gtk::EventControllerKey::new();
    let c = ctx.clone();
    let button = button.clone();
    let d = dialog.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        if key == gdk::Key::Escape {
            d.close();
            return glib::Propagation::Stop;
        }
        let old_s = old.to_string();
        if key == gdk::Key::BackSpace && mods.is_empty() {
            c.keys.update(|k| {
                k.bindings.insert(old_s.clone(), Binding::Disabled(m3_core::config::Disabled::Disabled));
            });
            button.set_label("Disabled");
            d.close();
            return glib::Propagation::Stop;
        }
        let Some(new) = combo_from_event(key, mods) else { return glib::Propagation::Proceed };
        let action = action.clone();
        c.keys.update(|k| {
            // Remove the old binding (explicitly, so inherited defaults go too)
            // and add the new one.
            k.bindings.insert(old_s.clone(), Binding::Disabled(m3_core::config::Disabled::Disabled));
            k.bindings.retain(|combo, _| KeyCombo::parse(combo).ok().map(|x| x.to_string()) != Some(new.clone()));
            k.bindings.insert(new.clone(), Binding::Action(action));
        });
        button.set_label(&new);
        d.close();
        glib::Propagation::Stop
    });
    dialog.add_controller(keys);
    dialog.present();
}

pub fn build(ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("Desktop", "Workspaces, windows, the panel and keyboard shortcuts.");

    // ---- workspaces
    let ws = ctx.workspaces.get();
    let card = ui::card(&content, "Workspaces");
    {
        let c = ctx.clone();
        ui::spin(&card, "Number of workspaces", "", (1.0, 32.0, 1.0), ws.count as f64, move |v| {
            c.workspaces.update(|w| {
                w.count = v as usize;
                w.names.truncate(w.count);
            });
        });
    }
    let names = gtk::Entry::new();
    names.set_text(&ws.names.join(", "));
    names.set_placeholder_text(Some("e.g. Web, Code, Chat"));
    {
        let c = ctx.clone();
        names.connect_changed(move |e| {
            let list: Vec<String> = e.text().split(',').map(|s| s.trim().to_owned()).collect();
            let list = if list.iter().all(|s| s.is_empty()) { Vec::new() } else { list };
            c.workspaces.update(|w| {
                let n = list.len().min(w.count);
                w.names = list[..n].to_vec();
            });
        });
    }
    ui::row(&card, "Names", "Comma separated, in order", &names);
    let layouts = [LayoutKind::Floating, LayoutKind::Tiling, LayoutKind::Monocle];
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Default layout",
            "For new workspaces",
            &["Floating", "Tiling", "Monocle"],
            layouts.iter().position(|l| *l == ws.default_layout).unwrap_or(0),
            move |i| {
                c.workspaces.update(|w| w.default_layout = layouts[i]);
            },
        );
    }
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Wrap around",
            "Going past the last workspace returns to the first",
            ws.wrap_around,
            move |v| c.workspaces.update(|w| w.wrap_around = v),
        );
    }

    // ---- windows
    let comp = ctx.compositor.get();
    let card = ui::card(&content, "Windows");
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Focus follows mouse",
            "Focus windows by hovering them",
            comp.general.focus_follows_mouse,
            move |v| c.compositor.update(|x| x.general.focus_follows_mouse = v),
        );
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Focus new windows", "", comp.general.focus_new_windows, move |v| {
            c.compositor.update(|x| x.general.focus_new_windows = v)
        });
    }
    {
        let c = ctx.clone();
        ui::spin(&card, "Gaps between tiled windows", "", (0.0, 64.0, 1.0), comp.windows.gaps as f64, move |v| {
            c.compositor.update(|x| x.windows.gaps = v as i32)
        });
    }
    {
        let c = ctx.clone();
        ui::slider(
            &card,
            "Main area size",
            "Width of the main window in the tiling layout",
            (0.1, 0.9, 0.05),
            comp.windows.master_ratio,
            ui::pct,
            move |v| c.compositor.update(|x| x.windows.master_ratio = v),
        );
    }
    {
        let c = ctx.clone();
        ui::spin(
            &card,
            "Edge snapping distance",
            "Pixels; 0 turns snapping off",
            (0.0, 128.0, 1.0),
            comp.windows.snap_threshold as f64,
            move |v| c.compositor.update(|x| x.windows.snap_threshold = v as i32),
        );
    }
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Hot corner",
            "Move the pointer to the top-left corner to open the overview",
            comp.general.hot_corner,
            move |v| c.compositor.update(|x| x.general.hot_corner = v),
        );
    }
    title_bar_buttons(&card);
    {
        let c = ctx.clone();
        ui::switch(
            &card,
            "Window animations",
            "Windows spring open; also follows “Reduce motion” in Appearance",
            comp.animations.enabled,
            move |v| c.compositor.update(|x| x.animations.enabled = v),
        );
    }
    {
        let c = ctx.clone();
        ui::slider(&card, "Animation speed", "", (0.5, 2.0, 0.25), comp.animations.speed, ui::times, move |v| {
            c.compositor.update(|x| x.animations.speed = v)
        });
    }

    // ---- panel
    let p = ctx.panels.get();
    let card = ui::card(&content, "Panel");
    let positions = [PanelPosition::Top, PanelPosition::Bottom, PanelPosition::Left, PanelPosition::Right];
    {
        use m3_core::config::PanelStyle;
        let c = ctx.clone();
        let current = if p.style == PanelStyle::Bar { 1 } else { 0 };
        ui::segmented(&card, "Style", &["Islands", "Bar"], current, move |i| {
            c.panels.update(|x| x.style = if i == 1 { PanelStyle::Bar } else { PanelStyle::Islands })
        });
    }
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Position",
            "",
            &["Top", "Bottom", "Left", "Right"],
            positions.iter().position(|x| *x == p.position).unwrap_or(0),
            move |i| c.panels.update(|x| x.position = positions[i]),
        );
    }
    let vis = [PanelVisibility::Always, PanelVisibility::Autohide, PanelVisibility::Intelligent];
    {
        let c = ctx.clone();
        ui::dropdown(
            &card,
            "Visibility",
            "",
            &["Always shown", "Hide automatically", "Hide when a window touches it"],
            vis.iter().position(|x| *x == p.visibility).unwrap_or(0),
            move |i| c.panels.update(|x| x.visibility = vis[i]),
        );
    }
    {
        let c = ctx.clone();
        ui::spin(&card, "Size", "Thickness in pixels", (32.0, 96.0, 2.0), p.size as f64, move |v| {
            c.panels.update(|x| x.size = v as u32)
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Floating", "Detach the panel from the screen edge", p.floating, move |v| {
            c.panels.update(|x| x.floating = v)
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "On every display", "", p.all_outputs, move |v| c.panels.update(|x| x.all_outputs = v));
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "24-hour clock", "", p.clock_format == ClockFormat::H24, move |v| {
            c.panels.update(|x| x.clock_format = if v { ClockFormat::H24 } else { ClockFormat::H12 })
        });
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Show seconds", "", p.clock_seconds, move |v| c.panels.update(|x| x.clock_seconds = v));
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Show date", "", p.clock_date, move |v| c.panels.update(|x| x.clock_date = v));
    }
    {
        let c = ctx.clone();
        ui::switch(&card, "Battery percentage", "", p.battery_percentage, move |v| {
            c.panels.update(|x| x.battery_percentage = v)
        });
    }

    // ---- shortcuts
    let card = ui::card(&content, "Keyboard shortcuts");
    let effective = ctx.keys.get().effective();
    let mut list: Vec<(KeyCombo, Action)> = effective;
    // Keep the default order where possible.
    let order: Vec<String> =
        default_bindings().into_iter().map(|(c, _)| KeyCombo::parse(&c).map(|k| k.to_string()).unwrap_or(c)).collect();
    list.sort_by_key(|(c, _)| order.iter().position(|o| *o == c.to_string()).unwrap_or(usize::MAX));
    for (combo, action) in list {
        let b = gtk::Button::with_label(&combo.to_string());
        b.add_css_class("m3-shortcut");
        let c = ctx.clone();
        let combo2 = combo.clone();
        let a2 = action.clone();
        b.connect_clicked(move |b| capture(&c, b, combo2.clone(), a2.clone()));
        ui::row(&card, &action_label(&action), "", &b);
    }
    let reset = gtk::Button::with_label("Reset all shortcuts");
    reset.add_css_class("outlined");
    {
        let c = ctx.clone();
        reset.connect_clicked(move |_| {
            c.keys.update(|k| {
                k.bindings.clear();
                k.inherit_defaults = true;
            });
            c.toast("Shortcuts reset — reopen this page to see the defaults.");
        });
    }
    ui::row(&card, "Defaults", "Undo all changes to shortcuts", &reset);
    page.upcast()
}

/// "Title bar buttons" — the shared `button-layout` setting apps read.
fn title_bar_buttons(card: &gtk::Box) {
    const SCHEMA: &str = "org.gnome.desktop.wm.preferences";
    let Some(source) = gtk::gio::SettingsSchemaSource::default() else { return };
    if source.lookup(SCHEMA, true).is_none() {
        return;
    }
    let settings = gtk::gio::Settings::new(SCHEMA);
    let layouts = [
        ("Minimize, maximize and close", "appmenu:minimize,maximize,close"),
        ("Minimize and close", "appmenu:minimize,close"),
        ("Close only", "appmenu:close"),
    ];
    let current = settings.string("button-layout");
    let index = layouts.iter().position(|l| l.1 == current.as_str()).unwrap_or(0);
    let labels: Vec<&str> = layouts.iter().map(|l| l.0).collect();
    ui::dropdown(card, "Title bar buttons", "For apps that draw their own title bars", &labels, index, move |i| {
        if let Err(e) = settings.set_string("button-layout", layouts[i].1) {
            tracing::warn!("cannot set the title-bar button layout: {e}");
        }
    });
}
