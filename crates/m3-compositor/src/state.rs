//! Global compositor state.
//!
//! [`State`] is the calloop data: it splits into the active [`Backend`] and
//! [`M3`], everything else. Keeping the backend separate lets rendering borrow
//! the renderer mutably while reading the rest of the state.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use smithay::{
    desktop::{PopupManager, layer_map_for_output},
    input::{Seat, SeatState, keyboard::XkbConfig, pointer::CursorImageStatus},
    output::Output,
    reexports::{
        calloop::{EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Clock, Monotonic},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        foreign_toplevel_list::ForeignToplevelListState,
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        input_method::InputMethodManagerState,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        security_context::{SecurityContext, SecurityContextState},
        selection::{
            data_device::DataDeviceState, primary_selection::PrimarySelectionState, wlr_data_control::DataControlState,
        },
        session_lock::SessionLockManagerState,
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{XdgShellState, decoration::XdgDecorationState},
        },
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
    },
    xwayland::X11Wm,
};
use tracing::{info, warn};

use crate::{
    Args, BackendKind,
    backend::Backend,
    config::Config,
    cursor::CursorManager,
    handlers::session_lock::LockState,
    input::Bindings,
    ipc::IpcServer,
    shell::{Shell, ops::WindowCycleState},
};

pub struct State {
    pub backend: Backend,
    pub m3: M3,
}

/// Per-output bookkeeping.
#[derive(Debug, Default)]
pub struct OutputState {
    pub redraw_queued: bool,
    /// Frames rendered, for diagnostics.
    pub frames: u64,
    pub last_frame: Option<Instant>,
}

pub struct M3 {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, State>,
    pub loop_signal: Option<LoopSignal>,
    pub clock: Clock<Monotonic>,
    pub start_time: Instant,
    pub socket_name: OsString,
    pub test_mode: bool,
    #[allow(dead_code)] // keep-alive
    pub backend_kind: BackendKind,

    pub config_dir: PathBuf,
    pub config: Config,
    pub theme: m3_theme::Theme,

    // Protocol state. Fields marked `keep-alive` are never read: they exist
    // so their globals' state lives as long as the compositor.
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    #[allow(dead_code)] // keep-alive
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    #[allow(dead_code)] // keep-alive
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    #[allow(dead_code)] // keep-alive
    pub viewporter_state: ViewporterState,
    pub xdg_activation_state: XdgActivationState,
    #[allow(dead_code)] // keep-alive
    pub fractional_scale_state: FractionalScaleManagerState,
    pub xdg_foreign_state: XdgForeignState,
    #[allow(dead_code)] // keep-alive
    pub presentation_state: PresentationState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub session_lock_state: SessionLockManagerState,
    pub idle_notifier_state: IdleNotifierState<State>,
    #[allow(dead_code)] // keep-alive
    pub idle_inhibit_state: IdleInhibitManagerState,
    pub foreign_toplevel_state: ForeignToplevelListState,
    #[allow(dead_code)] // keep-alive
    pub single_pixel_buffer_state: SinglePixelBufferState,
    #[allow(dead_code)] // keep-alive
    pub cursor_shape_state: CursorShapeManagerState,
    #[allow(dead_code)] // keep-alive
    pub security_context_state: SecurityContextState,
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,

    pub seat: Seat<State>,
    pub popups: PopupManager,
    pub shell: Shell,
    pub outputs: HashMap<Output, OutputState>,

    pub cursor: CursorManager,
    pub cursor_status: CursorImageStatus,
    pub dnd_icon: Option<WlSurface>,

    pub bindings: Bindings,
    /// Keycodes whose press was consumed by a binding; their release is
    /// swallowed too.
    pub suppressed_keys: HashSet<u32>,
    pub cycle: Option<WindowCycleState>,
    pub hot_corner_armed: bool,

