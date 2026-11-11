//! Wayland protocol handlers.

pub mod compositor;
pub mod layer_shell;
pub mod session_lock;
pub mod xdg_shell;
pub mod xwayland;

use std::{os::unix::io::OwnedFd, sync::Arc};

use smithay::{
    delegate_cursor_shape, delegate_data_control, delegate_data_device, delegate_foreign_toplevel_list,
    delegate_fractional_scale, delegate_idle_inhibit, delegate_idle_notify, delegate_input_method_manager,
    delegate_keyboard_shortcuts_inhibit, delegate_output, delegate_pointer_constraints, delegate_pointer_gestures,
    delegate_presentation, delegate_primary_selection, delegate_relative_pointer, delegate_seat,
    delegate_security_context, delegate_single_pixel_buffer, delegate_text_input_manager, delegate_viewporter,
    delegate_virtual_keyboard_manager, delegate_xdg_activation, delegate_xdg_foreign,
    desktop::{PopupKind, PopupManager},
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::LedState,
        pointer::{CursorImageStatus, PointerHandle},
    },
    reexports::wayland_server::{
        Resource,
        protocol::{wl_data_source::WlDataSource, wl_surface::WlSurface},
    },
    utils::{Logical, Rectangle},
    wayland::{
        compositor::{get_parent, with_states},
        foreign_toplevel_list::{ForeignToplevelListHandler, ForeignToplevelListState},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        input_method::{InputMethodHandler, PopupSurface as ImPopupSurface},
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
        },
        output::OutputHandler,
        pointer_constraints::{PointerConstraintsHandler, with_pointer_constraint},
        seat::WaylandFocus,
        security_context::{SecurityContext, SecurityContextHandler, SecurityContextListenerSource},
        selection::{
            SelectionHandler, SelectionSource, SelectionTarget,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler, set_data_device_focus,
            },
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState, set_primary_focus},
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        tablet_manager::TabletSeatHandler,
        xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData},
        xdg_foreign::{XdgForeignHandler, XdgForeignState},
    },
};
use tracing::warn;

use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    state::{ClientState, State},
};

// ---- seat ------------------------------------------------------------------

impl SeatHandler for State {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = PointerFocusTarget;
    type TouchFocus = PointerFocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.m3.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<State>, focused: Option<&KeyboardFocusTarget>) {
        let dh = &self.m3.display_handle;
        let client = focused.and_then(|f| f.wl_surface()).and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);

        let window = match focused {
            Some(KeyboardFocusTarget::Window(w)) => Some(w.clone()),
            Some(KeyboardFocusTarget::Popup(p)) => smithay::desktop::find_popup_root_surface(p)
                .ok()
                .and_then(|root| self.m3.shell.window_for_surface(&root)),
            _ => None,
        };
        // Keep the activated state while a layer surface (e.g. the launcher)
        // or popup temporarily has focus.
        if window.is_some() || focused.is_none() {
            self.update_activation(window.as_ref());
        }
        if let (Some(xwm), Some(w)) = (self.m3.xwm.as_mut(), window.as_ref()) {
            if let Some(x) = w.x11_surface() {
                if let Err(e) = xwm.raise_window(x) {
                    tracing::debug!("x11 raise failed: {e}");
                }
            }
        }
    }

    fn cursor_image(&mut self, _seat: &Seat<State>, image: CursorImageStatus) {
        self.m3.cursor_status = image;
        self.m3.queue_redraw_all();
    }

    fn led_state_changed(&mut self, _seat: &Seat<State>, led_state: LedState) {
        self.backend.update_leds(led_state);
    }
}
delegate_seat!(State);

impl TabletSeatHandler for State {}

// ---- selections ---------------------------------------------------------

impl SelectionHandler for State {
    type SelectionUserData = ();

    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        if let Some(xwm) = self.m3.xwm.as_mut() {
            if let Err(e) = xwm.new_selection(ty, source.map(|s| s.mime_types())) {
                warn!(?ty, "failed to forward selection to XWayland: {e}");
            }
        }
    }

    fn send_selection(&mut self, ty: SelectionTarget, mime_type: String, fd: OwnedFd, _seat: Seat<Self>, _: &()) {
        if let Some(xwm) = self.m3.xwm.as_mut() {
            if let Err(e) = xwm.send_selection(ty, mime_type, fd, self.m3.loop_handle.clone()) {
                warn!("failed to send selection to XWayland: {e}");
            }
        }
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.m3.data_device_state
    }
}

impl ClientDndGrabHandler for State {
    fn started(&mut self, _source: Option<WlDataSource>, icon: Option<WlSurface>, _seat: Seat<Self>) {
        self.m3.dnd_icon = icon;
    }
    fn dropped(&mut self, _target: Option<WlSurface>, _validated: bool, _seat: Seat<Self>) {
        self.m3.dnd_icon = None;
        self.m3.queue_redraw_all();
    }
}

impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {
        // m3 never starts server-side drags.
    }
}
delegate_data_device!(State);

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.m3.primary_selection_state
    }
}
delegate_primary_selection!(State);

impl DataControlHandler for State {
    fn data_control_state(&self) -> &DataControlState {
        &self.m3.data_control_state
    }
}
delegate_data_control!(State);

