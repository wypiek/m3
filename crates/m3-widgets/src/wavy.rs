//! M3 Expressive *wavy* progress indicators and the wavy slider.
//!
//! The active part of the track is a travelling sine wave, the remaining
//! track is flat with a stop indicator at the end. The wave flattens
//! smoothly when paused (`set_waving(false)`), as in M3 Expressive media
//! progress. Colours come from CSS: the active wave uses the widget's
//! `color`; the inactive track uses the same colour at low opacity unless a
//! track colour is set.

use std::cell::{Cell, RefCell};
use std::f64::consts::{PI, TAU};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// Geometry of a wave (logical pixels).
#[derive(Debug, Clone, Copy)]
pub struct WaveStyle {
    pub amplitude: f64,
    pub wavelength: f64,
    pub thickness: f64,
    /// Gap between the active and inactive track (and around a handle).
    pub gap: f64,
    /// Wavelengths travelled per second.
    pub speed: f64,
}

impl Default for WaveStyle {
    fn default() -> Self {
        Self { amplitude: 3.0, wavelength: 40.0, thickness: 4.0, gap: 4.0, speed: 0.6 }
    }
}

/// Points of a sine wave from `x0` to `x1` around `cy` (pure; tested).
pub fn wave_points(x0: f64, x1: f64, cy: f64, style: &WaveStyle, amplitude: f64, phase: f64) -> Vec<(f64, f64)> {
    if x1 <= x0 {
        return Vec::new();
    }
    let steps = ((x1 - x0) / 1.5).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| {
            let x = x0 + (x1 - x0) * i as f64 / steps as f64;
            (x, cy + amplitude * (TAU * x / style.wavelength + phase).sin())
        })
        .collect()
}

/// Where the active track ends for `fraction` in a track `x0..x1`.
pub fn active_end(x0: f64, x1: f64, fraction: f64) -> f64 {
    x0 + (x1 - x0) * fraction.clamp(0.0, 1.0)
}

struct WaveState {
    /// `None` = indeterminate.
    fraction: Option<f64>,
    waving: bool,
    /// Current amplitude scale 0..1 (animates towards `waving`).
    amp: f64,
    phase: f64,
    last_frame: Option<i64>,
    ticking: bool,
    style: WaveStyle,
    track: Option<gtk::gdk::RGBA>,
    /// Draw a slider handle at the active end.
    handle: bool,
    /// The handle is being dragged (drawn thinner).
    pressed: bool,
}

fn rgba(cr: &gtk::cairo::Context, c: &gtk::gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), f64::from(c.alpha()) * alpha);
}

fn draw_linear(area: &gtk::DrawingArea, st: &WaveState, cr: &gtk::cairo::Context, w: f64, h: f64) {
    let color = area.color();
    let s = &st.style;
    let cy = h / 2.0;
    let pad = s.thickness / 2.0 + if st.handle { 2.0 } else { 0.0 };
    let (x0, x1) = (pad, w - pad);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.set_line_width(s.thickness);
    let amp = s.amplitude * st.amp;
    let track = |cr: &gtk::cairo::Context| match &st.track {
        Some(t) => rgba(cr, t, 1.0),
        None => rgba(cr, &color, 0.28),
    };

    match st.fraction {
        Some(f) => {
            let end = active_end(x0, x1, f);
            let handle_w = if st.handle { if st.pressed { 2.0 } else { 4.0 } } else { 0.0 };
            let gap = if st.handle { s.gap + handle_w / 2.0 + 2.0 } else { s.gap };
            // Active wave (skip when it would be shorter than a dot).
            let a_end = end - if st.handle { gap } else { 0.0 };
            if a_end > x0 {
                // Fade the amplitude in over the first wavelength so short
                // progress doesn't look like noise.
                let ramp = ((a_end - x0) / s.wavelength).min(1.0);
                let pts = wave_points(x0, a_end, cy, s, amp * ramp, st.phase);
                if let Some(&(px, py)) = pts.first() {
                    cr.move_to(px, py);
                    for &(px, py) in &pts[1..] {
                        cr.line_to(px, py);
                    }
                    rgba(cr, &color, 1.0);
                    let _ = cr.stroke();
                }
            }
            // Inactive track and stop indicator.
            let i_start = end + gap;
            if i_start < x1 {
                cr.move_to(i_start, cy);
                cr.line_to(x1, cy);
                track(cr);
                let _ = cr.stroke();
            }
            if f < 0.995 {
                cr.arc(x1, cy, s.thickness / 2.0, 0.0, TAU);
                rgba(cr, &color, 1.0);
                let _ = cr.fill();
            }
            if st.handle {
                let hh = (h - 4.0).max(s.thickness * 3.0);
                cr.set_line_width(handle_w);
                cr.move_to(end, cy - hh / 2.0 + handle_w / 2.0);
                cr.line_to(end, cy + hh / 2.0 - handle_w / 2.0);
                rgba(cr, &color, 1.0);
                let _ = cr.stroke();
            }
        }
        None => {
            // Indeterminate: a wavy segment travels along a flat track.
            let span = x1 - x0;
            let t = (st.phase / TAU * 0.35).fract();
            let head = x0 + span * ((t * 1.6).min(1.0));
            let tail = x0 + span * ((t * 1.6 - 0.45).clamp(0.0, 1.0));
            cr.move_to(x0, cy);
            cr.line_to(x1, cy);
            track(cr);
            let _ = cr.stroke();
            if head - tail > 1.0 {
                let pts = wave_points(tail, head, cy, s, amp, st.phase);
                if let Some(&(px, py)) = pts.first() {
                    cr.move_to(px, py);
                    for &(px, py) in &pts[1..] {
                        cr.line_to(px, py);
                    }
                    rgba(cr, &color, 1.0);
                    let _ = cr.stroke();
                }
            }
        }
    }
}

fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

/// Shared animation driver for the wavy widgets.
fn ensure_ticking(area: &gtk::DrawingArea, state: &Rc<RefCell<WaveState>>) {
    {
        let mut s = state.borrow_mut();
        if s.ticking || !area.is_mapped() {
            return;
        }
        if !animations_enabled() {
            s.amp = if s.waving { 1.0 } else { 0.0 };
            drop(s);
            area.queue_draw();
            return;
        }
        s.ticking = true;
        s.last_frame = None;
    }
    let st = state.clone();
    area.add_tick_callback(move |area, clock| {
        let now = clock.frame_time();
        let mut s = st.borrow_mut();
        let dt = s.last_frame.map(|l| (now - l) as f64 / 1e6).unwrap_or(0.0).min(0.1);
        s.last_frame = Some(now);
        let target = if s.waving { 1.0 } else { 0.0 };
        // Ease the amplitude towards its target (~200 ms).
        let k = 1.0 - (-dt * 14.0).exp();
        s.amp += (target - s.amp) * k;
        if (s.amp - target).abs() < 0.002 {
            s.amp = target;
        }
        let moving = s.waving || s.fraction.is_none();
        if moving {
            s.phase = (s.phase + dt * TAU * s.style.speed) % (TAU * 1000.0);
        }
        let keep = moving || (s.amp - target).abs() > 0.0;
        if !keep {
            s.ticking = false;
        }
        drop(s);
        area.queue_draw();
        if keep && area.is_mapped() {
            glib::ControlFlow::Continue
        } else {
            st.borrow_mut().ticking = false;
            glib::ControlFlow::Break
        }
    });
}

fn new_state(fraction: Option<f64>, handle: bool) -> Rc<RefCell<WaveState>> {
    Rc::new(RefCell::new(WaveState {
        fraction,
        waving: true,
        amp: 1.0,
        phase: 0.0,
        last_frame: None,
        ticking: false,
        style: WaveStyle::default(),
        track: None,
        handle,
        pressed: false,
    }))
}

fn wire_area(area: &gtk::DrawingArea, state: &Rc<RefCell<WaveState>>) {
    let st = state.clone();
    area.set_draw_func(move |a, cr, w, h| draw_linear(a, &st.borrow(), cr, f64::from(w), f64::from(h)));
    let st = state.clone();
    area.connect_map(move |a| ensure_ticking(a, &st));
}

/// Linear wavy progress indicator (determinate or indeterminate).
#[derive(Clone)]
pub struct WavyProgress {
    area: gtk::DrawingArea,
    state: Rc<RefCell<WaveState>>,
}

