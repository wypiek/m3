//! ext-session-lock-v1: the security-critical part of screen locking.
//!
//! Invariants:
//! * While locked, only lock surfaces (or an opaque fallback colour) are
//!   rendered and only lock surfaces receive input.
//! * If the lock client dies without unlocking, the session stays locked.
//!   A new lock client may then take over (so `m3-lock` can be restarted),
//!   but never while the original client is still alive.
//! * `locked` is only sent to the client after every output has rendered a
//!   locked frame, so no content can be visible after the client is told the
//!   session is locked.

use std::collections::HashSet;

use smithay::{
    delegate_session_lock,
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1,
        wayland_server::{Resource, protocol::wl_output::WlOutput},
    },
    wayland::session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
};
use tracing::{info, warn};

use crate::{focus::KeyboardFocusTarget, state::State};

#[derive(Default)]
pub enum LockStatus {
    #[default]
    Unlocked,
    /// Lock requested; waiting until every output showed a locked frame.
    Locking {
        locker: SessionLocker,
        blanked: HashSet<String>,
    },
    Locked,
}

#[derive(Default)]
pub struct LockState {
    pub status: LockStatus,
    lock_object: Option<ExtSessionLockV1>,
    pub surfaces: Vec<(Output, LockSurface)>,
    /// When we last restarted a crashed lock client.
    last_relaunch: Option<std::time::Instant>,
}

impl LockState {
    pub fn is_locked(&self) -> bool {
        !matches!(self.status, LockStatus::Unlocked)
    }

    /// Whether the client holding the lock is still around.
    pub fn client_alive(&self) -> bool {
        self.lock_object.as_ref().is_some_and(|l| l.is_alive())
    }

    pub fn surface_for(&self, output: &Output) -> Option<&LockSurface> {
        self.surfaces.iter().find(|(o, s)| o == output && s.alive()).map(|(_, s)| s)
    }

    pub fn focus_target(&self, preferred: Option<&Output>) -> Option<KeyboardFocusTarget> {
        preferred
            .and_then(|o| self.surface_for(o))
            .or_else(|| self.surfaces.iter().find(|(_, s)| s.alive()).map(|(_, s)| s))
            .cloned()
            .map(KeyboardFocusTarget::Lock)
    }

    pub fn cleanup(&mut self) {
        self.surfaces.retain(|(_, s)| s.alive());
    }

    /// The session is locked but the lock client is gone: the screen stays
    /// locked (opaque), and a fresh m3-lock should be started so the user
    /// can authenticate. Rate-limited to avoid crash loops.
    pub fn needs_relaunch(&mut self) -> bool {
        if !self.is_locked() || self.client_alive() {
            return false;
        }
        let now = std::time::Instant::now();
        if self.last_relaunch.is_some_and(|t| now.duration_since(t) < std::time::Duration::from_secs(5)) {
            return false;
        }
        self.last_relaunch = Some(now);
        true
    }

    /// Called by backends after a frame was rendered on `output`.
    pub fn output_rendered(&mut self, output: &Output, all_outputs: &[Output]) {
        let done = match &mut self.status {
            LockStatus::Locking { blanked, .. } => {
                blanked.insert(output.name());
                all_outputs.iter().all(|o| blanked.contains(&o.name()))
            }
            _ => false,
        };
        if done {
            if let LockStatus::Locking { locker, .. } = std::mem::replace(&mut self.status, LockStatus::Locked) {
                locker.lock();
                info!("session locked");
            }
        }
    }
}

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.m3.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        let lock = &mut self.m3.lock;
        if lock.is_locked() {
            if lock.client_alive() {
                warn!("rejecting a second session lock while the current lock client is alive");
                // Dropping the locker sends `finished`.
                return;
            }
            // The previous lock client died; let the new one take over. The
            // session never became visible in between.
            info!("lock client replaced after the previous one exited");
            lock.lock_object = Some(confirmation.ext_session_lock().clone());
            lock.surfaces.retain(|(_, s)| s.alive());
            confirmation.lock();
            return;
        }
        info!("session lock requested");
        lock.lock_object = Some(confirmation.ext_session_lock().clone());
        lock.status = LockStatus::Locking { locker: confirmation, blanked: HashSet::new() };
        // Drop any grabs (menus, drags) so nothing keeps input.
        if let Some(p) = self.m3.seat.get_pointer() {
            p.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), 0);
        }
        if let Some(k) = self.m3.seat.get_keyboard() {
            k.unset_grab(self);
        }
        self.m3.cycle = None;
        self.m3.queue_redraw_all();
        self.ensure_focus();
        self.update_pointer_focus();
        self.m3.ipc_event(m3_protocol::Event::LockChanged { locked: true });
    }

    fn unlock(&mut self) {
        info!("session unlocked");
        self.m3.lock = LockState::default();
        self.m3.queue_redraw_all();
        self.set_keyboard_focus(None);
        self.ensure_focus();
        self.update_pointer_focus();
        self.m3.ipc_event(m3_protocol::Event::LockChanged { locked: false });
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else { return };
        let Some(geo) = self.m3.shell.output_geometry(&output) else { return };
        surface.with_pending_state(|s| {
            s.size = Some((geo.size.w as u32, geo.size.h as u32).into());
        });
        surface.send_configure();
        self.m3.lock.surfaces.retain(|(o, _)| o != &output);
        self.m3.lock.surfaces.push((output, surface));
        self.m3.queue_redraw_all();
        self.ensure_focus();
    }
}
delegate_session_lock!(State);