// ---- outputs / scaling ----------------------------------------------------

impl OutputHandler for State {}
delegate_output!(State);

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        let output = self
            .m3
            .shell
            .window_for_surface(&root)
            .and_then(|w| self.m3.shell.output_of(&w))
            .or_else(|| self.m3.shell.outputs.first().cloned());
        if let Some(output) = output {
            with_states(&surface, |states| {
                with_fractional_scale(states, |fs| {
                    fs.set_preferred_scale(output.current_scale().fractional_scale());
                });
            });
        }
    }
}
delegate_fractional_scale!(State);
delegate_viewporter!(State);
delegate_presentation!(State);
delegate_single_pixel_buffer!(State);
delegate_cursor_shape!(State);

// ---- input extras -----------------------------------------------------------

impl KeyboardShortcutsInhibitHandler for State {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.m3.keyboard_shortcuts_inhibit_state
    }
    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        // Remote desktop / VM viewers ask for this; grant it for normal
        // clients. The lock screen and VT switching ignore inhibitors.
        inhibitor.activate();
    }
}
delegate_keyboard_shortcuts_inhibit!(State);

delegate_relative_pointer!(State);
delegate_pointer_gestures!(State);
delegate_text_input_manager!(State);
delegate_virtual_keyboard_manager!(State);

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if pointer.current_focus().and_then(|f| f.wl_surface().map(|s| s.into_owned())).as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |c| {
                if let Some(c) = c {
                    c.activate();
                }
            });
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: smithay::utils::Point<f64, Logical>,
    ) {
        let active = with_pointer_constraint(surface, pointer, |c| c.is_some_and(|c| c.is_active()));
        if !active {
            return;
        }
        if let Some((ws, w)) = self.m3.shell.mapped_window_for_surface(surface) {
            if let Some(loc) = self.m3.shell.workspaces[ws].space.element_location(&w) {
                let origin = loc - w.geometry().loc;
                pointer.set_location(origin.to_f64() + location);
            }
        }
    }
}
delegate_pointer_constraints!(State);

impl InputMethodHandler for State {
    fn new_popup(&mut self, surface: ImPopupSurface) {
        if let Err(e) = self.m3.popups.track_popup(PopupKind::from(surface)) {
            warn!("failed to track input-method popup: {e}");
        }
    }
    fn popup_repositioned(&mut self, _: ImPopupSurface) {}
    fn dismiss_popup(&mut self, surface: ImPopupSurface) {
        if let Some(parent) = surface.get_parent().map(|p| p.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }
    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        self.m3
            .shell
            .mapped_window_for_surface(parent)
            .and_then(|(ws, w)| self.m3.shell.workspaces[ws].space.element_geometry(&w))
            .unwrap_or_default()
    }
}
delegate_input_method_manager!(State);

// ---- activation / foreign ----------------------------------------------------

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.m3.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        // Only grant tokens tied to recent user input on our seat; tokens
        // without a serial (e.g. created by the shell/launcher) are accepted
        // from unsandboxed clients.
        match data.serial {
            Some((serial, seat)) => {
                let Some(keyboard) = self.m3.seat.get_keyboard() else { return false };
                Seat::from_resource(&seat).as_ref() == Some(&self.m3.seat)
                    && keyboard.last_enter().is_some_and(|last| serial.is_no_older_than(&last))
            }
            None => data
                .client_id
                .and_then(|id| self.m3.display_handle.backend_handle().get_client_data(id).ok())
                .and_then(|d| d.downcast_arc::<ClientState>().ok())
                .is_some_and(|c| c.security_context.is_none()),
        }
    }

    fn request_activation(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        if data.timestamp.elapsed().as_secs() >= 10 {
            return;
        }
        if let Some(w) = self.m3.shell.window_for_surface(&surface) {
            self.activate_window(&w);
        }
    }
}
delegate_xdg_activation!(State);

impl XdgForeignHandler for State {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.m3.xdg_foreign_state
    }
}
delegate_xdg_foreign!(State);

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.m3.foreign_toplevel_state
    }
}
delegate_foreign_toplevel_list!(State);

// ---- idle --------------------------------------------------------------------

impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.m3.idle_notifier_state
    }
}
delegate_idle_notify!(State);

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.m3.idle_inhibitors.insert(surface);
    }
    fn uninhibit(&mut self, surface: WlSurface) {
        self.m3.idle_inhibitors.remove(&surface);
    }
}
delegate_idle_inhibit!(State);

// ---- security context (Flatpak) ------------------------------------------------

impl SecurityContextHandler for State {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        let r = self.m3.loop_handle.insert_source(source, move |stream, _, state| {
            let data = ClientState { security_context: Some(context.clone()), ..Default::default() };
            if let Err(e) = state.m3.display_handle.insert_client(stream, Arc::new(data)) {
                warn!("error adding sandboxed client: {e}");
            }
        });
        if let Err(e) = r {
            warn!("failed to listen on security-context socket: {e}");
        }
    }
}
delegate_security_context!(State);

/// Find the window or layer surface that owns `surface` (walking up subsurfaces).
pub fn root_surface(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}
