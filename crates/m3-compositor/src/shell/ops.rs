//! Window-management operations. These live on [`State`] because changing
//! focus needs the seat and the full compositor state.

use m3_core::config::LayoutKind;
use m3_protocol::Direction;
use smithay::{
    desktop::{Window, layer_map_for_output},
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer},
};
use tracing::debug;

use super::{WindowExt, layout, window::clamp_size};
use crate::{focus::KeyboardFocusTarget, state::State};

const TILED_STATES: [xdg_toplevel::State; 4] = [
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

/// An in-progress Alt+Tab cycle.
#[derive(Debug, Clone)]
pub struct WindowCycleState {
    pub windows: Vec<Window>,
    pub index: usize,
}

impl State {
    // ---- focus ---------------------------------------------------------

    pub fn keyboard_focus(&self) -> Option<KeyboardFocusTarget> {
        self.m3.seat.get_keyboard().and_then(|k| k.current_focus())
    }

    pub fn focused_window(&self) -> Option<Window> {
        match self.keyboard_focus()? {
            KeyboardFocusTarget::Window(w) => Some(w),
            KeyboardFocusTarget::Popup(p) => {
                let root = smithay::desktop::find_popup_root_surface(&p).ok()?;
                self.m3.shell.window_for_surface(&root)
            }
            _ => None,
        }
    }

    pub fn set_keyboard_focus(&mut self, target: Option<KeyboardFocusTarget>) {
        let Some(keyboard) = self.m3.seat.get_keyboard() else { return };
        if keyboard.current_focus() == target {
            return;
        }
        keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
    }

    /// A layer surface that demands exclusive keyboard focus (launcher,
    /// power menu, …), if any.
    fn exclusive_layer(&self) -> Option<KeyboardFocusTarget> {
        for output in &self.m3.shell.outputs {
            let map = layer_map_for_output(output);
            for layer in [Layer::Overlay, Layer::Top] {
                if let Some(l) = map
                    .layers_on(layer)
                    .rev()
                    .find(|l| l.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive)
                {
                    return Some(KeyboardFocusTarget::Layer(l.clone()));
                }
            }
        }
        None
    }

    /// Re-establish a sensible keyboard focus: the lock surface while locked,
    /// exclusive layer surfaces, otherwise keep the current focus while it's
    /// alive and visible, else fall back to the most recent window.
    pub fn ensure_focus(&mut self) {
        if self.m3.lock.is_locked() {
            let target = self.m3.lock.focus_target(self.pointer_output().as_ref());
            self.set_keyboard_focus(target);
            return;
        }
        if let Some(l) = self.exclusive_layer() {
            self.set_keyboard_focus(Some(l));
            return;
        }
        let current = self.keyboard_focus();
        let valid = match &current {
            None => false,
            Some(KeyboardFocusTarget::Window(w)) => {
                smithay::utils::IsAlive::alive(w) && self.m3.shell.active_ws().is_mapped(w)
            }
            Some(KeyboardFocusTarget::Lock(_)) => false,
            Some(t) => smithay::utils::IsAlive::alive(t),
        };
        if valid {
            return;
        }
        // A grab (popup menu) may hold a dead focus briefly; only replace
        // focus that is actually stale.
        let next = self.m3.shell.active_ws().mru.iter().find(|w| self.m3.shell.active_ws().is_mapped(w)).cloned();
        match next {
            Some(w) => self.focus_window(&w),
            None => {
                if current.is_some() {
                    self.set_keyboard_focus(None);
                }
            }
        }
    }

    /// Raise and focus a window on the active workspace.
    pub fn focus_window(&mut self, w: &Window) {
        let ws = self.m3.shell.active_ws_mut();
        if !ws.is_mapped(w) {
            return;
        }
        ws.space.raise_element(w, true);
        ws.touch_mru(w);
        if let Some(xwm) = self.m3.xwm.as_mut() {
            if let Some(x) = w.x11_surface() {
                if let Err(e) = xwm.raise_window(x) {
                    debug!("x11 raise failed: {e}");
                }
            }
        }
        self.m3.shell.dirty = true;
        self.m3.queue_redraw_all();
        if self.m3.lock.is_locked() || self.exclusive_layer().is_some() {
            return;
        }
        self.set_keyboard_focus(Some(KeyboardFocusTarget::Window(w.clone())));
    }

    /// Bring a window to the user: switch to its workspace, restore it if
    /// minimized, raise and focus it.
    pub fn activate_window(&mut self, w: &Window) {
        let Some(ws) = self.m3.shell.workspace_of(w) else { return };
        if ws != self.m3.shell.active {
            self.switch_workspace(ws);
        }
        if w.st().minimized {
            self.unminimize(w);
        }
        self.focus_window(w);
    }

    /// Update xdg "activated" states to match the keyboard focus.
    pub fn update_activation(&mut self, focused: Option<&Window>) {
        for w in self.m3.shell.active_ws().windows.clone() {
            let on = Some(&w) == focused;
            if w.set_activated(on) {
                w.send_configure();
            }
            if w.is_x11() {
                w.set_xdg_state(xdg_toplevel::State::Activated, on);
            }
        }
        self.m3.shell.dirty = true;
    }

    // ---- lifecycle -----------------------------------------------------

    /// A pending toplevel committed its first buffer: place and show it.
    pub fn map_window(&mut self, w: Window) {
        self.m3.shell.pending.retain(|p| p != &w);
        let (app_id, title) = (w.app_id(), w.title());
        let cfg = &self.m3.config.compositor;

        let mut target_ws = self.m3.shell.active;
        let mut forced_size = None;
        if !w.st().rules_applied {
            let mut st = w.st_mut();
            st.rules_applied = true;
            st.floating = w.is_transient();
            for rule in cfg.window_rules.iter().filter(|r| r.matches(app_id.as_deref(), title.as_deref())) {
                if let Some(f) = rule.floating {
                    st.floating = f;
                }
                if let Some(ws) = rule.workspace {
                    target_ws = (ws - 1).min(self.m3.shell.workspaces.len() - 1);
                }
                if let Some(m) = rule.maximized {
                    st.maximized = m;
                }
                if let Some(f) = rule.fullscreen {
                    st.fullscreen = f;
                }
                if let Some([sw, sh]) = rule.size {
                    forced_size = Some((sw, sh).into());
                }
                if let Some(o) = rule.opacity {
                    st.opacity = o as f32;
                }
            }
        } else if let Some(ws) = self.m3.shell.workspace_of(&w) {
            target_ws = ws;
        }

        let output = self.pointer_output().or_else(|| self.m3.shell.outputs.first().cloned());
        if w.st().output.is_none() {
            w.st_mut().output = output.as_ref().map(|o| o.name());
        }
        let area = output.as_ref().map(|o| self.m3.shell.usable_area(o)).unwrap_or_default();

        let mut size = w.geometry().size;
        if let Some(s) = forced_size {
            let (min, max) = w.min_max_size();
            size = clamp_size(s, min, max);
            w.request_size(Some(size));
            w.send_configure();
        }

        let ws = &mut self.m3.shell.workspaces[target_ws];
        let loc = if let Some(loc) = w.st_mut().minimized_loc.take().filter(|_| w.st().placed) {
            loc
        } else if let Some(loc) = ws.space.element_location(&w).filter(|_| w.st().placed) {
            loc
        } else {
            let occupied: Vec<Point<i32, Logical>> =
                ws.space.elements().filter_map(|e| ws.space.element_location(e)).collect();
            layout::place_floating(area, size, &occupied, 32)
        };
        if !w.st().placed || w.st().minimized {
            w.st_mut().opened_at = Some(std::time::Instant::now());
        }
        w.st_mut().placed = true;
        w.st_mut().minimized = false;
        if !ws.windows.contains(&w) {
            ws.windows.push(w.clone());
        }
        ws.space.map_element(w.clone(), loc, false);
        w.set_x11_geometry(Rectangle::new(loc, size));

        let (maximized, fullscreen) = (w.st().maximized, w.st().fullscreen);
        if maximized {
            w.st_mut().maximized = false;
            self.set_maximized(&w, true);
        }
        if fullscreen {
            w.st_mut().fullscreen = false;
            self.set_fullscreen(&w, true, None);
        }
        self.arrange(target_ws);
        self.m3.shell.dirty = true;
        self.m3.queue_redraw_all();

        if target_ws == self.m3.shell.active && self.m3.config.compositor.general.focus_new_windows {
            self.focus_window(&w);
        } else if target_ws == self.m3.shell.active {
            self.m3.shell.workspaces[target_ws].mru.push(w.clone());
        } else {
            self.m3.shell.workspaces[target_ws].mru.insert(0, w.clone());
        }
        debug!(id = w.id(), ?app_id, ws = target_ws + 1, "mapped window");
    }

    /// The client removed its buffer: hide the window but remember it.
    pub fn unmap_window(&mut self, w: &Window) {
        if let Some(ws) = self.m3.shell.workspace_of(w) {
            let loc = self.m3.shell.workspaces[ws].space.element_location(w);
            self.m3.shell.workspaces[ws].forget(w);
            w.st_mut().minimized_loc = loc;
            self.m3.shell.pending.push(w.clone());
            self.arrange(ws);
        }
        self.after_window_gone();
    }

    pub fn destroy_window(&mut self, w: &Window) {
        if let Some(ws) = self.m3.shell.workspace_of(w) {
            self.m3.shell.workspaces[ws].forget(w);
            self.arrange(ws);
        }
        self.m3.shell.pending.retain(|p| p != w);
        if let Some(c) = &mut self.m3.cycle {
            c.windows.retain(|x| x != w);
        }
        self.after_window_gone();
    }

    fn after_window_gone(&mut self) {
        self.m3.shell.dirty = true;
        self.m3.queue_redraw_all();
        self.ensure_focus();
        self.update_pointer_focus();
    }

    // ---- arrangement ---------------------------------------------------

    fn managed_by_layout(&self, ws: usize, w: &Window) -> bool {
        let st = w.st();
        self.m3.shell.workspaces[ws].layout != LayoutKind::Floating
            && !st.floating
            && !st.maximized
            && !st.fullscreen
            && !st.minimized
    }

    /// Apply the workspace layout and maximize/fullscreen geometry.
    pub fn arrange(&mut self, ws_idx: usize) {
        let outputs = self.m3.shell.outputs.clone();
        let gap = self.m3.config.compositor.windows.gaps;
        let ratio = self.m3.config.compositor.windows.master_ratio;
        for output in &outputs {
            let area = self.m3.shell.usable_area(output);
            let out_geo = self.m3.shell.output_geometry(output).unwrap_or(area);
            let windows: Vec<Window> = self.m3.shell.workspaces[ws_idx]
                .windows
                .iter()
                .filter(|w| self.m3.shell.workspaces[ws_idx].is_mapped(w))
                .filter(|w| self.m3.shell.output_of(w).as_ref() == Some(output))
                .cloned()
                .collect();

            let tiled: Vec<Window> = windows.iter().filter(|w| self.managed_by_layout(ws_idx, w)).cloned().collect();
            let rects = match self.m3.shell.workspaces[ws_idx].layout {
                LayoutKind::Floating => Vec::new(),
                LayoutKind::Tiling => layout::master_stack(area, tiled.len(), ratio, gap),
                LayoutKind::Monocle => layout::monocle(area, tiled.len(), gap),
            };
            for (w, r) in tiled.iter().zip(rects) {
                for s in TILED_STATES {
                    w.set_xdg_state(s, true);
                }
                self.place(ws_idx, w, r);
            }
            let floating: Vec<Window> =
                windows.iter().filter(|w| !self.managed_by_layout(ws_idx, w)).cloned().collect();
            for w in &floating {
                let st = w.st().clone();
                if st.fullscreen {
                    self.place(ws_idx, w, out_geo);
                } else if st.maximized {
                    self.place(ws_idx, w, area);
                } else if !st.snapped {
                    for s in TILED_STATES {
                        w.set_xdg_state(s, false);
                    }
                    w.send_configure();
                }
            }
        }
        self.m3.shell.dirty = true;
        self.m3.queue_redraw_all();
    }

    fn place(&mut self, ws_idx: usize, w: &Window, r: Rectangle<i32, Logical>) {
        let (min, max) = w.min_max_size();
        let size = clamp_size(r.size, min, max);
        w.request_size(Some(size));
        w.send_configure();
        w.set_x11_geometry(Rectangle::new(r.loc, size));
        self.m3.shell.workspaces[ws_idx].space.map_element(w.clone(), r.loc, false);
    }

    fn window_geometry(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        let ws = self.m3.shell.workspace_of(w)?;
        self.m3.shell.workspaces[ws].space.element_geometry(w)
    }

    fn save_restore(&self, w: &Window) {
        let st = w.st();
        let floating_now = !st.maximized && !st.fullscreen && !st.snapped;
        drop(st);
        if floating_now {
            w.st_mut().restore = self.window_geometry(w);
        }
    }

    fn restore_floating(&mut self, w: &Window) {
        let Some(ws) = self.m3.shell.workspace_of(w) else { return };
        let restore = w.st_mut().restore.take();
        match restore {
            Some(r) => {
                w.request_size(Some(r.size));
                w.send_configure();
                w.set_x11_geometry(r);
                self.m3.shell.workspaces[ws].space.map_element(w.clone(), r.loc, false);
            }
            None => {
                w.request_size(None);
                w.send_configure();
            }
        }
    }

    pub fn set_maximized(&mut self, w: &Window, on: bool) {
        if w.st().maximized == on {
            return;
        }
        if on {
            self.save_restore(w);
        }
        {
            let mut st = w.st_mut();
            st.maximized = on;
            st.snapped = false;
        }
        w.set_xdg_state(xdg_toplevel::State::Maximized, on);
        if !on && !w.st().fullscreen {
            self.restore_floating(w);
        }
        if let Some(ws) = self.m3.shell.workspace_of(w) {
            self.arrange(ws);
        }
    }

    pub fn set_fullscreen(&mut self, w: &Window, on: bool, output: Option<Output>) {
        if w.st().fullscreen == on {
            return;
        }
        if on {
            self.save_restore(w);
            if let Some(o) = output {
                w.st_mut().output = Some(o.name());
            }
        }
        w.st_mut().fullscreen = on;
        w.set_xdg_state(xdg_toplevel::State::Fullscreen, on);
        if !on {
            if w.st().maximized {
                w.set_xdg_state(xdg_toplevel::State::Maximized, true);
            } else {
                self.restore_floating(w);
            }
        }
        if let Some(ws) = self.m3.shell.workspace_of(w) {
            self.arrange(ws);
        }
        if on {
            self.focus_window(w);
        }
    }

    pub fn minimize(&mut self, w: &Window) {
        let Some(ws) = self.m3.shell.workspace_of(w) else { return };
        let loc = self.m3.shell.workspaces[ws].space.element_location(w);
        {
            let mut st = w.st_mut();
            st.minimized = true;
            st.minimized_loc = loc;
        }
        self.m3.shell.workspaces[ws].space.unmap_elem(w);
        if let Some(x) = w.x11_surface() {
            let _ = x.set_suspended(true);
        }
        self.arrange(ws);
        self.after_window_gone();
    }

    pub fn unminimize(&mut self, w: &Window) {
        let Some(ws) = self.m3.shell.workspace_of(w) else { return };
        let loc = {
            let mut st = w.st_mut();
            st.minimized = false;
            st.minimized_loc.take()
        };
        let loc = loc.unwrap_or_else(|| {
            let area = self.m3.shell.output_of(w).map(|o| self.m3.shell.usable_area(&o)).unwrap_or_default();
            layout::place_floating(area, w.geometry().size, &[], 32)
        });
        if let Some(x) = w.x11_surface() {
            let _ = x.set_suspended(false);
        }
        self.m3.shell.workspaces[ws].space.map_element(w.clone(), loc, false);
        self.arrange(ws);
    }

    pub fn toggle_floating(&mut self, w: &Window) {
        let floating = !w.st().floating;
        w.st_mut().floating = floating;
        if floating {
            self.restore_floating(w);
        } else {
            self.save_restore(w);
        }
        if let Some(ws) = self.m3.shell.workspace_of(w) {
            self.arrange(ws);
        }
    }

    /// Super+arrow behaviour.
    pub fn snap_window(&mut self, w: &Window, dir: Direction) {
        let Some(ws) = self.m3.shell.workspace_of(w) else { return };
        if self.managed_by_layout(ws, w) {
            // In tiling layouts left/right promote to master / demote to stack.
            let list = &mut self.m3.shell.workspaces[ws].windows;
            if let Some(i) = list.iter().position(|x| x == w) {
                let item = list.remove(i);
                match dir {
                    Direction::Left | Direction::Up => list.insert(0, item),
                    Direction::Right | Direction::Down => list.push(item),
                }
            }
            self.arrange(ws);
            return;
        }
        match dir {
            Direction::Up => self.set_maximized(w, true),
            Direction::Down => {
                if w.st().maximized {
                    self.set_maximized(w, false);
                } else if w.st().snapped {
                    w.st_mut().snapped = false;
                    self.restore_floating(w);
                } else {
                    self.minimize(w);
                }
            }
            Direction::Left | Direction::Right => {
                if w.st().maximized {
                    self.set_maximized(w, false);
                }
                let Some(output) = self.m3.shell.output_of(w) else { return };
                let area = self.m3.shell.usable_area(&output);
                let r = layout::snap(area, dir, self.m3.config.compositor.windows.gaps);
                self.save_restore(w);
                w.st_mut().snapped = true;
                let (l, rr) = (dir == Direction::Left, dir == Direction::Right);
                w.set_xdg_state(xdg_toplevel::State::TiledTop, true);
                w.set_xdg_state(xdg_toplevel::State::TiledBottom, true);
                w.set_xdg_state(xdg_toplevel::State::TiledLeft, rr);
                w.set_xdg_state(xdg_toplevel::State::TiledRight, l);
                self.place(ws, w, r);
            }
        }
        self.m3.shell.dirty = true;
    }

    /// Called when an interactive move starts on a snapped/maximized window:
    /// restore its floating size around the pointer.
    pub fn unsnap_for_move(&mut self, w: &Window, pointer: Point<f64, Logical>) -> Option<Point<i32, Logical>> {
        let (snapped, maximized) = (w.st().snapped, w.st().maximized);
        if !snapped && !maximized {
            return None;
        }
        let geo = self.window_geometry(w)?;
        let restore = w.st().restore?;
        let fx = ((pointer.x - geo.loc.x as f64) / geo.size.w.max(1) as f64).clamp(0.0, 1.0);
        let new_loc = Point::from((pointer.x as i32 - (restore.size.w as f64 * fx) as i32, pointer.y as i32 - 16));
        w.st_mut().restore = Some(Rectangle::new(new_loc, restore.size));
        if maximized {
            self.set_maximized(w, false);
        } else {
            w.st_mut().snapped = false;
            for s in TILED_STATES {
                w.set_xdg_state(s, false);
            }
            self.restore_floating(w);
        }
        Some(new_loc)
    }

    // ---- workspaces ----------------------------------------------------

    pub fn switch_workspace(&mut self, idx: usize) {
        if idx >= self.m3.shell.workspaces.len() || idx == self.m3.shell.active {
            return;
        }
        self.m3.cycle = None;
        self.m3.shell.active = idx;
        self.m3.shell.dirty = true;
        self.m3.queue_redraw_all();
        let next = self.m3.shell.active_ws().mru.iter().find(|w| self.m3.shell.active_ws().is_mapped(w)).cloned();
        match next {
            Some(w) => self.focus_window(&w),
            None => {
                if !self.m3.lock.is_locked() && self.exclusive_layer().is_none() {
                    self.set_keyboard_focus(None);
                }
            }
        }
        self.update_pointer_focus();
    }

    pub fn switch_workspace_relative(&mut self, delta: i32) {
        let n = self.m3.shell.workspaces.len() as i32;
        let cur = self.m3.shell.active as i32;
        let next = if self.m3.shell.wrap_around { (cur + delta).rem_euclid(n) } else { (cur + delta).clamp(0, n - 1) };
        self.switch_workspace(next as usize);
    }

    pub fn move_window_to_workspace(&mut self, w: &Window, idx: usize) {
        let Some(from) = self.m3.shell.workspace_of(w) else { return };
        if idx >= self.m3.shell.workspaces.len() || idx == from {
            return;
        }
        let loc = self.m3.shell.workspaces[from].space.element_location(w);
        self.m3.shell.workspaces[from].forget(w);
        let to = &mut self.m3.shell.workspaces[idx];
        to.windows.push(w.clone());
        to.mru.insert(0, w.clone());
        if !w.st().minimized {
            if let Some(loc) = loc {
                to.space.map_element(w.clone(), loc, false);
            }
        }
        self.arrange(from);
        self.arrange(idx);
        self.after_window_gone();
    }

    pub fn set_layout(&mut self, ws: usize, layout: LayoutKind) {
        let Some(workspace) = self.m3.shell.workspaces.get_mut(ws) else { return };
        if workspace.layout == layout {
            return;
        }
        let tiled_before: Vec<Window> = workspace.windows.clone();
        workspace.layout = layout;
        if layout == LayoutKind::Floating {
            for w in &tiled_before {
                if !w.st().floating && !w.st().maximized && !w.st().fullscreen {
                    for s in TILED_STATES {
                        w.set_xdg_state(s, false);
                    }
                    self.restore_floating(w);
                }
            }
        } else {
            for w in &tiled_before {
                if !w.st().floating && w.st().restore.is_none() {
                    self.save_restore(w);
                }
            }
        }
        self.arrange(ws);
    }

    // ---- focus navigation ----------------------------------------------

    pub fn focus_step(&mut self, forward: bool) {
        let ws = self.m3.shell.active_ws();
        let list: Vec<Window> = ws.windows.iter().filter(|w| ws.is_mapped(w)).cloned().collect();
        if list.is_empty() {
            return;
        }
        let cur = self.focused_window().and_then(|f| list.iter().position(|w| *w == f));
        let n = list.len();
        let next = match cur {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        let w = list[next].clone();
        self.focus_window(&w);
    }

    pub fn focus_direction(&mut self, dir: Direction) {
        let Some(cur) = self.focused_window() else { return self.focus_step(true) };
        let space = &self.m3.shell.active_ws().space;
        let Some(cg) = space.element_geometry(&cur) else { return };
        let c = super::window::rect_center(cg);
        let best = space
            .elements()
            .filter(|w| **w != cur)
            .filter_map(|w| space.element_geometry(w).map(|g| (w.clone(), super::window::rect_center(g))))
            .filter(|(_, p)| match dir {
                Direction::Left => p.x < c.x,
                Direction::Right => p.x > c.x,
                Direction::Up => p.y < c.y,
                Direction::Down => p.y > c.y,
            })
            .min_by_key(|(_, p)| {
                let (dx, dy) = ((p.x - c.x) as i64, (p.y - c.y) as i64);
                // Penalise off-axis distance so "left" prefers windows level with us.
                match dir {
                    Direction::Left | Direction::Right => dx.abs() + 2 * dy.abs(),
                    Direction::Up | Direction::Down => dy.abs() + 2 * dx.abs(),
                }
            })
            .map(|(w, _)| w);
        if let Some(w) = best {
            self.focus_window(&w);
        }
    }

    /// Alt+Tab: step through the focus history; [`Self::end_cycle`] commits.
    pub fn cycle_windows(&mut self, reverse: bool) {
        if self.m3.cycle.is_none() {
            let ws = self.m3.shell.active_ws();
            let mut windows = ws.mru.clone();
            for w in &ws.windows {
                if !windows.contains(w) {
                    windows.push(w.clone());
                }
            }
            if windows.len() < 2 {
                return;
            }
            self.m3.cycle = Some(WindowCycleState { windows, index: 0 });
        }
        let Some(c) = self.m3.cycle.as_mut() else { return };
        let n = c.windows.len();
        c.index = if reverse { (c.index + n - 1) % n } else { (c.index + 1) % n };
        let w = c.windows[c.index].clone();
        if w.st().minimized {
            self.unminimize(&w);
        }
        // Raise and focus without reordering the history mid-cycle.
        let ws = self.m3.shell.active_ws_mut();
        ws.space.raise_element(&w, true);
        self.set_keyboard_focus(Some(KeyboardFocusTarget::Window(w)));
        self.m3.queue_redraw_all();
    }

    pub fn end_cycle(&mut self) {
        if let Some(c) = self.m3.cycle.take() {
            if let Some(w) = c.windows.get(c.index) {
                let w = w.clone();
                self.focus_window(&w);
            }
        }
    }
}