    pub lock: LockState,
    pub idle_inhibitors: HashSet<WlSurface>,
    /// Last user input, for idle locking.
    pub last_activity: Instant,
    /// The idle period (identified by its `last_activity`) the lock / suspend
    /// timers already acted on, so each fires once per idle period.
    pub idle_lock_done: Option<Instant>,
    pub idle_suspend_done: Option<Instant>,
    /// Recent m3-shell (re)starts, for crash-loop protection.
    pub shell_restarts: Vec<Instant>,
    pub shell_pid: Option<u32>,
    /// Started with `--session`.
    pub session: bool,
    pub quitting: bool,

    pub ipc: Option<IpcServer>,

    pub xwm: Option<X11Wm>,
    pub xdisplay: Option<u32>,
    /// A wake-up for the next animation frame is scheduled.
    pub animation_tick: bool,
    /// `copy_with_damage` screencopy requests waiting for a new frame.
    pub pending_screencopies: Vec<crate::screencopy::PendingCopy>,
    pub _screencopy: crate::screencopy::ScreencopyState,
    /// Displays are powered off after inactivity.
    pub screens_off: bool,
    /// Colour temperature currently applied (kelvin) and transition target;
    /// `None` = not applied yet (forces a write).
    pub night_light_applied: Option<f64>,
    pub night_light_ticking: bool,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    pub security_context: Option<SecurityContext>,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl State {
    pub fn new(
        args: &Args,
        kind: BackendKind,
        display: Display<State>,
        event_loop: &mut EventLoop<'static, State>,
    ) -> anyhow::Result<Self> {
        let dh = display.handle();
        let loop_handle = event_loop.handle();

        // Wayland socket.
        let source = ListeningSocketSource::new_auto().context("creating the Wayland socket")?;
        let socket_name = source.socket_name().to_os_string();
        loop_handle
            .insert_source(source, |stream, _, state| {
                if let Err(e) = state.m3.display_handle.insert_client(stream, Arc::new(ClientState::default())) {
                    warn!("error adding Wayland client: {e}");
                }
            })
            .map_err(|e| anyhow::anyhow!("inserting the Wayland socket source: {e}"))?;
        loop_handle
            .insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, state| {
                // SAFETY: the display is never dropped while the source exists.
                if let Err(e) = unsafe { display.get_mut().dispatch_clients(state) } {
                    warn!("error dispatching Wayland clients: {e}");
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| anyhow::anyhow!("inserting the Wayland display source: {e}"))?;
        info!(socket = ?socket_name, "listening on Wayland socket");

        let config_dir = match &args.config_dir {
            Some(d) => d.clone(),
            None => m3_core::config::default_dir()?,
        };
        let config = Config::load(&config_dir);

        let clock = Clock::new();
        let compositor_state = CompositorState::new::<State>(&dh);
        let xdg_shell_state = XdgShellState::new::<State>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<State>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<State>(&dh);
        let shm_state = ShmState::new::<State>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<State>(&dh);
        // Clipboard managers are trusted; sandboxed clients don't get data control.
        let data_control_state = DataControlState::new::<State, _>(&dh, Some(&primary_selection_state), |client| {
            client.get_data::<ClientState>().is_none_or(|c| c.security_context.is_none())
        });
        let data_device_state = DataDeviceState::new::<State>(&dh);
        let viewporter_state = ViewporterState::new::<State>(&dh);
        let xdg_activation_state = XdgActivationState::new::<State>(&dh);
        let fractional_scale_state = FractionalScaleManagerState::new::<State>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<State>(&dh);
        let presentation_state = PresentationState::new::<State>(&dh, clock.id() as u32);
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<State>(&dh);
        let unsandboxed = |client: &smithay::reexports::wayland_server::Client| {
            client.get_data::<ClientState>().is_none_or(|c| c.security_context.is_none())
        };
        let session_lock_state = SessionLockManagerState::new::<State, _>(&dh, unsandboxed);
        let idle_notifier_state = IdleNotifierState::<State>::new(&dh, loop_handle.clone());
        let idle_inhibit_state = IdleInhibitManagerState::new::<State>(&dh);
        let foreign_toplevel_state = ForeignToplevelListState::new_with_filter::<State>(&dh, unsandboxed);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<State>(&dh);
        let cursor_shape_state = CursorShapeManagerState::new::<State>(&dh);
        let security_context_state = SecurityContextState::new::<State, _>(&dh, unsandboxed);
        let xwayland_shell_state = smithay::wayland::xwayland_shell::XWaylandShellState::new::<State>(&dh);
        TextInputManagerState::new::<State>(&dh);
        InputMethodManagerState::new::<State, _>(&dh, unsandboxed);
        VirtualKeyboardManagerState::new::<State, _>(&dh, unsandboxed);
        RelativePointerManagerState::new::<State>(&dh);
        PointerConstraintsState::new::<State>(&dh);
        PointerGesturesState::new::<State>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        let kb = &config.compositor.input.keyboard;
        let xkb = XkbConfig {
            rules: "",
            model: &kb.model,
            layout: &kb.layout,
            variant: &kb.variant,
            options: if kb.options.is_empty() { None } else { Some(kb.options.clone()) },
        };
        let keyboard = match seat.add_keyboard(xkb, kb.repeat_delay, kb.repeat_rate) {
            Ok(k) => k,
            Err(e) => {
                warn!("keyboard layout `{}` is invalid ({e}); falling back to us", kb.layout);
                seat.add_keyboard(XkbConfig::default(), kb.repeat_delay, kb.repeat_rate)
                    .context("initializing the keyboard")?
            }
        };
        if kb.numlock {
            let mut mods = keyboard.modifier_state();
            mods.num_lock = true;
            keyboard.set_modifier_state(mods);
        }
        seat.add_pointer();

        let bindings = Bindings::from_config(&config.keybindings);
        let shell = Shell::new(&config);
        let cursor = CursorManager::new(&config.compositor.general.cursor_theme, config.compositor.general.cursor_size);

        let m3 = M3 {
            animation_tick: false,
            screens_off: false,
            night_light_applied: None,
            night_light_ticking: false,
            pending_screencopies: Vec::new(),
            _screencopy: crate::screencopy::ScreencopyState::new(&dh),
            display_handle: dh.clone(),
            loop_handle: loop_handle.clone(),
            loop_signal: None,
            clock,
            start_time: Instant::now(),
            socket_name: socket_name.clone(),
            test_mode: args.test_mode,
            backend_kind: kind,
            config_dir,
            theme: crate::config::resolve_theme(&config),
            config,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            data_control_state,
            viewporter_state,
            xdg_activation_state,
            fractional_scale_state,
            xdg_foreign_state,
            presentation_state,
            keyboard_shortcuts_inhibit_state,
            session_lock_state,
            idle_notifier_state,
            idle_inhibit_state,
            foreign_toplevel_state,
            single_pixel_buffer_state,
            cursor_shape_state,
            security_context_state,
            xwayland_shell_state,
            seat,
            popups: PopupManager::default(),
            shell,
            outputs: HashMap::new(),
            cursor,
            cursor_status: CursorImageStatus::default_named(),
            dnd_icon: None,
            bindings,
            suppressed_keys: HashSet::new(),
            cycle: None,
            hot_corner_armed: true,
            lock: LockState::default(),
            idle_inhibitors: HashSet::new(),
            last_activity: Instant::now(),
            idle_lock_done: None,
            idle_suspend_done: None,
            shell_restarts: Vec::new(),
            shell_pid: None,
            session: false,
            quitting: false,
            ipc: None,
            xwm: None,
            xdisplay: None,
        };

        let backend = Backend::init(kind, args, event_loop, &dh)?;
        let mut state = State { backend, m3 };

        // Environment for everything we spawn (and for the backend's children).
        // SAFETY: single-threaded at this point; no other threads read the env.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", &socket_name);
            std::env::set_var("XDG_CURRENT_DESKTOP", "m3");
            std::env::set_var("XDG_SESSION_DESKTOP", "m3");
            std::env::set_var("XDG_SESSION_TYPE", "wayland");
            std::env::remove_var("DISPLAY");
        }

        state.backend.add_initial_outputs(&mut state.m3, args)?;
        let formats = state.backend.shm_formats();
        state.m3.shm_state.update_formats(formats);
        state.backend.init_dmabuf(&dh);

        match IpcServer::start(&mut state.m3) {
            Ok(server) => {
                // SAFETY: as above.
                unsafe { std::env::set_var(m3_protocol::SOCKET_ENV, server.path()) };
                state.m3.ipc = Some(server);
            }
            Err(e) => warn!("IPC disabled: {e:#}"),
        }

        state.start_idle_timer();
        state.update_night_light();
        let r = loop_handle.insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(Duration::from_secs(60)),
            |_, _, state| {
                state.update_night_light();
                smithay::reexports::calloop::timer::TimeoutAction::ToDuration(Duration::from_secs(60))
            },
        );
        if let Err(e) = r {
            warn!("night light timer unavailable: {e}");
        }

        // Automatic light/dark switching: re-check once a minute. Cheap, and
        // only redraws when the resolved theme actually changed.
        let r = loop_handle.insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(Duration::from_secs(60)),
            |_, _, state| {
                if state.m3.config.theme.mode == m3_core::config::ThemeMode::Auto {
                    let theme = crate::config::resolve_theme(&state.m3.config);
                    if theme != state.m3.theme {
                        state.m3.theme = theme;
                        state.m3.queue_redraw_all();
                    }
                }
                smithay::reexports::calloop::timer::TimeoutAction::ToDuration(Duration::from_secs(60))
            },
        );
        if let Err(e) = r {
            warn!("theme schedule timer unavailable: {e}");
        }

