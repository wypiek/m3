//! Interactive move and resize.

use std::cell::RefCell;

use m3_protocol::Direction;
use smithay::{
    desktop::{Window, WindowSurface},
    input::{
        Seat,
        pointer::{
            AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
            GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
            GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
        },
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Serial, Size},
    wayland::{compositor::with_states, seat::WaylandFocus},
    xwayland::xwm::ResizeEdge as X11ResizeEdge,
};

use crate::{
    focus::PointerFocusTarget,
    shell::{WindowExt, layout, window::clamp_size},
    state::State,
};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ResizeEdge: u32 {
        const TOP = 0b0001;
        const BOTTOM = 0b0010;
        const LEFT = 0b0100;
        const RIGHT = 0b1000;
        const TOP_LEFT = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT = Self::BOTTOM.bits() | Self::LEFT.bits();
        const TOP_RIGHT = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(e: xdg_toplevel::ResizeEdge) -> Self {
        Self::from_bits(e as u32).unwrap_or(ResizeEdge::BOTTOM_RIGHT)
    }
}

impl From<X11ResizeEdge> for ResizeEdge {
    fn from(e: X11ResizeEdge) -> Self {
        match e {
            X11ResizeEdge::Top => Self::TOP,
            X11ResizeEdge::Bottom => Self::BOTTOM,
            X11ResizeEdge::Left => Self::LEFT,
            X11ResizeEdge::Right => Self::RIGHT,
            X11ResizeEdge::TopLeft => Self::TOP_LEFT,
            X11ResizeEdge::TopRight => Self::TOP_RIGHT,
            X11ResizeEdge::BottomLeft => Self::BOTTOM_LEFT,
            X11ResizeEdge::BottomRight => Self::BOTTOM_RIGHT,
        }
    }
}

/// Validate a client-initiated grab (the client must hold an implicit grab
/// from a button press on one of its surfaces).
fn client_grab_start(seat: &Seat<State>, window: &Window, serial: Serial) -> Option<GrabStartData<State>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start = pointer.grab_start_data()?;
    let (focus, _) = start.focus.as_ref()?;
    let surface = window.surface()?;
    if !focus.same_client_as(&smithay::reexports::wayland_server::Resource::id(&surface)) {
        return None;
    }
    Some(start)
}

fn compositor_grab_start(seat: &Seat<State>, button: u32) -> Option<GrabStartData<State>> {
    let pointer = seat.get_pointer()?;
    Some(GrabStartData { focus: None, button, location: pointer.current_location() })
}

pub fn start_move(state: &mut State, seat: &Seat<State>, window: &Window, serial: Serial, client: bool) {
    let start = if client { client_grab_start(seat, window, serial) } else { compositor_grab_start(seat, BTN_LEFT) };
    let Some(start) = start else { return };
    let Some(ws) = state.m3.shell.workspace_of(window) else { return };
    if window.st().fullscreen {
        return;
    }
    // Dragging a tiled window turns it into a floating one.
    let tiled = state.m3.shell.workspaces[ws].layout != m3_core::config::LayoutKind::Floating && !window.st().floating;
    if tiled {
        window.st_mut().floating = true;
        state.arrange(ws);
    }
    let unsnapped = state.unsnap_for_move(window, start.location);
    let Some(initial) = unsnapped.or_else(|| state.m3.shell.workspaces[ws].space.element_location(window)) else {
        return;
    };
    state.focus_window(window);
    let grab = MoveGrab { start_data: start, window: window.clone(), initial, snap_preview: None };
    if let Some(pointer) = seat.get_pointer() {
        pointer.set_grab(state, grab, serial, Focus::Clear);
    }
}

pub fn start_resize(
    state: &mut State,
    seat: &Seat<State>,
    window: &Window,
    serial: Serial,
    edges: ResizeEdge,
    client: bool,
) {
    let start = if client { client_grab_start(seat, window, serial) } else { compositor_grab_start(seat, BTN_RIGHT) };
    let Some(start) = start else { return };
    let Some(ws) = state.m3.shell.workspace_of(window) else { return };
    {
        let st = window.st();
        if st.fullscreen || st.maximized {
            return;
        }
    }
    let Some(loc) = state.m3.shell.workspaces[ws].space.element_location(window) else { return };
    let initial = Rectangle::new(loc, window.geometry().size);
    window.st_mut().snapped = false;
    ResizeState::set(window, ResizeState::Resizing { edges, initial });
    window.set_xdg_state(xdg_toplevel::State::Resizing, true);
    window.send_configure();
    let grab = ResizeGrab { start_data: start, window: window.clone(), edges, initial, last_size: initial.size };
    if let Some(pointer) = seat.get_pointer() {
        pointer.set_grab(state, grab, serial, Focus::Clear);
    }
}

