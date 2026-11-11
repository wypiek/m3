use smithay::{
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType,
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
    },
    input::{Seat, pointer::Focus},
    output::Output,
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode, shell::server::xdg_toplevel,
        },
        wayland_server::protocol::{wl_output, wl_seat},
    },
    utils::{Rectangle, Serial},
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        decoration::XdgDecorationHandler,
    },
};
use tracing::debug;

use crate::{
    focus::KeyboardFocusTarget,
    grabs::{self, ResizeEdge},
    shell::{WindowExt, init_window},
    state::State,
};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.m3.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        let id = self.m3.shell.next_window_id();
        init_window(&window, id, 1.0);
        // Not mapped until it commits a buffer.
        self.m3.shell.pending.push(window);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.m3.shell.window_for_surface(surface.wl_surface()) {
            self.destroy_window(&w);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(e) = self.m3.popups.track_popup(PopupKind::Xdg(surface)) {
            debug!("failed to track popup: {e}");
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else { return };
        if let Some(w) = self.m3.shell.window_for_surface(surface.wl_surface()) {
            grabs::start_move(self, &seat, &w, serial, true);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else { return };
        if let Some(w) = self.m3.shell.window_for_surface(surface.wl_surface()) {
            grabs::start_resize(self, &seat, &w, serial, ResizeEdge::from(edges), true);
        }
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else { return };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else { return };
        let root_target = self.m3.shell.window_for_surface(&root).map(KeyboardFocusTarget::Window).or_else(|| {
            self.m3.shell.outputs.iter().find_map(|o| {
                layer_map_for_output(o)
                    .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                    .cloned()
                    .map(KeyboardFocusTarget::Layer)
            })
        });
        let Some(root_target) = root_target else { return };
        let Ok(mut grab) = self.m3.popups.grab_popup(root_target, kind, &seat, serial) else { return };

        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial) || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        match self.m3.shell.window_for_surface(surface.wl_surface()) {
            Some(w) if self.m3.shell.workspace_of(&w).is_some() => self.set_maximized(&w, true),
            Some(w) => {
                // Not mapped yet: remember, applied on map.
                w.st_mut().maximized = true;
                surface.with_pending_state(|s| s.states.set(xdg_toplevel::State::Maximized));
            }
            None => {}
        }
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.m3.shell.window_for_surface(surface.wl_surface()) {
            if self.m3.shell.workspace_of(&w).is_some() {
                self.set_maximized(&w, false);
            } else {
                w.st_mut().maximized = false;
                surface.with_pending_state(|s| s.states.unset(xdg_toplevel::State::Maximized));
            }
        }
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<wl_output::WlOutput>) {
        let output = output.as_ref().and_then(Output::from_resource);
        match self.m3.shell.window_for_surface(surface.wl_surface()) {
            Some(w) if self.m3.shell.workspace_of(&w).is_some() => self.set_fullscreen(&w, true, output),
            Some(w) => {
                w.st_mut().fullscreen = true;
                if let Some(o) = output {
                    w.st_mut().output = Some(o.name());
                }
                surface.with_pending_state(|s| s.states.set(xdg_toplevel::State::Fullscreen));
            }
            None => {}
        }
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.m3.shell.window_for_surface(surface.wl_surface()) {
            if self.m3.shell.workspace_of(&w).is_some() {
                self.set_fullscreen(&w, false, None);
            } else {
                w.st_mut().fullscreen = false;
                surface.with_pending_state(|s| s.states.unset(xdg_toplevel::State::Fullscreen));
            }
        }
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.m3.shell.window_for_surface(surface.wl_surface()) {
            self.minimize(&w);
        }
    }

    fn title_changed(&mut self, _surface: ToplevelSurface) {
        self.m3.shell.dirty = true;
    }

    fn app_id_changed(&mut self, _surface: ToplevelSurface) {
        self.m3.shell.dirty = true;
    }

    fn parent_changed(&mut self, _surface: ToplevelSurface) {}
}
delegate_xdg_shell!(State);

impl State {
    /// Keep popups inside the usable area of the output their parent is on.
    pub(crate) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else { return };

        // Popups of windows.
        if let Some((ws, w)) = self.m3.shell.mapped_window_for_surface(&root) {
            let space = &self.m3.shell.workspaces[ws].space;
            let Some(win_loc) = space.element_location(&w) else { return };
            let output = self.m3.shell.output_of(&w);
            let Some(area) = output.map(|o| self.m3.shell.usable_area(&o)) else { return };
            let mut target = area;
            target.loc -= get_popup_toplevel_coords(&kind);
            target.loc -= win_loc - w.geometry().loc;
            popup.with_pending_state(|s| s.geometry = s.positioner.get_unconstrained_geometry(target));
            return;
        }

        // Popups of layer surfaces (panel menus).
        for output in &self.m3.shell.outputs {
            let map = layer_map_for_output(output);
            let Some(layer) = map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL) else { continue };
            let Some(layer_geo) = map.layer_geometry(layer) else { continue };
            let out_geo = self.m3.shell.output_geometry(output).unwrap_or_default();
            let mut target = Rectangle::from_size(out_geo.size);
            target.loc -= layer_geo.loc;
            target.loc -= get_popup_toplevel_coords(&kind);
            popup.with_pending_state(|s| s.geometry = s.positioner.get_unconstrained_geometry(target));
            return;
        }
    }
}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Until the client asks for something specific, use the configured
        // preference.
        let mode = self.preferred_decoration_mode();
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        // m3 does not draw server-side title bars yet, so clients asking for
        // them are told to decorate themselves (xdg-decoration allows the
        // compositor to pick the mode).
        let _ = mode;
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecorationMode::ClientSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_decoration_mode();
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}
delegate_xdg_decoration!(State);

impl State {
    pub(crate) fn preferred_decoration_mode(&self) -> DecorationMode {
        // Server-side decorations are not rendered yet; always client-side.
        DecorationMode::ClientSide
    }
}
