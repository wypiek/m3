//! Keyboard and pointer focus targets.

use std::borrow::Cow;

use smithay::{
    backend::input::KeyState,
    desktop::{LayerSurface, PopupKind, Window, WindowSurface},
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, MotionEvent, PointerTarget, RelativeMotionEvent,
        },
        touch::{DownEvent, OrientationEvent, ShapeEvent, TouchTarget, UpEvent},
    },
    reexports::wayland_server::{Resource, backend::ObjectId, protocol::wl_surface::WlSurface},
    utils::{IsAlive, Serial},
    wayland::{seat::WaylandFocus, session_lock::LockSurface},
    xwayland::X11Surface,
};

use crate::state::State;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardFocusTarget {
    Window(Window),
    Layer(LayerSurface),
    Popup(PopupKind),
    Lock(LockSurface),
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Window(w) => w.alive(),
            Self::Layer(l) => l.alive(),
            Self::Popup(p) => p.alive(),
            Self::Lock(l) => l.alive(),
        }
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(p: PopupKind) -> Self {
        Self::Popup(p)
    }
}

impl From<Window> for KeyboardFocusTarget {
    fn from(w: Window) -> Self {
        Self::Window(w)
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Window(w) => w.wl_surface(),
            Self::Layer(l) => Some(Cow::Borrowed(l.wl_surface())),
            Self::Popup(p) => Some(Cow::Borrowed(p.wl_surface())),
            Self::Lock(l) => Some(Cow::Borrowed(l.wl_surface())),
        }
    }
}

/// Dispatch to the underlying Wayland or X11 keyboard target.
macro_rules! kb_dispatch {
    ($self:ident, $t:ident => $e:expr) => {
        match $self {
            KeyboardFocusTarget::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(s) => {
                    let $t = s.wl_surface();
                    $e
                }
                WindowSurface::X11(s) => {
                    let $t = s;
                    $e
                }
            },
            KeyboardFocusTarget::Layer(l) => {
                let $t = l.wl_surface();
                $e
            }
            KeyboardFocusTarget::Popup(p) => {
                let $t = p.wl_surface();
                $e
            }
            KeyboardFocusTarget::Lock(l) => {
                let $t = l.wl_surface();
                $e
            }
        }
    };
}

impl KeyboardTarget<State> for KeyboardFocusTarget {
    fn enter(&self, seat: &Seat<State>, data: &mut State, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        kb_dispatch!(self, t => KeyboardTarget::enter(t, seat, data, keys, serial))
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        kb_dispatch!(self, t => KeyboardTarget::leave(t, seat, data, serial))
    }
    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        kb_dispatch!(self, t => KeyboardTarget::key(t, seat, data, key, state, serial, time))
    }
    fn modifiers(&self, seat: &Seat<State>, data: &mut State, modifiers: ModifiersState, serial: Serial) {
        kb_dispatch!(self, t => KeyboardTarget::modifiers(t, seat, data, modifiers, serial))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointerFocusTarget {
    Surface(WlSurface),
    X11(X11Surface),
}

impl IsAlive for PointerFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Surface(s) => s.alive(),
            Self::X11(s) => s.alive(),
        }
    }
}

impl WaylandFocus for PointerFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Surface(s) => Some(Cow::Borrowed(s)),
            Self::X11(s) => s.wl_surface().map(Cow::Owned),
        }
    }
    fn same_client_as(&self, object_id: &ObjectId) -> bool {
        match self {
            Self::Surface(s) => s.id().same_client_as(object_id),
            Self::X11(s) => s.same_client_as(object_id),
        }
    }
}

impl From<WlSurface> for PointerFocusTarget {
    fn from(s: WlSurface) -> Self {
        Self::Surface(s)
    }
}

