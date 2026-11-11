//! Where shell sheets (quick settings, notifications, popovers) appear,
//! derived from the panel's position so they open next to the panel.

use std::cell::Cell;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, LayerShell};
use m3_core::config::PanelPosition;

thread_local! {
    static PANEL: Cell<PanelPosition> = const { Cell::new(PanelPosition::Bottom) };
}

pub fn set_panel_position(p: PanelPosition) {
    PANEL.with(|c| c.set(p));
}

pub fn panel_position() -> PanelPosition {
    PANEL.with(Cell::get)
}

/// The side a popover opens on: away from the panel's screen edge.
pub fn popover_side() -> gtk::PositionType {
    match panel_position() {
        PanelPosition::Top => gtk::PositionType::Bottom,
        PanelPosition::Bottom => gtk::PositionType::Top,
        PanelPosition::Left => gtk::PositionType::Right,
        PanelPosition::Right => gtk::PositionType::Left,
    }
}

/// Screen edges a sheet is anchored to: the panel's edge plus the end
/// (right for horizontal panels, bottom for vertical ones) where the status
/// area lives.
pub fn sheet_edges(p: PanelPosition) -> [Edge; 2] {
    match p {
        PanelPosition::Top => [Edge::Top, Edge::Right],
        PanelPosition::Bottom => [Edge::Bottom, Edge::Right],
        PanelPosition::Left => [Edge::Left, Edge::Bottom],
        PanelPosition::Right => [Edge::Right, Edge::Bottom],
    }
}

/// Revealer transition that slides out of the panel.
pub fn reveal_transition(p: PanelPosition) -> gtk::RevealerTransitionType {
    match p {
        PanelPosition::Top => gtk::RevealerTransitionType::SlideDown,
        PanelPosition::Bottom => gtk::RevealerTransitionType::SlideUp,
        PanelPosition::Left => gtk::RevealerTransitionType::SlideRight,
        PanelPosition::Right => gtk::RevealerTransitionType::SlideLeft,
    }
}

/// CSS entrance animation matching the direction (see components.css).
pub fn enter_class(p: PanelPosition) -> &'static str {
    match p {
        PanelPosition::Bottom => "m3-enter-up",
        _ => "m3-enter",
    }
}

/// Lay a sheet out next to the panel. The sheet window covers the usable
/// area (the panel's exclusive zone keeps the panel itself clickable) and is
/// transparent; the sheet sits in the corner by the panel's status area with
/// an 8 px gap. Clicks on the transparent part dismiss it (see
/// [`dismiss_on_outside_click`]).
pub fn anchor_sheet(window: &gtk::Window, revealer: &gtk::Revealer, p: PanelPosition) {
    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(e, true);
        window.set_margin(e, 0);
    }
    window.set_exclusive_zone(0);
    revealer.set_transition_type(reveal_transition(p));
    let [a, b] = sheet_edges(p);
    let align = |e: Edge| match e {
        Edge::Top | Edge::Left => gtk::Align::Start,
        _ => gtk::Align::End,
    };
    for e in [a, b] {
        match e {
            Edge::Top | Edge::Bottom => revealer.set_valign(align(e)),
            _ => revealer.set_halign(align(e)),
        }
    }
    revealer.set_margin_top(8);
    revealer.set_margin_bottom(8);
    revealer.set_margin_start(8);
    revealer.set_margin_end(8);
}

/// Hide a sheet when the user clicks outside of `sheet` in its window.
pub fn dismiss_on_outside_click(window: &gtk::Window, sheet: &impl IsA<gtk::Widget>, hide: impl Fn() + 'static) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    let sheet = sheet.as_ref().clone();
    click.connect_pressed(move |g, _, x, y| {
        let Some(w) = g.widget() else { return };
        let inside =
            w.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|picked| picked == sheet || picked.is_ancestor(&sheet));
        if !inside {
            hide();
        }
    });
    window.add_controller(click);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheets_open_next_to_the_panel() {
        assert_eq!(sheet_edges(PanelPosition::Bottom), [Edge::Bottom, Edge::Right]);
        assert_eq!(sheet_edges(PanelPosition::Left)[0], Edge::Left);
        assert_eq!(reveal_transition(PanelPosition::Bottom), gtk::RevealerTransitionType::SlideUp);
        assert_eq!(enter_class(PanelPosition::Bottom), "m3-enter-up");
    }

    #[test]
    fn popovers_open_away_from_the_edge() {
        set_panel_position(PanelPosition::Bottom);
        assert_eq!(popover_side(), gtk::PositionType::Top);
        set_panel_position(PanelPosition::Top);
        assert_eq!(popover_side(), gtk::PositionType::Bottom);
    }
}
