//! XWayland integration: legacy X11 applications.

use std::os::unix::io::OwnedFd;
use std::process::Stdio;

use smithay::{
    delegate_xwayland_keyboard_grab, delegate_xwayland_shell,
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::CompositorHandler,
        selection::{
            SelectionTarget,
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection,
                set_primary_selection,
            },
        },
        xwayland_keyboard_grab::XWaylandKeyboardGrabHandler,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler,
        xwm::{Reorder, ResizeEdge as X11ResizeEdge, XwmId},
    },
};
use tracing::{error, info, warn};

use crate::{
    focus::KeyboardFocusTarget,
    grabs::{self, ResizeEdge},
    shell::{WindowExt, init_window},
    state::{M3, State},
};

impl M3 {
    pub fn start_xwayland(&mut self) {
        let spawned = XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        );
        let (xwayland, client) = match spawned {
            Ok(x) => x,
            Err(e) => {
                warn!("XWayland unavailable, X11 applications will not run: {e}");
                return;
            }
        };
        let r = self.loop_handle.insert_source(xwayland, move |event, _, state| match event {
            XWaylandEvent::Ready { x11_socket, display_number } => {
                state.client_compositor_state(&client).set_client_scale(1.0);
                match X11Wm::start_wm(state.m3.loop_handle.clone(), x11_socket, client.clone()) {
                    Ok(mut wm) => {
                        let image = state.m3.cursor.default_image();
                        if let Err(e) = wm.set_cursor(
                            &image.pixels_rgba,
                            (image.width as u16, image.height as u16).into(),
                            (image.xhot as u16, image.yhot as u16).into(),
                        ) {
                            warn!("failed to set XWayland cursor: {e}");
                        }
                        state.m3.xwm = Some(wm);
                        state.m3.xdisplay = Some(display_number);
                        // SAFETY: the compositor is single-threaded.
                        unsafe { std::env::set_var("DISPLAY", format!(":{display_number}")) };
                        info!("XWayland ready on :{display_number}");
                    }
                    Err(e) => error!("failed to start the X11 window manager: {e}"),
                }
            }
            XWaylandEvent::Error => warn!("XWayland failed to start"),
        });
        if let Err(e) = r {
            error!("failed to insert XWayland source: {e}");
        }
    }
}

impl State {
    fn x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.m3
            .shell
            .all_windows()
            .into_iter()
            .chain(self.m3.shell.pending.iter().cloned())
            .find(|w| w.x11_surface() == Some(surface))
    }
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.m3.xwayland_shell_state
    }
}
delegate_xwayland_shell!(State);

impl XWaylandKeyboardGrabHandler for State {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<KeyboardFocusTarget> {
        self.m3.shell.window_for_surface(surface).map(KeyboardFocusTarget::Window)
    }
}
delegate_xwayland_keyboard_grab!(State);

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        // XwmHandler callbacks only run while the WM exists.
        self.m3.xwm.as_mut().expect("X11 callback without a window manager")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Err(e) = surface.set_mapped(true) {
            warn!("failed to map X11 window: {e}");
            return;
        }
        let window = match self.x11_window(&surface) {
            Some(w) => w,
            None => {
                let w = Window::new_x11_window(surface);
                let id = self.m3.shell.next_window_id();
                init_window(&w, id, 1.0);
                w
            }
        };
        self.map_window(window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let w = Window::new_x11_window(surface);
        let id = self.m3.shell.next_window_id();
        init_window(&w, id, 1.0);
        self.m3.shell.override_redirect.push(w);
        self.m3.queue_redraw_all();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.m3.shell.override_redirect.retain(|w| w.x11_surface() != Some(&surface));
        if let Some(w) = self.x11_window(&surface) {
            if self.m3.shell.workspace_of(&w).is_some() {
                self.unmap_window(&w);
            }
        }
        if !surface.is_override_redirect() {
            if let Err(e) = surface.set_mapped(false) {
                warn!("failed to unmap X11 window: {e}");
            }
        }
        self.m3.queue_redraw_all();
    }

    fn destroyed_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.m3.shell.override_redirect.retain(|w| w.x11_surface() != Some(&surface));
        if let Some(w) = self.x11_window(&surface) {
            self.destroy_window(&w);
        }
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Honour size requests for floating windows; tiled / maximized ones
        // keep the geometry the layout gave them.
        let managed = self.x11_window(&surface).filter(|win| self.m3.shell.workspace_of(win).is_some());
        let mut geo = surface.geometry();
        if let Some(win) = &managed {
            let st = win.st();
            if st.maximized
                || st.fullscreen
                || !st.floating
                    && self.m3.shell.workspaces[self.m3.shell.workspace_of(win).unwrap_or(0)].layout
                        != m3_core::config::LayoutKind::Floating
            {
                drop(st);
                let _ = surface.configure(geo);
                return;
            }
        }
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        if managed.is_none() {
            if let Some(x) = x {
                geo.loc.x = x;
            }
            if let Some(y) = y {
                geo.loc.y = y;
            }
        }
        if let Err(e) = surface.configure(geo) {
            warn!("X11 configure failed: {e}");
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        surface: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        // Override-redirect windows position themselves.
        if surface.is_override_redirect() {
            self.m3.queue_redraw_all();
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(w) = self.x11_window(&surface) {
            self.set_maximized(&w, true);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(w) = self.x11_window(&surface) {
            self.set_maximized(&w, false);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(w) = self.x11_window(&surface) {
            self.set_fullscreen(&w, true, None);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(w) = self.x11_window(&surface) {
            self.set_fullscreen(&w, false, None);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        if let Some(w) = self.x11_window(&surface) {
            self.minimize(&w);
        }
    }

    fn resize_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32, edges: X11ResizeEdge) {
        let seat = self.m3.seat.clone();
        if let Some(w) = self.x11_window(&surface) {
            grabs::start_resize(self, &seat, &w, SERIAL_COUNTER.next_serial(), ResizeEdge::from(edges), false);
        }
    }

    fn move_request(&mut self, _xwm: XwmId, surface: X11Surface, _button: u32) {
        let seat = self.m3.seat.clone();
        if let Some(w) = self.x11_window(&surface) {
            grabs::start_move(self, &seat, &w, SERIAL_COUNTER.next_serial(), false);
        }
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Only the focused X11 client may read the clipboard.
        matches!(
            self.focused_window().and_then(|w| w.x11_surface().and_then(|x| x.xwm_id())),
            Some(id) if id == xwm
        )
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        let r = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.m3.seat, mime_type, fd).map_err(|e| e.to_string())
            }
            SelectionTarget::Primary => {
                request_primary_client_selection(&self.m3.seat, mime_type, fd).map_err(|e| e.to_string())
            }
        };
        if let Err(e) = r {
            warn!("failed to forward Wayland selection to X11: {e}");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        let dh = self.m3.display_handle.clone();
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(&dh, &self.m3.seat, mime_types, ()),
            SelectionTarget::Primary => set_primary_selection(&dh, &self.m3.seat, mime_types, ()),
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        let dh = self.m3.display_handle.clone();
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.m3.seat).is_some() {
                    clear_data_device_selection(&dh, &self.m3.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.m3.seat).is_some() {
                    clear_primary_selection(&dh, &self.m3.seat)
                }
            }
        }
    }
}
