//! Responsive layout: the window switches between a wide layout (side bar
//! with labels and search), a medium one (navigation rail) and a narrow one
//! (rail, and rows stack their control under the title).

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Wide,
    Medium,
    Narrow,
}

impl Mode {
    pub fn css_class(self) -> &'static str {
        match self {
            Mode::Wide => "wide",
            Mode::Medium => "medium",
            Mode::Narrow => "narrow",
        }
    }
}

/// The layout for a window `width` logical pixels wide (pure; tested).
pub fn mode_for_width(width: i32) -> Mode {
    match width {
        ..600 => Mode::Narrow,
        600..900 => Mode::Medium,
        _ => Mode::Wide,
    }
}

/// Widest page content, in logical pixels.
pub const CONTENT_MAX: f64 = 760.0;

/// Side margin that centres content of at most [`CONTENT_MAX`] in a
/// viewport `width` wide (pure; tested).
pub fn clamp_margin(width: f64) -> i32 {
    ((width - CONTENT_MAX) / 2.0).max(0.0).floor() as i32
}

/// Controls at least this wide go under the title in the narrow layout.
const STACK_WIDER_THAN: i32 = 140;

thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Wide) };
    static ROWS: RefCell<Vec<(glib::WeakRef<gtk::Box>, glib::WeakRef<gtk::Widget>)>> = const { RefCell::new(Vec::new()) };
}

pub fn mode() -> Mode {
    MODE.with(Cell::get)
}

/// Remember a settings row so it can stack when the window gets narrow.
pub fn register_row(row: &gtk::Box, control: &gtk::Widget) {
    apply_row(row, control, mode());
    ROWS.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|(b, _)| b.upgrade().is_some());
        r.push((row.downgrade(), control.downgrade()));
    });
}

fn apply_row(row: &gtk::Box, control: &gtk::Widget, mode: Mode) {
    let (_, natural, _, _) = control.measure(gtk::Orientation::Horizontal, -1);
    let stack = mode == Mode::Narrow && natural > STACK_WIDER_THAN;
    if stack {
        row.set_orientation(gtk::Orientation::Vertical);
        row.set_spacing(10);
        control.set_halign(gtk::Align::Start);
        row.add_css_class("stacked");
    } else {
        row.set_orientation(gtk::Orientation::Horizontal);
        row.set_spacing(16);
        control.set_halign(gtk::Align::Fill);
        row.remove_css_class("stacked");
    }
}

/// Switch every registered row to `mode`.
pub fn set_mode(mode: Mode) {
    MODE.with(|m| m.set(mode));
    ROWS.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|(b, c)| match (b.upgrade(), c.upgrade()) {
            (Some(b), Some(c)) => {
                apply_row(&b, &c, mode);
                true
            }
            _ => false,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoints() {
        assert_eq!(mode_for_width(360), Mode::Narrow);
        assert_eq!(mode_for_width(599), Mode::Narrow);
        assert_eq!(mode_for_width(600), Mode::Medium);
        assert_eq!(mode_for_width(899), Mode::Medium);
        assert_eq!(mode_for_width(1100), Mode::Wide);
    }

    #[test]
    fn content_is_centred_and_capped() {
        assert_eq!(clamp_margin(500.0), 0);
        assert_eq!(clamp_margin(760.0), 0);
        assert_eq!(clamp_margin(1000.0), 120);
    }
}
