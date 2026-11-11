//! Material 3 Expressive widgets for GTK 4 that CSS alone can't express:
//! the expressive shape set with spring-animated shape morphing, shape icon
//! containers, the expressive loading indicator, wavy progress indicators
//! and the wavy slider — plus [`AppStyle`], the live theme for m3 apps.
//!
//! Colours come from CSS: a [`ShapeView`] paints with its own CSS `color`,
//! so `.m3-shape.primary { color: var(--m3-primary-container); }` themes it.

pub mod app;
pub mod mpris;
pub mod shape;
pub mod wavy;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

pub use app::AppStyle;
pub use shape::Shape;
pub use wavy::{CircularWavyProgress, WaveStyle, WavyProgress, WavySlider};

/// Expressive spatial spring parameters (damping ratio, stiffness).
#[derive(Debug, Clone, Copy)]
pub struct SpringSpec {
    pub damping: f64,
    pub stiffness: f64,
}

impl SpringSpec {
    pub const FAST: SpringSpec = SpringSpec { damping: 0.6, stiffness: 800.0 };
    pub const DEFAULT: SpringSpec = SpringSpec { damping: 0.8, stiffness: 380.0 };
    pub const SLOW: SpringSpec = SpringSpec { damping: 0.8, stiffness: 200.0 };
}

fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

struct MorphState {
    from: [f64; shape::SAMPLES],
    from_rot: f64,
    to: [f64; shape::SAMPLES],
    to_rot: f64,
    target: Shape,
    spring: SpringSpec,
    /// Frame time (µs) the current animation started, `None` until the
    /// first frame.
    start: Option<i64>,
    progress: f64,
    animating: bool,
    /// Continuous rotation in degrees per second (loading indicator).
    spin: f64,
    spin_origin: Option<i64>,
    spin_angle: f64,
    /// Fixed fill colour (RGB) instead of the CSS colour.
    fill: Option<(f64, f64, f64)>,
}

impl MorphState {
    fn current(&self) -> ([f64; shape::SAMPLES], f64) {
        let r = shape::morph(&self.from, &self.to, self.progress);
        let rot = self.from_rot + (self.to_rot - self.from_rot) * self.progress + self.spin_angle;
        (r, rot)
    }
}

/// A drawing area that paints a [`Shape`] in its CSS `color` and morphs
/// between shapes with an expressive spring.
#[derive(Clone)]
pub struct ShapeView {
    area: gtk::DrawingArea,
    state: Rc<RefCell<MorphState>>,
}