impl From<KeyboardFocusTarget> for PointerFocusTarget {
    fn from(k: KeyboardFocusTarget) -> Self {
        match k {
            KeyboardFocusTarget::Window(w) => match w.underlying_surface() {
                WindowSurface::Wayland(t) => Self::Surface(t.wl_surface().clone()),
                WindowSurface::X11(x) => Self::X11(x.clone()),
            },
            KeyboardFocusTarget::Layer(l) => Self::Surface(l.wl_surface().clone()),
            KeyboardFocusTarget::Popup(p) => Self::Surface(p.wl_surface().clone()),
            KeyboardFocusTarget::Lock(l) => Self::Surface(l.wl_surface().clone()),
        }
    }
}

macro_rules! ptr_dispatch {
    ($self:ident, $t:ident => $e:expr) => {
        match $self {
            PointerFocusTarget::Surface($t) => $e,
            PointerFocusTarget::X11($t) => $e,
        }
    };
}

impl PointerTarget<State> for PointerFocusTarget {
    fn enter(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        ptr_dispatch!(self, t => PointerTarget::enter(t, seat, data, event))
    }
    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &MotionEvent) {
        ptr_dispatch!(self, t => PointerTarget::motion(t, seat, data, event))
    }
    fn relative_motion(&self, seat: &Seat<State>, data: &mut State, event: &RelativeMotionEvent) {
        ptr_dispatch!(self, t => PointerTarget::relative_motion(t, seat, data, event))
    }
    fn button(&self, seat: &Seat<State>, data: &mut State, event: &ButtonEvent) {
        ptr_dispatch!(self, t => PointerTarget::button(t, seat, data, event))
    }
    fn axis(&self, seat: &Seat<State>, data: &mut State, frame: AxisFrame) {
        ptr_dispatch!(self, t => PointerTarget::axis(t, seat, data, frame))
    }
    fn frame(&self, seat: &Seat<State>, data: &mut State) {
        ptr_dispatch!(self, t => PointerTarget::frame(t, seat, data))
    }
    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial, time: u32) {
        ptr_dispatch!(self, t => PointerTarget::leave(t, seat, data, serial, time))
    }
    fn gesture_swipe_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeBeginEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_swipe_begin(t, seat, data, event))
    }
    fn gesture_swipe_update(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeUpdateEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_swipe_update(t, seat, data, event))
    }
    fn gesture_swipe_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureSwipeEndEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_swipe_end(t, seat, data, event))
    }
    fn gesture_pinch_begin(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchBeginEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_pinch_begin(t, seat, data, event))
    }
    fn gesture_pinch_update(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchUpdateEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_pinch_update(t, seat, data, event))
    }
    fn gesture_pinch_end(&self, seat: &Seat<State>, data: &mut State, event: &GesturePinchEndEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_pinch_end(t, seat, data, event))
    }
    fn gesture_hold_begin(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldBeginEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_hold_begin(t, seat, data, event))
    }
    fn gesture_hold_end(&self, seat: &Seat<State>, data: &mut State, event: &GestureHoldEndEvent) {
        ptr_dispatch!(self, t => PointerTarget::gesture_hold_end(t, seat, data, event))
    }
}

impl TouchTarget<State> for PointerFocusTarget {
    fn down(&self, seat: &Seat<State>, data: &mut State, event: &DownEvent, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::down(t, seat, data, event, seq))
    }
    fn up(&self, seat: &Seat<State>, data: &mut State, event: &UpEvent, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::up(t, seat, data, event, seq))
    }
    fn motion(&self, seat: &Seat<State>, data: &mut State, event: &smithay::input::touch::MotionEvent, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::motion(t, seat, data, event, seq))
    }
    fn frame(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::frame(t, seat, data, seq))
    }
    fn cancel(&self, seat: &Seat<State>, data: &mut State, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::cancel(t, seat, data, seq))
    }
    fn shape(&self, seat: &Seat<State>, data: &mut State, event: &ShapeEvent, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::shape(t, seat, data, event, seq))
    }
    fn orientation(&self, seat: &Seat<State>, data: &mut State, event: &OrientationEvent, seq: Serial) {
        ptr_dispatch!(self, t => TouchTarget::orientation(t, seat, data, event, seq))
    }
}
