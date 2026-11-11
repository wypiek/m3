//! Input processing: keyboard shortcuts, pointer focus, click-to-focus,
//! compositor-initiated move/resize, scrolling, gestures and touch.

use m3_core::config::{KeyCombo, Keybindings};
use m3_protocol::Action;
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent, GestureEndEvent,
        GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent, TouchEvent,
    },
    desktop::{WindowSurfaceType, layer_map_for_output},
    input::{
        keyboard::{FilterResult, Keycode, Keysym, KeysymHandle, ModifiersState, xkb},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, RelativeMotionEvent,
        },
    },
    output::Output,
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::{
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
        seat::WaylandFocus,
        shell::wlr_layer::{KeyboardInteractivity, Layer},
    },
};
use tracing::{debug, warn};

use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    grabs::{self, BTN_LEFT, BTN_RIGHT, ResizeEdge},
    state::State,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
}

impl From<&ModifiersState> for Mods {
    fn from(m: &ModifiersState) -> Self {
        Self { ctrl: m.ctrl, alt: m.alt, shift: m.shift, logo: m.logo }
    }
}

/// Keybindings resolved to keysyms.
#[derive(Debug, Default)]
pub struct Bindings {
    entries: Vec<(Mods, Keysym, Action)>,
}

impl Bindings {
    pub fn from_config(kb: &Keybindings) -> Self {
        let mut entries = Vec::new();
        for (combo, action) in kb.effective() {
            match resolve(&combo) {
                Some(sym) => {
                    let m = combo.modifiers;
                    entries.push((Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, logo: m.logo }, sym, action));
                }
                None => warn!("keybinding `{combo}`: unknown key name `{}`", combo.key),
            }
        }
        Self { entries }
    }

    fn find(&self, mods: Mods, syms: &[Keysym]) -> Option<Action> {
        self.entries
            .iter()
            .find(|(m, sym, _)| *m == mods && syms.iter().any(|s| s == sym || lower(*s) == *sym))
            .map(|(_, _, a)| a.clone())
    }
}

fn resolve(combo: &KeyCombo) -> Option<Keysym> {
    let sym = xkb::keysym_from_name(&combo.key, xkb::KEYSYM_NO_FLAGS);
    let sym = if sym.raw() == 0 { xkb::keysym_from_name(&combo.key, xkb::KEYSYM_CASE_INSENSITIVE) } else { sym };
    (sym.raw() != 0).then_some(sym)
}

/// Lowercase Latin letters so `Super+Q` matches whether or not Shift/Caps
/// changed the produced keysym.
fn lower(sym: Keysym) -> Keysym {
    let raw = sym.raw();
    if (0x41..=0x5a).contains(&raw) { Keysym::new(raw + 0x20) } else { sym }
}