impl WavyProgress {
    /// `fraction = None` makes it indeterminate.
    pub fn new(fraction: Option<f64>) -> Self {
        let area = gtk::DrawingArea::builder().content_height(12).hexpand(true).build();
        area.add_css_class("m3-wavy-progress");
        area.set_accessible_role(gtk::AccessibleRole::ProgressBar);
        let state = new_state(fraction, false);
        wire_area(&area, &state);
        let me = Self { area, state };
        me.update_accessible();
        me
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set_fraction(&self, fraction: Option<f64>) {
        self.state.borrow_mut().fraction = fraction.map(|f| f.clamp(0.0, 1.0));
        self.update_accessible();
        self.area.queue_draw();
        ensure_ticking(&self.area, &self.state);
    }

    /// Wave (playing/loading) or flatten (paused).
    pub fn set_waving(&self, waving: bool) {
        self.state.borrow_mut().waving = waving;
        ensure_ticking(&self.area, &self.state);
    }

    pub fn set_style(&self, style: WaveStyle) {
        self.state.borrow_mut().style = style;
        self.area.set_content_height((style.amplitude * 2.0 + style.thickness + 4.0).ceil() as i32);
        self.area.queue_draw();
    }

    pub fn set_track_color(&self, color: Option<gtk::gdk::RGBA>) {
        self.state.borrow_mut().track = color;
        self.area.queue_draw();
    }

    fn update_accessible(&self) {
        let f = self.state.borrow().fraction;
        if let Some(f) = f {
            self.area.update_property(&[
                gtk::accessible::Property::ValueMin(0.0),
                gtk::accessible::Property::ValueMax(1.0),
                gtk::accessible::Property::ValueNow(f),
            ]);
        }
    }
}

type SeekFn = Rc<dyn Fn(f64)>;

/// A wavy slider (M3 Expressive media seek bar): drag or click to seek,
/// arrow keys step by 5 %. The value is a fraction `0..=1`.
#[derive(Clone)]
pub struct WavySlider {
    area: gtk::DrawingArea,
    state: Rc<RefCell<WaveState>>,
    on_seek: Rc<RefCell<Option<SeekFn>>>,
    dragging: Rc<Cell<bool>>,
}

impl WavySlider {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::builder().content_height(32).hexpand(true).focusable(true).build();
        area.add_css_class("m3-wavy-slider");
        area.set_accessible_role(gtk::AccessibleRole::Slider);
        area.set_cursor_from_name(Some("pointer"));
        let state = new_state(Some(0.0), true);
        state.borrow_mut().style = WaveStyle { amplitude: 3.0, wavelength: 36.0, thickness: 5.0, gap: 4.0, speed: 0.5 };
        wire_area(&area, &state);
        let me = Self { area, state, on_seek: Rc::new(RefCell::new(None)), dragging: Rc::new(Cell::new(false)) };

        let fraction_at = |area: &gtk::DrawingArea, x: f64| {
            let w = f64::from(area.width()).max(1.0);
            ((x - 6.0) / (w - 12.0).max(1.0)).clamp(0.0, 1.0)
        };
        let drag = gtk::GestureDrag::new();
        let m = me.clone();
        drag.connect_drag_begin(move |g, x, _| {
            let Some(area) = g.widget().and_downcast::<gtk::DrawingArea>() else { return };
            m.dragging.set(true);
            m.state.borrow_mut().pressed = true;
            m.set_value_internal(fraction_at(&area, x));
            area.grab_focus();
        });
        let m = me.clone();
        drag.connect_drag_update(move |g, dx, _| {
            let (Some(area), Some((sx, _))) = (g.widget().and_downcast::<gtk::DrawingArea>(), g.start_point()) else {
                return;
            };
            m.set_value_internal(fraction_at(&area, sx + dx));
        });
        let m = me.clone();
        drag.connect_drag_end(move |_, _, _| {
            m.dragging.set(false);
            m.state.borrow_mut().pressed = false;
            m.area.queue_draw();
            m.emit_seek();
        });
        me.area.add_controller(drag);

        let keys = gtk::EventControllerKey::new();
        let m = me.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let v = m.value();
            let nv = match key {
                gtk::gdk::Key::Left | gtk::gdk::Key::Down => v - 0.05,
                gtk::gdk::Key::Right | gtk::gdk::Key::Up => v + 0.05,
                gtk::gdk::Key::Home => 0.0,
                gtk::gdk::Key::End => 1.0,
                _ => return glib::Propagation::Proceed,
            };
            m.set_value_internal(nv);
            m.emit_seek();
            glib::Propagation::Stop
        });
        me.area.add_controller(keys);
        me.update_accessible();
        me
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Called with the new fraction when the user seeks (on release).
    pub fn connect_seek(&self, f: impl Fn(f64) + 'static) {
        *self.on_seek.borrow_mut() = Some(Rc::new(f));
    }

    pub fn value(&self) -> f64 {
        self.state.borrow().fraction.unwrap_or(0.0)
    }

    /// Set the position from the player; ignored while the user drags.
    pub fn set_value(&self, fraction: f64) {
        if !self.dragging.get() {
            self.set_value_internal(fraction);
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.get()
    }

    pub fn set_waving(&self, waving: bool) {
        self.state.borrow_mut().waving = waving;
        ensure_ticking(&self.area, &self.state);
    }

    fn set_value_internal(&self, f: f64) {
        self.state.borrow_mut().fraction = Some(f.clamp(0.0, 1.0));
        self.update_accessible();
        self.area.queue_draw();
    }

    fn emit_seek(&self) {
        let cb = self.on_seek.borrow().clone();
        if let Some(cb) = cb {
            cb(self.value());
        }
    }

    fn update_accessible(&self) {
        self.area.update_property(&[
            gtk::accessible::Property::ValueMin(0.0),
            gtk::accessible::Property::ValueMax(1.0),
            gtk::accessible::Property::ValueNow(self.value()),
        ]);
    }
}

