//! On-screen displays: a small pill at the bottom of the screen for volume,
//! brightness, microphone, keyboard locks, device and workspace changes.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::{glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

pub struct Osd {
    window: gtk::Window,
    revealer: gtk::Revealer,
    icon: m3_widgets::ShapeIcon,
    label: gtk::Label,
    bar: gtk::ProgressBar,
    timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Osd {
    pub fn new(app: &gtk::Application) -> Self {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("m3-osd"));
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, 96);
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::None);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-osd-window");
        window.set_can_target(false);

        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        pill.add_css_class("m3-osd");
        let icon = m3_widgets::ShapeIcon::new(m3_widgets::Shape::Cookie(9), "volume_up", 48, true);
        let label = gtk::Label::new(None);
        label.add_css_class("m3-title-medium-emphasized");
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("m3-osd-bar");
        bar.set_size_request(220, -1);
        bar.set_valign(gtk::Align::Center);
        pill.append(icon.widget());
        pill.append(&bar);
        pill.append(&label);
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        revealer.set_transition_duration(200);
        revealer.set_child(Some(&pill));
        // Layer surfaces not anchored on opposite edges must never be 0 px
        // wide/tall (protocol error), even while a revealer is collapsed.
        revealer.set_size_request(1, 1);
        window.set_child(Some(&revealer));
        Self { window, revealer, icon, label, bar, timer: Rc::new(RefCell::new(None)) }
    }

    /// Show `icon` with a level (0..1) or just a text label.
    pub fn show(&self, icon: &str, level: Option<f64>, text: &str) {
        // A new kind of OSD gets a fresh shape spin, the expressive way.
        if self.icon.label().text() != icon {
            let next = if self.icon.shape().shape() == m3_widgets::Shape::Cookie(9) {
                m3_widgets::Shape::Cookie(12)
            } else {
                m3_widgets::Shape::Cookie(9)
            };
            self.icon.shape().morph_to(next, 0.0, m3_widgets::SpringSpec::DEFAULT);
        }
        self.icon.set_icon(icon);
        self.bar.set_visible(level.is_some());
        if let Some(l) = level {
            self.bar.set_fraction(l.clamp(0.0, 1.0));
            self.label.set_text(&format!("{:.0}", l * 100.0));
        } else {
            self.label.set_text(text);
        }
        self.window.update_property(&[gtk::accessible::Property::Label(text)]);
        if !self.window.is_visible() {
            if let Some(pill) = self.revealer.child() {
                m3_widgets::animate(&pill, "m3-enter-up");
            }
            self.window.present();
        }
        self.revealer.set_reveal_child(true);
        if let Some(t) = self.timer.borrow_mut().take() {
            t.remove();
        }
        let revealer = self.revealer.clone();
        let window = self.window.clone();
        let timer = self.timer.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(1500), move || {
            // This source is finished; forget its id so it is never removed twice.
            timer.borrow_mut().take();
            revealer.set_reveal_child(false);
            glib::timeout_add_local_once(Duration::from_millis(250), move || {
                if !revealer.reveals_child() {
                    window.set_visible(false);
                }
            });
        });
        *self.timer.borrow_mut() = Some(id);
    }
}