        if state.m3.config.compositor.general.xwayland && !args.no_xwayland {
            state.m3.start_xwayland();
        }

        Ok(state)
    }

    /// Runs once per event-loop iteration after all sources were dispatched.
    pub fn refresh_and_flush(&mut self) {
        self.m3.refresh();
        self.ensure_focus();
        self.m3.ipc_refresh();
        self.render_queued();
        self.flush_screencopies();
        if let Some(ipc) = &mut self.m3.ipc {
            ipc.flush_writes();
        }
        if let Err(e) = self.m3.display_handle.flush_clients() {
            warn!("error flushing clients: {e}");
        }
    }

    /// Input after the displays went off: bring them back.
    pub fn wake_displays(&mut self) {
        if self.m3.screens_off {
            info!("activity; turning displays on");
            self.m3.screens_off = false;
            self.backend.set_displays_powered(true);
            self.m3.night_light_applied = None;
            self.update_night_light();
            self.m3.queue_redraw_all();
        }
    }

    /// Move the night-light colour temperature towards what the schedule
    /// wants, smoothly (a short series of gamma updates).
    pub fn update_night_light(&mut self) {
        let nl = &self.m3.config.compositor.night_light;
        let target = if nl.active_at(m3_core::time::local_minutes()) { f64::from(nl.temperature) } else { 6500.0 };
        let current = self.m3.night_light_applied;
        if current == Some(target) || self.m3.night_light_ticking {
            return;
        }
        // First application (startup, VT switch back) is immediate unless
        // it's a real change from neutral.
        let start = current.unwrap_or(6500.0);
        if current.is_none() && target == 6500.0 {
            if self.backend.set_color_temperature(6500.0) {
                self.m3.night_light_applied = Some(6500.0);
            }
            return;
        }
        let instant = self.m3.config.theme.reduced_motion || !self.m3.config.compositor.animations.enabled;
        if instant {
            if self.backend.set_color_temperature(target) {
                self.m3.night_light_applied = Some(target);
            }
            return;
        }
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        self.m3.night_light_ticking = true;
        self.m3.night_light_applied = Some(start);
        let r = self.m3.loop_handle.insert_source(Timer::from_duration(Duration::from_millis(40)), |_, _, state| {
            let nl = &state.m3.config.compositor.night_light;
            let target = if nl.active_at(m3_core::time::local_minutes()) { f64::from(nl.temperature) } else { 6500.0 };
            let cur = state.m3.night_light_applied.unwrap_or(6500.0);
            let next = crate::nightlight::step_towards(cur, target);
            if !state.backend.set_color_temperature(next) {
                state.m3.night_light_ticking = false;
                return TimeoutAction::Drop;
            }
            state.m3.night_light_applied = Some(next);
            if next == target {
                state.m3.night_light_ticking = false;
                TimeoutAction::Drop
            } else {
                TimeoutAction::ToDuration(Duration::from_millis(40))
            }
        });
        if let Err(e) = r {
            warn!("night light transition unavailable: {e}");
            self.m3.night_light_ticking = false;
        }
    }

    /// Render every output that has a redraw queued and is ready for one.
    pub fn render_queued(&mut self) {
        if self.m3.screens_off {
            return;
        }
        let outputs: Vec<Output> =
            self.m3.outputs.iter().filter(|(_, s)| s.redraw_queued).map(|(o, _)| o.clone()).collect();
        for output in outputs {
            self.backend.render(&mut self.m3, &output);
        }
    }

    /// Lock the session after `idle.lock_after` seconds without input. The
    /// timer re-arms itself for exactly the remaining time (no polling) and
    /// respects idle inhibitors (video players, presentations).
    fn start_idle_timer(&mut self) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let first = Duration::from_secs(u64::from(self.m3.config.compositor.idle.lock_after.max(1)));
        let r = self.m3.loop_handle.insert_source(Timer::from_duration(first), |_, _, state| {
            let m3 = &mut state.m3;
            let lock_after = m3.config.compositor.idle.lock_after;
            if lock_after == 0 {
                // Disabled: check again later in case the setting changes.
                return TimeoutAction::ToDuration(Duration::from_secs(60));
            }
            let limit = Duration::from_secs(u64::from(lock_after));
            let idle = m3.last_activity.elapsed();
            if idle < limit {
                return TimeoutAction::ToDuration(limit - idle);
            }
            if m3.idle_lock_done != Some(m3.last_activity) && !m3.lock.is_locked() && m3.idle_inhibitors.is_empty() {
                info!("idle for {}s; locking", idle.as_secs());
                crate::spawn::spawn(m3, &["m3-lock".to_owned()]);
                m3.idle_lock_done = Some(m3.last_activity);
            }
            TimeoutAction::ToDuration(limit)
        });
        if let Err(e) = r {
            warn!("idle lock timer unavailable: {e}");
        }

        // Screen off: power displays down; any input wakes them.
        let first = match self.m3.config.compositor.idle.screen_off_after {
            0 => Duration::from_secs(60),
            n => Duration::from_secs(u64::from(n)),
        };
        let r = self.m3.loop_handle.insert_source(Timer::from_duration(first), |_, _, state| {
            let after = state.m3.config.compositor.idle.screen_off_after;
            if after == 0 {
                return TimeoutAction::ToDuration(Duration::from_secs(60));
            }
            let limit = Duration::from_secs(u64::from(after));
            let idle = state.m3.last_activity.elapsed();
            if idle < limit {
                return TimeoutAction::ToDuration(limit - idle);
            }
            if !state.m3.screens_off && state.m3.idle_inhibitors.is_empty() {
                info!("idle for {}s; turning displays off", idle.as_secs());
                state.m3.screens_off = true;
                state.backend.set_displays_powered(false);
            }
            TimeoutAction::ToDuration(limit)
        });
        if let Err(e) = r {
            warn!("screen-off timer unavailable: {e}");
        }

        // Automatic suspend, through logind (polkit decides if allowed).
        let first = match self.m3.config.compositor.idle.suspend_after {
            0 => Duration::from_secs(60),
            n => Duration::from_secs(u64::from(n)),
        };
        let r = self.m3.loop_handle.insert_source(Timer::from_duration(first), |_, _, state| {
            let m3 = &mut state.m3;
            let after = m3.config.compositor.idle.suspend_after;
            if after == 0 {
                return TimeoutAction::ToDuration(Duration::from_secs(60));
            }
            let limit = Duration::from_secs(u64::from(after));
            let idle = m3.last_activity.elapsed();
            if idle < limit {
                return TimeoutAction::ToDuration(limit - idle);
            }
            if m3.idle_suspend_done != Some(m3.last_activity) && m3.idle_inhibitors.is_empty() {
                info!("idle for {}s; suspending", idle.as_secs());
                crate::spawn::spawn(m3, &["systemctl".to_owned(), "suspend".to_owned()]);
                m3.idle_suspend_done = Some(m3.last_activity);
            }
            TimeoutAction::ToDuration(limit)
        });
        if let Err(e) = r {
            warn!("idle suspend timer unavailable: {e}");
        }
    }

    /// Outputs were added, removed or reconfigured.
    pub fn outputs_changed(&mut self) {
        for output in self.m3.shell.outputs.clone() {
            crate::backend::layer_arrange(&output);
        }
        // Rescue floating windows stranded on a removed/moved output.
        let bounds = self.m3.shell.global_bounds();
        for ws in &mut self.m3.shell.workspaces {
            for w in ws.space.elements().cloned().collect::<Vec<_>>() {
                if let Some(geo) = ws.space.element_geometry(&w) {
                    let loc = crate::shell::layout::keep_reachable(geo, bounds, 64);
                    if loc != geo.loc {
                        ws.space.map_element(w, loc, false);
                    }
                }
            }
        }
        for ws in 0..self.m3.shell.workspaces.len() {
            self.arrange(ws);
        }
        self.m3.ipc_outputs_changed();
        self.update_pointer_focus();
        self.m3.queue_redraw_all();
        // New CRTCs start with a neutral gamma ramp.
        self.m3.night_light_applied = None;
        self.update_night_light();
    }

    pub fn shutdown(&mut self) {
        self.m3.quitting = true;
        crate::spawn::end_session(&self.m3);
        if let Some(pid) = self.m3.shell_pid.take().and_then(|p| rustix::process::Pid::from_raw(p as i32)) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        if let Some(ipc) = self.m3.ipc.take() {
            ipc.remove_socket();
        }
    }
}