enum KeyAction {
    Action(Action),
    VtSwitch(i32),
}

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>)
    where
        I::Device: 'static,
    {
        self.m3.idle_notifier_state.notify_activity(&self.m3.seat);
        self.m3.last_activity = std::time::Instant::now();
        self.wake_displays();
        match event {
            InputEvent::Keyboard { event } => {
                self.on_key(event.key_code(), event.state(), event.time_msec());
            }
            InputEvent::PointerMotion { event } => {
                self.on_pointer_motion_relative(event.delta(), event.delta_unaccel(), event.time());
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let Some(output) = self.m3.shell.outputs.first().cloned() else { return };
                let Some(geo) = self.m3.shell.output_geometry(&output) else { return };
                let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
                self.on_pointer_motion_absolute(pos, event.time_msec());
            }
            InputEvent::PointerButton { event } => {
                self.on_pointer_button(event.button_code(), event.state(), event.time_msec());
            }
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<I>(&event),
            InputEvent::GestureSwipeBegin { event } => {
                let p = self.m3.seat.get_pointer();
                if let Some(p) = p {
                    p.gesture_swipe_begin(
                        self,
                        &GestureSwipeBeginEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GestureSwipeUpdate { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_swipe_update(
                        self,
                        &GestureSwipeUpdateEvent { time: event.time_msec(), delta: event.delta() },
                    );
                }
            }
            InputEvent::GestureSwipeEnd { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_swipe_end(
                        self,
                        &GestureSwipeEndEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            cancelled: event.cancelled(),
                        },
                    );
                }
            }
            InputEvent::GesturePinchBegin { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_pinch_begin(
                        self,
                        &GesturePinchBeginEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GesturePinchUpdate { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_pinch_update(
                        self,
                        &GesturePinchUpdateEvent {
                            time: event.time_msec(),
                            delta: event.delta(),
                            scale: event.scale(),
                            rotation: event.rotation(),
                        },
                    );
                }
            }
            InputEvent::GesturePinchEnd { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_pinch_end(
                        self,
                        &GesturePinchEndEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            cancelled: event.cancelled(),
                        },
                    );
                }
            }
            InputEvent::GestureHoldBegin { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_hold_begin(
                        self,
                        &GestureHoldBeginEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GestureHoldEnd { event } => {
                if let Some(p) = self.m3.seat.get_pointer() {
                    p.gesture_hold_end(
                        self,
                        &GestureHoldEndEvent {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            cancelled: event.cancelled(),
                        },
                    );
                }
            }
            InputEvent::TouchDown { event } => self.on_touch_down::<I>(&event),
            InputEvent::TouchMotion { event } => self.on_touch_motion::<I>(&event),
            InputEvent::TouchUp { event } => {
                if let Some(t) = self.m3.seat.get_touch() {
                    t.up(
                        self,
                        &smithay::input::touch::UpEvent {
                            slot: event.slot(),
                            time: event.time_msec(),
                            serial: SERIAL_COUNTER.next_serial(),
                        },
                    );
                }
            }
            InputEvent::TouchFrame { .. } => {
                if let Some(t) = self.m3.seat.get_touch() {
                    t.frame(self);
                }
            }
            InputEvent::TouchCancel { .. } => {
                if let Some(t) = self.m3.seat.get_touch() {
                    t.cancel(self);
                }
            }
            InputEvent::DeviceAdded { device } => self.backend.device_added(&mut self.m3, device),
            InputEvent::DeviceRemoved { .. } => {}
            _ => {}
        }
    }

    // ---- keyboard --------------------------------------------------------

    pub fn on_key(&mut self, keycode: Keycode, key_state: KeyState, time: u32) {
        let Some(keyboard) = self.m3.seat.get_keyboard() else { return };
        let serial = SERIAL_COUNTER.next_serial();
        let raw = keycode.raw();
        let locked = self.m3.lock.is_locked();
        let inhibited = self
            .keyboard_focus()
            .and_then(|f| f.wl_surface().map(|s| s.into_owned()))
            .and_then(|s| self.m3.seat.keyboard_shortcuts_inhibitor_for_surface(&s))
            .is_some_and(|i| i.is_active());

        let result =
            keyboard.input::<Option<KeyAction>, _>(self, keycode, key_state, serial, time, |state, mods, handle| {
                if key_state == KeyState::Released {
                    if state.m3.suppressed_keys.remove(&raw) {
                        return FilterResult::Intercept(None);
                    }
                    return FilterResult::Forward;
                }
                // VT switching works even while locked or inhibited.
                let modified = handle.modified_sym().raw();
                if (xkb::keysyms::KEY_XF86Switch_VT_1..=xkb::keysyms::KEY_XF86Switch_VT_12).contains(&modified) {
                    state.m3.suppressed_keys.insert(raw);
                    let vt = (modified - xkb::keysyms::KEY_XF86Switch_VT_1 + 1) as i32;
                    return FilterResult::Intercept(Some(KeyAction::VtSwitch(vt)));
                }
                if locked || inhibited {
                    return FilterResult::Forward;
                }
                let syms = binding_syms(&handle);
                if let Some(action) = state.m3.bindings.find(Mods::from(mods), &syms) {
                    state.m3.suppressed_keys.insert(raw);
                    return FilterResult::Intercept(Some(KeyAction::Action(action)));
                }
                FilterResult::Forward
            });

        match result {
            Some(Some(KeyAction::Action(a))) => self.do_action(a),
            Some(Some(KeyAction::VtSwitch(vt))) => self.backend.switch_vt(vt),
            _ => {}
        }

        // Releasing Alt ends an Alt+Tab cycle.
        if key_state == KeyState::Released && self.m3.cycle.is_some() {
            let mods = keyboard.modifier_state();
            if !mods.alt && !mods.logo {
                self.end_cycle();
            }
        }
    }

    // ---- pointer -----------------------------------------------------------

    pub fn pointer_location(&self) -> Point<f64, Logical> {
        self.m3.seat.get_pointer().map(|p| p.current_location()).unwrap_or_default()
    }

    pub fn pointer_output(&self) -> Option<Output> {
        let p = self.pointer_location();
        self.m3.shell.output_at(p).cloned().or_else(|| self.m3.shell.outputs.first().cloned())
    }

    /// Clamp a position to the nearest point on any output.
    fn clamp_to_outputs(&self, p: Point<f64, Logical>) -> Point<f64, Logical> {
        if self.m3.shell.output_at(p).is_some() {
            return p;
        }
        let mut best: Option<(f64, Point<f64, Logical>)> = None;
        for o in &self.m3.shell.outputs {
            let Some(g) = self.m3.shell.output_geometry(o) else { continue };
            let g = g.to_f64();
            let c = Point::from((
                p.x.clamp(g.loc.x, g.loc.x + g.size.w - 1.0),
                p.y.clamp(g.loc.y, g.loc.y + g.size.h - 1.0),
            ));
            let d = (c.x - p.x).powi(2) + (c.y - p.y).powi(2);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, c));
            }
        }
        best.map(|(_, c)| c).unwrap_or(p)
    }

    /// What is under the pointer at `pos`, in stacking order.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        let output = self.m3.shell.output_at(pos)?;
        let og = self.m3.shell.output_geometry(output)?;
        let rel = pos - og.loc.to_f64();

        if self.m3.lock.is_locked() {
            return self
                .m3
                .lock
                .surface_for(output)
                .map(|s| (PointerFocusTarget::Surface(s.wl_surface().clone()), og.loc.to_f64()));
        }

        let map = layer_map_for_output(output);
        let layer_hit = |layer: Layer| {
            map.layer_under(layer, rel).and_then(|l| {
                let lg = map.layer_geometry(l)?;
                l.surface_under(rel - lg.loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (PointerFocusTarget::Surface(s), (p + lg.loc + og.loc).to_f64()))
            })
        };

        if let Some(hit) = layer_hit(Layer::Overlay) {
            return Some(hit);
        }
        let fullscreen = self.m3.shell.fullscreen_on(output);
        if fullscreen.is_none() {
            if let Some(hit) = layer_hit(Layer::Top) {
                return Some(hit);
            }
        }
        for w in self.m3.shell.override_redirect.iter().rev() {
            if let Some(x) = w.x11_surface() {
                if x.geometry().to_f64().contains(pos) {
                    return Some((PointerFocusTarget::X11(x.clone()), x.geometry().loc.to_f64()));
                }
            }
        }
        let space = self.m3.shell.active_space();
        if let Some((w, render_loc)) = space.element_under(pos) {
            if let Some(x) = w.x11_surface() {
                return Some((PointerFocusTarget::X11(x.clone()), render_loc.to_f64()));
            }
            if let Some((s, p)) = w.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((PointerFocusTarget::Surface(s), (p + render_loc).to_f64()));
            }
        }
        if fullscreen.is_none() {
            for layer in [Layer::Bottom, Layer::Background] {
                if let Some(hit) = layer_hit(layer) {
                    return Some(hit);
                }
            }
        }
        None
    }

    /// Re-send pointer focus for the current position (after windows moved,
    /// workspaces switched, etc.).
    pub fn update_pointer_focus(&mut self) {
        let Some(pointer) = self.m3.seat.get_pointer() else { return };
        if pointer.is_grabbed() {
            return;
        }
        let pos = pointer.current_location();
        let under = self.surface_under(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time: self.now_ms() },
        );
        pointer.frame(self);
    }

    pub fn now_ms(&self) -> u32 {
        self.m3.uptime().as_millis() as u32
    }

    pub fn on_pointer_motion_absolute(&mut self, pos: Point<f64, Logical>, time: u32) {
        let pos = self.clamp_to_outputs(pos);
        self.pointer_moved_to(pos, None, time as u64 * 1000);
    }

    pub fn on_pointer_motion_relative(
        &mut self,
        delta: Point<f64, Logical>,
        delta_unaccel: Point<f64, Logical>,
        utime: u64,
    ) {
        let Some(pointer) = self.m3.seat.get_pointer() else { return };
        let current = pointer.current_location();

        // Pointer constraints (games, 3D viewports).
        let mut locked = false;
        let mut confine_region = None;
        if let Some((focus, origin)) = self.surface_under(current) {
            if let Some(surface) = focus.wl_surface() {
                with_pointer_constraint(&surface, &pointer, |c| match c {
                    Some(c) if c.is_active() => match &*c {
                        PointerConstraint::Locked(_) => locked = true,
                        PointerConstraint::Confined(conf) => {
                            confine_region = Some((origin, conf.region().cloned(), surface.clone().into_owned()))
                        }
                    },
                    _ => {}
                });
            }
        }

        let rel = (delta, delta_unaccel);
        if locked {
            let under = self.surface_under(current);
            pointer.relative_motion(self, under, &RelativeMotionEvent { delta: rel.0, delta_unaccel: rel.1, utime });
            pointer.frame(self);
            return;
        }
        let mut target = self.clamp_to_outputs(current + delta);
        if let Some((origin, region, surface)) = confine_region {
            let inside = |p: Point<f64, Logical>| {
                let local = (p - origin).to_i32_round();
                let in_surface =
                    self.surface_under(p).is_some_and(|(f, _)| f.wl_surface().as_deref() == Some(&surface));
                in_surface && region.as_ref().is_none_or(|r| r.contains(local))
            };
            if !inside(target) {
                target = current;
            }
        }
        self.pointer_moved_to(target, Some(rel), utime);
    }

    fn pointer_moved_to(
        &mut self,
        pos: Point<f64, Logical>,
        rel: Option<(Point<f64, Logical>, Point<f64, Logical>)>,
        utime: u64,
    ) {
        let Some(pointer) = self.m3.seat.get_pointer() else { return };
        let under = self.surface_under(pos);
        let time = (utime / 1000) as u32;

        if self.m3.config.compositor.general.focus_follows_mouse && !pointer.is_grabbed() {
            if let Some((PointerFocusTarget::Surface(s), _)) = &under {
                if let Some((ws, w)) = self.m3.shell.mapped_window_for_surface(&crate::handlers::root_surface(s)) {
                    if ws == self.m3.shell.active && self.focused_window().as_ref() != Some(&w) {
                        self.set_keyboard_focus(Some(KeyboardFocusTarget::Window(w)));
                    }
                }
            }
        }

        pointer.motion(self, under.clone(), &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
        if let Some((d, du)) = rel {
            pointer.relative_motion(self, under, &RelativeMotionEvent { delta: d, delta_unaccel: du, utime });
        }
        pointer.frame(self);
        self.check_hot_corner(pos);
        self.m3.queue_redraw_all();
    }

    fn check_hot_corner(&mut self, pos: Point<f64, Logical>) {
        if !self.m3.config.compositor.general.hot_corner || self.m3.lock.is_locked() {
            return;
        }
        let Some(o) = self.m3.shell.output_at(pos) else { return };
        let Some(g) = self.m3.shell.output_geometry(o) else { return };
        let (dx, dy) = (pos.x - g.loc.x as f64, pos.y - g.loc.y as f64);
        if dx <= 1.0 && dy <= 1.0 {
            if self.m3.hot_corner_armed {
                self.m3.hot_corner_armed = false;
                self.do_action(Action::ToggleOverview);
            }
        } else if dx > 24.0 || dy > 24.0 {
            self.m3.hot_corner_armed = true;
        }
    }

    pub fn on_pointer_button(&mut self, button: u32, state: ButtonState, time: u32) {
        let Some(pointer) = self.m3.seat.get_pointer() else { return };
        let serial = SERIAL_COUNTER.next_serial();

        if state == ButtonState::Pressed && !pointer.is_grabbed() && !self.m3.lock.is_locked() {
            let pos = pointer.current_location();
            let mods = self.m3.seat.get_keyboard().map(|k| k.modifier_state()).unwrap_or_default();
            let window = self.m3.shell.active_space().element_under(pos).map(|(w, _)| w.clone());

            // Super + drag: compositor-driven move / resize.
            if mods.logo {
                if let Some(w) = &window {
                    if button == BTN_LEFT {
                        let seat = self.m3.seat.clone();
                        grabs::start_move(self, &seat, w, serial, false);
                        return;
                    }
                    if button == BTN_RIGHT {
                        let edges = self.edges_for(w, pos);
                        let seat = self.m3.seat.clone();
                        self.focus_window(w);
                        grabs::start_resize(self, &seat, w, serial, edges, false);
                        return;
                    }
                }
            }

            // Click to focus.
            match self.surface_under(pos) {
                Some((PointerFocusTarget::Surface(s), _)) => {
                    let root = crate::handlers::root_surface(&s);
                    if let Some((ws, w)) = self.m3.shell.mapped_window_for_surface(&root) {
                        if ws == self.m3.shell.active {
                            self.focus_window(&w);
                        }
                    } else if let Some(layer) = self.layer_for_surface(&root) {
                        if layer.cached_state().keyboard_interactivity != KeyboardInteractivity::None {
                            self.set_keyboard_focus(Some(KeyboardFocusTarget::Layer(layer)));
                        }
                    }
                }
                Some((PointerFocusTarget::X11(_), _)) => {
                    if let Some(w) = window {
                        self.focus_window(&w);
                    }
                }
                None => {}
            }
        }

        pointer.button(self, &ButtonEvent { button, state, serial, time });
        pointer.frame(self);
    }

    fn layer_for_surface(
        &self,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) -> Option<smithay::desktop::LayerSurface> {
        self.m3
            .shell
            .outputs
            .iter()
            .find_map(|o| layer_map_for_output(o).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned())
    }

    /// Pick resize edges from the pointer's quadrant within the window.
    fn edges_for(&self, w: &smithay::desktop::Window, pos: Point<f64, Logical>) -> ResizeEdge {
        let Some(g) = self.m3.shell.active_space().element_geometry(w) else { return ResizeEdge::BOTTOM_RIGHT };
        let left = pos.x < (g.loc.x + g.size.w / 2) as f64;
        let top = pos.y < (g.loc.y + g.size.h / 2) as f64;
        match (left, top) {
            (true, true) => ResizeEdge::TOP_LEFT,
            (true, false) => ResizeEdge::BOTTOM_LEFT,
            (false, true) => ResizeEdge::TOP_RIGHT,
            (false, false) => ResizeEdge::BOTTOM_RIGHT,
        }
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: &I::PointerAxisEvent) {
        let source = event.source();
        let factor =
            if source == AxisSource::Finger { self.m3.config.compositor.input.touchpad.scroll_factor } else { 1.0 };
        let amount =
            |axis| event.amount(axis).unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0) * factor;
        let (h, v) = (amount(Axis::Horizontal), amount(Axis::Vertical));
        let mut frame = AxisFrame::new(event.time_msec()).source(source);
        if h != 0.0 {
            frame = frame.value(Axis::Horizontal, h);
            if let Some(d) = event.amount_v120(Axis::Horizontal) {
                frame = frame.v120(Axis::Horizontal, d as i32);
            }
        }
        if v != 0.0 {
            frame = frame.value(Axis::Vertical, v);
            if let Some(d) = event.amount_v120(Axis::Vertical) {
                frame = frame.v120(Axis::Vertical, d as i32);
            }
        }
        if source == AxisSource::Finger {
            if event.amount(Axis::Horizontal) == Some(0.0) {
                frame = frame.stop(Axis::Horizontal);
            }
            if event.amount(Axis::Vertical) == Some(0.0) {
                frame = frame.stop(Axis::Vertical);
            }
        }
        if let Some(p) = self.m3.seat.get_pointer() {
            p.axis(self, frame);
            p.frame(self);
        }
    }

    fn touch_position<I: InputBackend, E: AbsolutePositionEvent<I>>(&self, event: &E) -> Option<Point<f64, Logical>> {
        let output = self.pointer_output()?;
        let geo = self.m3.shell.output_geometry(&output)?;
        Some(event.position_transformed(geo.size) + geo.loc.to_f64())
    }

    fn on_touch_down<I: InputBackend>(&mut self, event: &I::TouchDownEvent) {
        let Some(touch) = self.m3.seat.get_touch() else { return };
        let Some(pos) = self.touch_position::<I, _>(event) else { return };
        let under = self.surface_under(pos);
        if let Some((PointerFocusTarget::Surface(s), _)) = &under {
            if let Some((ws, w)) = self.m3.shell.mapped_window_for_surface(&crate::handlers::root_surface(s)) {
                if ws == self.m3.shell.active {
                    self.focus_window(&w);
                }
            }
        }
        touch.down(
            self,
            under,
            &smithay::input::touch::DownEvent {
                slot: event.slot(),
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
            },
        );
    }

    fn on_touch_motion<I: InputBackend>(&mut self, event: &I::TouchMotionEvent) {
        let Some(touch) = self.m3.seat.get_touch() else { return };
        let Some(pos) = self.touch_position::<I, _>(event) else { return };
        let under = self.surface_under(pos);
        touch.motion(
            self,
            under,
            &smithay::input::touch::MotionEvent { slot: event.slot(), location: pos, time: event.time_msec() },
        );
    }

    /// Synthetic input from the IPC test interface.
    pub fn inject_test_input(&mut self, input: m3_protocol::TestInput) {
        let time = self.now_ms();
        self.m3.idle_notifier_state.notify_activity(&self.m3.seat);
        self.m3.last_activity = std::time::Instant::now();
        self.wake_displays();
        match input {
            m3_protocol::TestInput::PointerMotion { x, y } => self.on_pointer_motion_absolute((x, y).into(), time),
            m3_protocol::TestInput::PointerButton { button, pressed } => {
                self.on_pointer_button(button, if pressed { ButtonState::Pressed } else { ButtonState::Released }, time)
            }
            m3_protocol::TestInput::Key { keycode, pressed } => self.on_key(
                Keycode::new(keycode + 8),
                if pressed { KeyState::Pressed } else { KeyState::Released },
                time,
            ),
        }
        debug!("injected test input");
    }
}

/// Keysyms a key press can match bindings with: the unmodified syms and
/// the Latin equivalent (so shortcuts work on non-Latin layouts).
fn binding_syms(handle: &KeysymHandle<'_>) -> Vec<Keysym> {
    let mut v: Vec<Keysym> = handle.raw_syms().to_vec();
    v.push(handle.raw_latin_sym_or_raw_current_sym().unwrap_or(handle.modified_sym()));
    v.push(handle.modified_sym());
    v
}
