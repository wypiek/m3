//! Per-window metadata and helpers that hide the Wayland/X11 difference.

use std::cell::{Ref, RefCell, RefMut};

use smithay::{
    desktop::{Window, WindowSurface},
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

#[derive(Debug, Clone, Default)]
pub struct WindowState {
    /// Floating windows are not arranged by the tiling layouts.
    pub floating: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    /// Output the window belongs to (by name).
    pub output: Option<String>,
    /// Floating geometry to restore after maximize/fullscreen/tiling.
    pub restore: Option<Rectangle<i32, Logical>>,
    pub opacity: f32,
    /// Window rules have been applied.
    pub rules_applied: bool,
    /// The window has been mapped at least once (placement done).
    pub placed: bool,
    /// Where the window was when it got minimized.
    pub minimized_loc: Option<Point<i32, Logical>>,
    /// Snapped to half the screen; dragging it restores `restore`.
    pub snapped: bool,
    /// When the window last appeared (first map or un-minimize), for the
    /// open animation.
    pub opened_at: Option<std::time::Instant>,
}

#[derive(Debug)]
pub struct WindowData {
    pub id: u64,
    state: RefCell<WindowState>,
}

pub trait WindowExt {
    fn data(&self) -> &WindowData;
    fn id(&self) -> u64 {
        self.data().id
    }
    fn st(&self) -> Ref<'_, WindowState> {
        self.data().state.borrow()
    }
    fn st_mut(&self) -> RefMut<'_, WindowState> {
        self.data().state.borrow_mut()
    }
    fn app_id(&self) -> Option<String>;
    fn title(&self) -> Option<String>;
    fn surface(&self) -> Option<WlSurface>;
    /// Request a new size (`None` lets the client choose).
    fn request_size(&self, size: Option<Size<i32, Logical>>);
    /// Tell an X11 window where it is (Wayland windows don't know).
    fn set_x11_geometry(&self, geo: Rectangle<i32, Logical>);
    fn set_xdg_state(&self, state: xdg_toplevel::State, on: bool);
    fn send_configure(&self);
    fn close(&self);
    fn min_max_size(&self) -> (Size<i32, Logical>, Size<i32, Logical>);
    fn is_transient(&self) -> bool;
}

/// Attach m3 metadata to a freshly created window.
pub fn init_window(window: &Window, id: u64, opacity: f32) {
    window
        .user_data()
        .insert_if_missing(|| WindowData { id, state: RefCell::new(WindowState { opacity, ..Default::default() }) });
}

impl WindowExt for Window {
    fn data(&self) -> &WindowData {
        // Every window gets its data in `init_window` right after creation,
        // before it becomes reachable from anywhere else.
        self.user_data().get::<WindowData>().expect("window without m3 data")
    }

    fn app_id(&self) -> Option<String> {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => with_states(t.wl_surface(), |s| {
                s.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().ok().and_then(|d| d.app_id.clone()))
            }),
            WindowSurface::X11(x) => Some(x.class()).filter(|c| !c.is_empty()),
        }
    }

    fn title(&self) -> Option<String> {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => with_states(t.wl_surface(), |s| {
                s.data_map.get::<XdgToplevelSurfaceData>().and_then(|d| d.lock().ok().and_then(|d| d.title.clone()))
            }),
            WindowSurface::X11(x) => Some(x.title()).filter(|t| !t.is_empty()),
        }
    }

    fn surface(&self) -> Option<WlSurface> {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => Some(t.wl_surface().clone()),
            WindowSurface::X11(x) => x.wl_surface(),
        }
    }

    fn request_size(&self, size: Option<Size<i32, Logical>>) {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => t.with_pending_state(|s| s.size = size),
            WindowSurface::X11(x) => {
                if let Some(size) = size {
                    let mut geo = x.geometry();
                    geo.size = size;
                    if let Err(e) = x.configure(geo) {
                        tracing::debug!("x11 configure failed: {e}");
                    }
                }
            }
        }
    }

    fn set_x11_geometry(&self, geo: Rectangle<i32, Logical>) {
        if let WindowSurface::X11(x) = self.underlying_surface() {
            if let Err(e) = x.configure(geo) {
                tracing::debug!("x11 configure failed: {e}");
            }
        }
    }

    fn set_xdg_state(&self, state: xdg_toplevel::State, on: bool) {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => t.with_pending_state(|s| {
                if on {
                    s.states.set(state);
                } else {
                    s.states.unset(state);
                }
            }),
            WindowSurface::X11(x) => {
                let r = match state {
                    xdg_toplevel::State::Maximized => x.set_maximized(on),
                    xdg_toplevel::State::Fullscreen => x.set_fullscreen(on),
                    xdg_toplevel::State::Activated => x.set_activated(on),
                    _ => Ok(()),
                };
                if let Err(e) = r {
                    tracing::debug!("x11 state change failed: {e}");
                }
            }
        }
    }

    fn send_configure(&self) {
        if let WindowSurface::Wayland(t) = self.underlying_surface() {
            if t.is_initial_configure_sent() {
                t.send_pending_configure();
            }
        }
    }

    fn close(&self) {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => t.send_close(),
            WindowSurface::X11(x) => {
                if let Err(e) = x.close() {
                    tracing::debug!("x11 close failed: {e}");
                }
            }
        }
    }

    fn min_max_size(&self) -> (Size<i32, Logical>, Size<i32, Logical>) {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => with_states(t.wl_surface(), |s| {
                let mut c = s.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                let c = c.current();
                (c.min_size, c.max_size)
            }),
            WindowSurface::X11(x) => (x.min_size().unwrap_or_default(), x.max_size().unwrap_or_default()),
        }
    }

    /// Dialogs and other windows with a parent float by default.
    fn is_transient(&self) -> bool {
        match self.underlying_surface() {
            WindowSurface::Wayland(t) => t.parent().is_some(),
            WindowSurface::X11(x) => x.is_transient_for().is_some() || x.is_popup(),
        }
    }
}

/// Clamp a size into a client's min/max constraints (0 means unconstrained).
pub fn clamp_size(size: Size<i32, Logical>, min: Size<i32, Logical>, max: Size<i32, Logical>) -> Size<i32, Logical> {
    let maxw = if max.w > 0 { max.w } else { i32::MAX };
    let maxh = if max.h > 0 { max.h } else { i32::MAX };
    Size::from((size.w.clamp(min.w.max(1), maxw.max(1)), size.h.clamp(min.h.max(1), maxh.max(1))))
}

pub fn rect_center(r: Rectangle<i32, Logical>) -> Point<i32, Logical> {
    Point::from((r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2))
}