impl M3 {
    /// Keep frames coming while something animates: the event loop sleeps
    /// until an event arrives, so wake it for the next frame (backends that
    /// wait for vblank simply render on their next vblank).
    pub fn schedule_animation_frame(&mut self) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        if self.animation_tick {
            return;
        }
        self.animation_tick = true;
        let r = self.loop_handle.insert_source(Timer::from_duration(Duration::from_millis(8)), |_, _, state| {
            state.m3.animation_tick = false;
            state.m3.queue_redraw_all();
            TimeoutAction::Drop
        });
        if let Err(e) = r {
            warn!("cannot schedule an animation frame: {e}");
            self.animation_tick = false;
        }
    }

    pub fn queue_redraw_all(&mut self) {
        for s in self.outputs.values_mut() {
            s.redraw_queued = true;
        }
    }

    /// Periodic housekeeping between event dispatches.
    pub fn refresh(&mut self) {
        self.shell.refresh();
        self.popups.cleanup();
        for output in self.outputs.keys() {
            layer_map_for_output(output).cleanup();
        }
        self.lock.cleanup();
        if self.lock.needs_relaunch() {
            warn!("the lock screen exited while the session is locked; restarting it");
            crate::spawn::spawn(self, &["m3-lock".to_owned()]);
        }
        self.idle_inhibitors.retain(smithay::utils::IsAlive::alive);
        let inhibited = !self.idle_inhibitors.is_empty();
        self.idle_notifier_state.set_is_inhibited(inhibited);
    }

    pub fn quit(&mut self) {
        info!("quit requested");
        self.quitting = true;
        if let Some(s) = &self.loop_signal {
            s.stop();
        }
    }

    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }
}