pub struct MoveGrab {
    start_data: GrabStartData<State>,
    window: Window,
    initial: Point<i32, Logical>,
    snap_preview: Option<Direction>,
}

impl MoveGrab {
    /// Edge of the output the pointer is pressed against, if any.
    fn edge_target(state: &State, p: Point<f64, Logical>) -> Option<Direction> {
        let o = state.m3.shell.output_at(p)?;
        let g = state.m3.shell.output_geometry(o)?;
        let edge = 4.0;
        if p.x <= g.loc.x as f64 + edge {
            Some(Direction::Left)
        } else if p.x >= (g.loc.x + g.size.w) as f64 - edge - 1.0 {
            Some(Direction::Right)
        } else if p.y <= g.loc.y as f64 + edge {
            Some(Direction::Up)
        } else {
            None
        }
    }
}

macro_rules! forward_gestures {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, focus, event);
        }
        fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) {
            handle.axis(data, details)
        }
        fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
            handle.frame(data);
        }
        fn gesture_swipe_begin(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureSwipeBeginEvent,
        ) {
            handle.gesture_swipe_begin(data, event)
        }
        fn gesture_swipe_update(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureSwipeUpdateEvent,
        ) {
            handle.gesture_swipe_update(data, event)
        }
        fn gesture_swipe_end(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureSwipeEndEvent,
        ) {
            handle.gesture_swipe_end(data, event)
        }
        fn gesture_pinch_begin(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GesturePinchBeginEvent,
        ) {
            handle.gesture_pinch_begin(data, event)
        }
        fn gesture_pinch_update(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GesturePinchUpdateEvent,
        ) {
            handle.gesture_pinch_update(data, event)
        }
        fn gesture_pinch_end(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GesturePinchEndEvent,
        ) {
            handle.gesture_pinch_end(data, event)
        }
        fn gesture_hold_begin(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureHoldBeginEvent,
        ) {
            handle.gesture_hold_begin(data, event)
        }
        fn gesture_hold_end(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            event: &GestureHoldEndEvent,
        ) {
            handle.gesture_hold_end(data, event)
        }
        fn start_data(&self) -> &GrabStartData<State> {
            &self.start_data
        }
    };
}

impl PointerGrab<State> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let Some(ws) = data.m3.shell.workspace_of(&self.window) else { return };
        let delta = event.location - self.start_data.location;
        let mut loc = (self.initial.to_f64() + delta).to_i32_round();
        let size = self.window.geometry().size;
        if let Some(o) = data.m3.shell.output_at(event.location).cloned() {
            let area = data.m3.shell.usable_area(&o);
            loc = layout::edge_snap(loc, size, area, data.m3.config.compositor.windows.snap_threshold);
            self.window.st_mut().output = Some(o.name());
        }
        self.snap_preview = MoveGrab::edge_target(data, event.location);
        data.m3.shell.workspaces[ws].space.map_element(self.window.clone(), loc, true);
        self.window.set_x11_geometry(Rectangle::new(loc, size));
        data.m3.queue_redraw_all();
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            if let Some(dir) = self.snap_preview.take() {
                let w = self.window.clone();
                if dir == Direction::Up { data.set_maximized(&w, true) } else { data.snap_window(&w, dir) }
            }
            data.m3.shell.dirty = true;
        }
    }

    fn unset(&mut self, _data: &mut State) {}

    forward_gestures!();
}

/// Resize bookkeeping stored on the surface so commits can keep the
/// opposite edge fixed when resizing from the top or left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ResizeState {
    #[default]
    Idle,
    Resizing {
        edges: ResizeEdge,
        initial: Rectangle<i32, Logical>,
    },
    WaitingForLastCommit {
        edges: ResizeEdge,
        initial: Rectangle<i32, Logical>,
    },
}

