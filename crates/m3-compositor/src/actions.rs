//! Executing [`Action`]s from keybindings and IPC.

use m3_core::config::LayoutKind;
use m3_protocol::{Action, Event};
use tracing::{info, warn};

use crate::{shell::WindowExt, spawn, state::State};

impl State {
    pub fn do_action(&mut self, action: Action) {
        if action.is_shell_action() {
            self.m3.ipc_event(Event::ShellAction { action });
            return;
        }
        if self.m3.lock.is_locked() && !matches!(action, Action::SwitchVt { .. } | Action::Quit) {
            return;
        }
        match action {
            Action::Spawn { command } => spawn::spawn(&self.m3, &command),
            Action::SpawnTerminal => spawn::spawn_terminal(&self.m3),
            Action::CloseWindow => {
                if let Some(w) = self.focused_window() {
                    w.close();
                }
            }
            Action::CloseWindowById { id } => {
                if let Some(w) = self.m3.shell.window_by_id(id) {
                    w.close();
                }
            }
            Action::FocusWindow { id } => {
                if let Some(w) = self.m3.shell.window_by_id(id) {
                    self.activate_window(&w);
                }
            }
            Action::FocusNext => self.focus_step(true),
            Action::FocusPrevious => self.focus_step(false),
            Action::FocusDirection { direction } => self.focus_direction(direction),
            Action::ToggleMaximize => {
                if let Some(w) = self.focused_window() {
                    let on = !w.st().maximized;
                    self.set_maximized(&w, on);
                }
            }
            Action::ToggleFullscreen => {
                if let Some(w) = self.focused_window() {
                    let on = !w.st().fullscreen;
                    self.set_fullscreen(&w, on, None);
                }
            }
            Action::Minimize => {
                if let Some(w) = self.focused_window() {
                    self.minimize(&w);
                }
            }
            Action::ToggleFloating => {
                if let Some(w) = self.focused_window() {
                    self.toggle_floating(&w);
                }
            }
            Action::Snap { direction } => {
                if let Some(w) = self.focused_window() {
                    self.snap_window(&w, direction);
                }
            }
            Action::SwitchWorkspace { index } => {
                if index >= 1 {
                    self.switch_workspace(index - 1);
                }
            }
            Action::NextWorkspace => self.switch_workspace_relative(1),
            Action::PreviousWorkspace => self.switch_workspace_relative(-1),
            Action::MoveToWorkspace { index } => {
                if let (Some(w), true) = (self.focused_window(), index >= 1) {
                    self.move_window_to_workspace(&w, index - 1);
                }
            }
            Action::MoveWindowToWorkspace { id, index } => {
                if let (Some(w), true) = (self.m3.shell.window_by_id(id), index >= 1) {
                    self.move_window_to_workspace(&w, index - 1);
                }
            }
            Action::CycleLayout => {
                let ws = self.m3.shell.active;
                let next = self.m3.shell.active_ws().layout.next();
                self.set_layout(ws, next);
            }
            Action::SetLayout { layout } => match LayoutKind::from_name(&layout) {
                Some(l) => {
                    let ws = self.m3.shell.active;
                    self.set_layout(ws, l);
                }
                None => warn!("unknown layout `{layout}`"),
            },
            Action::CycleWindows { reverse } => self.cycle_windows(reverse),
            Action::Lock => spawn::spawn(&self.m3, &["m3-lock".to_owned()]),
            Action::DisplaysOff => {
                if !self.m3.screens_off {
                    self.m3.screens_off = true;
                    self.backend.set_displays_powered(false);
                }
            }
            Action::Screenshot => {
                if let Some(path) = crate::screenshot::default_path() {
                    if let Err(e) = self.screenshot(None, &path, None) {
                        warn!("screenshot failed: {e:#}");
                    } else {
                        info!("screenshot saved to {}", path.display());
                        self.m3.ipc_event(Event::ScreenshotSaved { path });
                    }
                }
            }
            Action::ReloadConfig => self.reload_config(),
            Action::RestartShell => spawn::restart_shell(&mut self.m3),
            Action::SwitchVt { vt } => self.backend.switch_vt(vt),
            Action::Quit => self.m3.quit(),
            // Shell actions were forwarded above.
            Action::ToggleOverview
            | Action::ToggleLauncher
            | Action::ToggleQuickSettings
            | Action::ToggleNotifications
            | Action::VolumeUp
            | Action::VolumeDown
            | Action::VolumeMute
            | Action::MicMute
            | Action::BrightnessUp
            | Action::BrightnessDown
            | Action::MediaPlayPause
            | Action::MediaNext
            | Action::MediaPrevious
            | Action::ScreenshotRegion => {}
        }
    }

    pub fn reload_config(&mut self) {
        let (config, errors) = crate::config::Config::reload(&self.m3.config_dir, &self.m3.config);
        for e in &errors {
            warn!("{e}");
        }
        self.m3.bindings = crate::input::Bindings::from_config(&config.keybindings);
        let kb = &config.compositor.input.keyboard;
        if let Some(keyboard) = self.m3.seat.get_keyboard() {
            keyboard.change_repeat_info(kb.repeat_rate, kb.repeat_delay);
            let xkb = smithay::input::keyboard::XkbConfig {
                rules: "",
                model: &kb.model,
                layout: &kb.layout,
                variant: &kb.variant,
                options: if kb.options.is_empty() { None } else { Some(kb.options.clone()) },
            };
            if let Err(e) = keyboard.set_xkb_config(self, xkb) {
                warn!("keyboard layout `{}` is invalid: {e}", kb.layout);
            }
        }
        self.m3.cursor.reconfigure(&config.compositor.general.cursor_theme, config.compositor.general.cursor_size);
        self.m3.theme = crate::config::resolve_theme(&config);
        self.m3.config = config;
        self.m3.shell.apply_config(&self.m3.config);
        self.backend.apply_input_config(&self.m3.config.compositor.input);
        self.backend.apply_output_config(&mut self.m3);
        for ws in 0..self.m3.shell.workspaces.len() {
            self.arrange(ws);
        }
        self.m3.queue_redraw_all();
        self.update_night_light();
        self.m3.ipc_event(Event::ConfigReloaded);
        info!("configuration reloaded ({} errors)", errors.len());
    }
}