impl Default for WavySlider {
    fn default() -> Self {
        Self::new()
    }
}

/// Circular wavy progress indicator: a wavy ring arc (determinate) or a
/// rotating, growing/shrinking arc (indeterminate).
#[derive(Clone)]
pub struct CircularWavyProgress {
    area: gtk::DrawingArea,
    state: Rc<RefCell<WaveState>>,
}

impl CircularWavyProgress {
    pub fn new(size: i32, fraction: Option<f64>) -> Self {
        let area = gtk::DrawingArea::builder().content_width(size).content_height(size).build();
        area.add_css_class("m3-wavy-progress");
        area.set_accessible_role(gtk::AccessibleRole::ProgressBar);
        let state = new_state(fraction, false);
        state.borrow_mut().style = WaveStyle { amplitude: 1.6, wavelength: 0.0, thickness: 4.0, gap: 6.0, speed: 0.5 };
        let st = state.clone();
        area.set_draw_func(move |a, cr, w, h| {
            let st = st.borrow();
            let color = a.color();
            let (w, h) = (f64::from(w), f64::from(h));
            let s = &st.style;
            let r = w.min(h) / 2.0 - s.thickness / 2.0 - s.amplitude;
            let (cx, cy) = (w / 2.0, h / 2.0);
            // Waves around the ring: ~ one per 9 px of circumference.
            let n = ((TAU * r) / 9.0).round().max(6.0);
            let amp = s.amplitude * st.amp;
            let (start, sweep, rot) = match st.fraction {
                Some(f) => (-PI / 2.0, TAU * f.clamp(0.0, 1.0), 0.0),
                None => {
                    let t = st.phase / TAU;
                    let grow = 0.15 + 0.6 * (0.5 - 0.5 * (t * TAU * 0.7).cos());
                    (-PI / 2.0, TAU * grow, st.phase * 1.4)
                }
            };
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            cr.set_line_width(s.thickness);
            // Track (the rest of the ring, with gaps).
            let gap = (s.gap + s.thickness) / r;
            if sweep < TAU - 2.0 * gap {
                cr.new_path();
                cr.arc(cx, cy, r, start + rot + sweep + gap, start + rot + TAU - gap);
                match &st.track {
                    Some(t) => rgba(cr, t, 1.0),
                    None => rgba(cr, &color, 0.28),
                }
                let _ = cr.stroke();
            }
            if sweep > 0.01 {
                cr.new_path();
                let steps = (sweep * r / 1.5).ceil().max(2.0) as usize;
                for i in 0..=steps {
                    let a = start + rot + sweep * i as f64 / steps as f64;
                    let rr = r + amp * (n * a + st.phase).sin();
                    let (x, y) = (cx + rr * a.cos(), cy + rr * a.sin());
                    if i == 0 { cr.move_to(x, y) } else { cr.line_to(x, y) }
                }
                rgba(cr, &color, 1.0);
                let _ = cr.stroke();
            }
        });
        let st = state.clone();
        area.connect_map(move |a| ensure_ticking(a, &st));
        Self { area, state }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set_fraction(&self, fraction: Option<f64>) {
        self.state.borrow_mut().fraction = fraction;
        self.area.queue_draw();
        ensure_ticking(&self.area, &self.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_stays_within_amplitude() {
        let s = WaveStyle::default();
        let pts = wave_points(0.0, 200.0, 10.0, &s, 3.0, 1.0);
        assert!(pts.len() > 100);
        assert!(pts.iter().all(|&(_, y)| (7.0 - 1e-9..=13.0 + 1e-9).contains(&y)));
        assert_eq!(pts.first().map(|p| p.0), Some(0.0));
        assert_eq!(pts.last().map(|p| p.0), Some(200.0));
    }

    #[test]
    fn flat_when_amplitude_is_zero() {
        let pts = wave_points(0.0, 50.0, 4.0, &WaveStyle::default(), 0.0, 0.3);
        assert!(pts.iter().all(|&(_, y)| y == 4.0));
        assert!(wave_points(5.0, 5.0, 0.0, &WaveStyle::default(), 3.0, 0.0).is_empty());
    }

    #[test]
    fn active_end_clamps() {
        assert_eq!(active_end(10.0, 110.0, 0.5), 60.0);
        assert_eq!(active_end(10.0, 110.0, -1.0), 10.0);
        assert_eq!(active_end(10.0, 110.0, 2.0), 110.0);
    }
}