impl ResizeState {
    fn with<T>(window: &Window, f: impl FnOnce(&mut ResizeState) -> T) -> Option<T> {
        let surface = window.wl_surface()?;
        Some(with_states(&surface, |states| {
            states.data_map.insert_if_missing(RefCell::<ResizeState>::default);
            let cell = states.data_map.get::<RefCell<ResizeState>>().expect("just inserted");
            f(&mut cell.borrow_mut())
        }))
    }

    fn set(window: &Window, s: ResizeState) {
        ResizeState::with(window, |r| *r = s);
    }
}

pub struct ResizeGrab {
    start_data: GrabStartData<State>,
    window: Window,
    edges: ResizeEdge,
    initial: Rectangle<i32, Logical>,
    last_size: Size<i32, Logical>,
}

impl PointerGrab<State> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(PointerFocusTarget, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        let mut delta = event.location - self.start_data.location;
        let mut w = self.initial.size.w;
        let mut h = self.initial.size.h;
        if self.edges.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                delta.x = -delta.x;
            }
            w = (self.initial.size.w as f64 + delta.x) as i32;
        }
        if self.edges.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) {
            if self.edges.intersects(ResizeEdge::TOP) {
                delta.y = -delta.y;
            }
            h = (self.initial.size.h as f64 + delta.y) as i32;
        }
        let (min, max) = self.window.min_max_size();
        self.last_size = clamp_size(Size::from((w, h)), min, max);

        match self.window.underlying_surface() {
            WindowSurface::Wayland(t) => {
                t.with_pending_state(|s| {
                    s.states.set(xdg_toplevel::State::Resizing);
                    s.size = Some(self.last_size);
                });
                t.send_pending_configure();
            }
            WindowSurface::X11(x) => {
                // X11 windows resize synchronously: move the origin ourselves.
                let mut loc = self.initial.loc;
                if self.edges.intersects(ResizeEdge::LEFT) {
                    loc.x += self.initial.size.w - self.last_size.w;
                }
                if self.edges.intersects(ResizeEdge::TOP) {
                    loc.y += self.initial.size.h - self.last_size.h;
                }
                let _ = x.configure(Rectangle::new(loc, self.last_size));
                if let Some(ws) = data.m3.shell.workspace_of(&self.window) {
                    data.m3.shell.workspaces[ws].space.map_element(self.window.clone(), loc, false);
                }
            }
        }
        data.m3.queue_redraw_all();
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            if let WindowSurface::Wayland(t) = self.window.underlying_surface() {
                t.with_pending_state(|s| {
                    s.states.unset(xdg_toplevel::State::Resizing);
                    s.size = Some(self.last_size);
                });
                t.send_pending_configure();
                ResizeState::set(
                    &self.window,
                    ResizeState::WaitingForLastCommit { edges: self.edges, initial: self.initial },
                );
            } else {
                ResizeState::set(&self.window, ResizeState::Idle);
            }
            data.m3.shell.dirty = true;
        }
    }

    fn unset(&mut self, _data: &mut State) {}

    forward_gestures!();
}

/// Keep the opposite edge anchored while resizing from the top/left.
pub fn handle_resize_commit(state: &mut State, window: &Window) {
    if window.is_x11() {
        return;
    }
    let Some(ws) = state.m3.shell.workspace_of(window) else { return };
    let Some(mut loc) = state.m3.shell.workspaces[ws].space.element_location(window) else { return };
    let geo = window.geometry();
    let adjust = ResizeState::with(window, |s| match *s {
        ResizeState::Resizing { edges, initial } => Some((edges, initial)),
        ResizeState::WaitingForLastCommit { edges, initial } => {
            *s = ResizeState::Idle;
            Some((edges, initial))
        }
        ResizeState::Idle => None,
    })
    .flatten();
    let Some((edges, initial)) = adjust else { return };
    let mut moved = false;
    if edges.intersects(ResizeEdge::LEFT) {
        loc.x = initial.loc.x + (initial.size.w - geo.size.w);
        moved = true;
    }
    if edges.intersects(ResizeEdge::TOP) {
        loc.y = initial.loc.y + (initial.size.h - geo.size.h);
        moved = true;
    }
    if moved {
        state.m3.shell.workspaces[ws].space.map_element(window.clone(), loc, false);
    }
}