impl ShapeView {
    pub fn new(initial: Shape, size: i32) -> Self {
        let area = gtk::DrawingArea::builder().content_width(size).content_height(size).build();
        area.add_css_class("m3-shape");
        let r = initial.radii();
        let state = Rc::new(RefCell::new(MorphState {
            from: r,
            from_rot: 0.0,
            to: r,
            to_rot: 0.0,
            target: initial,
            spring: SpringSpec::DEFAULT,
            start: None,
            progress: 1.0,
            animating: false,
            spin: 0.0,
            spin_origin: None,
            spin_angle: 0.0,
            fill: None,
        }));
        let st = state.clone();
        area.set_draw_func(move |area, cr, w, h| {
            let (r, rot) = st.borrow().current();
            let fill = st.borrow().fill;
            let pts = shape::outline(&r, f64::from(w), f64::from(h), rot);
            let Some(&(x0, y0)) = pts.first() else { return };
            cr.move_to(x0, y0);
            for &(x, y) in &pts[1..] {
                cr.line_to(x, y);
            }
            cr.close_path();
            match fill {
                Some((r, g, b)) => cr.set_source_rgb(r, g, b),
                None => {
                    let c = area.color();
                    cr.set_source_rgba(
                        f64::from(c.red()),
                        f64::from(c.green()),
                        f64::from(c.blue()),
                        f64::from(c.alpha()),
                    );
                }
            }
            let _ = cr.fill();
        });
        Self { area, state }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn shape(&self) -> Shape {
        self.state.borrow().target
    }

    /// Paint with a fixed `0xRRGGBB` colour instead of the CSS `color`.
    pub fn set_fill(&self, rgb: u32) {
        let c = |shift: u32| f64::from((rgb >> shift) & 0xff) / 255.0;
        self.state.borrow_mut().fill = Some((c(16), c(8), c(0)));
        self.area.queue_draw();
    }

    /// Morph to `target`, rotated by `rotation` degrees, with `spring`.
    pub fn morph_to(&self, target: Shape, rotation: f64, spring: SpringSpec) {
        {
            let mut s = self.state.borrow_mut();
            if s.target == target && (s.to_rot - rotation).abs() < f64::EPSILON {
                return;
            }
            let (cur, cur_rot) = s.current();
            s.from = cur;
            s.from_rot = cur_rot - s.spin_angle;
            s.to = target.radii();
            s.to_rot = rotation;
            s.target = target;
            s.spring = spring;
            if !animations_enabled() {
                s.progress = 1.0;
                s.from = s.to;
                s.from_rot = rotation;
                drop(s);
                self.area.queue_draw();
                return;
            }
            s.progress = 0.0;
            s.start = None;
            if s.animating {
                return;
            }
            s.animating = true;
        }
        self.ensure_ticking();
    }

    /// Spin continuously at `deg_per_sec` (0 stops).
    pub fn set_spin(&self, deg_per_sec: f64) {
        let start = {
            let mut s = self.state.borrow_mut();
            s.spin = if animations_enabled() { deg_per_sec } else { 0.0 };
            s.spin_origin = None;
            if s.spin == 0.0 {
                s.spin_angle = 0.0;
            }
            let start = s.spin != 0.0 && !s.animating;
            if start {
                s.animating = true;
            }
            start
        };
        if start {
            self.ensure_ticking();
        }
        self.area.queue_draw();
    }

    fn ensure_ticking(&self) {
        let st = self.state.clone();
        self.area.add_tick_callback(move |area, clock| {
            let now = clock.frame_time();
            let mut s = st.borrow_mut();
            let start = *s.start.get_or_insert(now);
            let t = (now - start) as f64 / 1e6;
            let (p, settled) = shape::spring(t, s.spring.damping, s.spring.stiffness);
            s.progress = p;
            if s.spin != 0.0 {
                let origin = *s.spin_origin.get_or_insert(now);
                s.spin_angle = ((now - origin) as f64 / 1e6 * s.spin) % 360.0;
            }
            let keep = !settled || s.spin != 0.0;
            if !keep {
                s.animating = false;
            }
            drop(s);
            area.queue_draw();
            if keep { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }
}

/// An icon (Material Symbols ligature or text) on an expressive shape.
#[derive(Clone)]
pub struct ShapeIcon {
    overlay: gtk::Overlay,
    shape: ShapeView,
    label: gtk::Label,
}

impl ShapeIcon {
    /// `icon` is a Material Symbols ligature name; pass `symbol = false` to
    /// show plain text (an initial, for example) instead.
    pub fn new(shape: Shape, icon: &str, size: i32, symbol: bool) -> Self {
        let view = ShapeView::new(shape, size);
        let label = gtk::Label::new(Some(icon));
        label.set_halign(gtk::Align::Center);
        label.set_valign(gtk::Align::Center);
        label.set_can_target(false);
        if symbol {
            label.add_css_class("m3-icon");
            label.add_css_class("filled");
        }
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("m3-shape-icon");
        overlay.set_child(Some(view.widget()));
        overlay.add_overlay(&label);
        overlay.set_halign(gtk::Align::Center);
        overlay.set_valign(gtk::Align::Center);
        Self { overlay, shape: view, label }
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.overlay
    }

    pub fn shape(&self) -> &ShapeView {
        &self.shape
    }

    pub fn label(&self) -> &gtk::Label {
        &self.label
    }

    pub fn set_icon(&self, icon: &str) {
        self.label.set_label(icon);
    }
}

/// Morph `view` when `widget` is hovered or pressed: `rest` normally,
/// `hover` under the pointer (rotated a little), `pressed` while held.
pub fn morph_on_interaction(
    widget: &impl IsA<gtk::Widget>,
    view: &ShapeView,
    rest: Shape,
    hover: Shape,
    pressed: Shape,
) {
    let motion = gtk::EventControllerMotion::new();
    let v = view.clone();
    motion.connect_enter(move |_, _, _| v.morph_to(hover, 45.0, SpringSpec::DEFAULT));
    let v = view.clone();
    motion.connect_leave(move |_| v.morph_to(rest, 0.0, SpringSpec::DEFAULT));
    widget.as_ref().add_controller(motion);

    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let v = view.clone();
    click.connect_pressed(move |_, _, _, _| v.morph_to(pressed, 90.0, SpringSpec::FAST));
    let v = view.clone();
    click.connect_released(move |_, _, _, _| v.morph_to(hover, 45.0, SpringSpec::FAST));
    let v = view.clone();
    click.connect_cancel(move |_, _| v.morph_to(rest, 0.0, SpringSpec::DEFAULT));
    widget.as_ref().add_controller(click);
}

/// The M3 Expressive loading indicator: a shape that keeps morphing through
/// the expressive shape sequence while rotating. Animates only while mapped.
#[derive(Clone)]
pub struct LoadingIndicator {
    view: ShapeView,
}

const LOADING_SEQUENCE: [Shape; 7] = [
    Shape::Burst(12),
    Shape::Cookie(9),
    Shape::Pentagon,
    Shape::Square,
    Shape::Sunny(8),
    Shape::Cookie(4),
    Shape::Circle,
];

impl LoadingIndicator {
    pub fn new(size: i32) -> Self {
        let view = ShapeView::new(LOADING_SEQUENCE[0], size);
        view.widget().add_css_class("m3-loading-indicator");
        let source: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        let v = view.clone();
        let src = source.clone();
        view.widget().connect_map(move |_| {
            v.set_spin(140.0);
            if !animations_enabled() {
                return;
            }
            let step = Rc::new(std::cell::Cell::new(0usize));
            let v2 = v.clone();
            let id = glib::timeout_add_local(std::time::Duration::from_millis(650), move || {
                let i = (step.get() + 1) % LOADING_SEQUENCE.len();
                step.set(i);
                v2.morph_to(LOADING_SEQUENCE[i], 0.0, SpringSpec::DEFAULT);
                glib::ControlFlow::Continue
            });
            if let Some(old) = src.borrow_mut().replace(id) {
                old.remove();
            }
        });
        let v = view.clone();
        view.widget().connect_unmap(move |_| {
            v.set_spin(0.0);
            if let Some(id) = source.borrow_mut().take() {
                id.remove();
            }
        });
        Self { view }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        self.view.widget()
    }
}

/// (Re)play a CSS keyframe animation bound to `class` (e.g. `m3-enter`).
///
/// GTK starts a CSS animation when a style starts using it, so the class is
/// removed now and added back two frames later (after a style pass without
/// it). Works for windows that are about to be shown.
pub fn animate(widget: &impl IsA<gtk::Widget>, class: &'static str) {
    let w = widget.as_ref();
    w.remove_css_class(class);
    let frames = std::cell::Cell::new(0u8);
    w.add_tick_callback(move |w, _| {
        frames.set(frames.get() + 1);
        if frames.get() < 2 {
            return glib::ControlFlow::Continue;
        }
        w.add_css_class(class);
        glib::ControlFlow::Break
    });
}
