//! Window management: workspaces, windows and their arrangement.
//!
//! Model: a list of global workspaces. Each workspace owns a smithay
//! [`Space`] into which every output is mapped at its global position, so a
//! workspace spans all monitors. Windows remember their "home" output, which
//! the tiling layouts use to group windows per monitor.

pub mod layout;
pub mod ops;
pub mod window;

use m3_core::config::LayoutKind;
use smithay::{
    desktop::{Space, Window, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, Rectangle},
};

use crate::config::Config;
pub use window::{WindowExt, init_window};

pub struct Workspace {
    pub name: String,
    pub space: Space<Window>,
    pub layout: LayoutKind,
    /// All windows of the workspace (mapped and minimized), in tiling order.
    pub windows: Vec<Window>,
    /// Focus history, most recent first.
    pub mru: Vec<Window>,
}

impl Workspace {
    fn new(name: String, layout: LayoutKind) -> Self {
        Self { name, space: Space::default(), layout, windows: Vec::new(), mru: Vec::new() }
    }

    pub fn is_mapped(&self, w: &Window) -> bool {
        self.space.elements().any(|e| e == w)
    }

    pub fn touch_mru(&mut self, w: &Window) {
        self.mru.retain(|x| x != w);
        self.mru.insert(0, w.clone());
    }

    pub fn forget(&mut self, w: &Window) {
        self.space.unmap_elem(w);
        self.windows.retain(|x| x != w);
        self.mru.retain(|x| x != w);
    }

    /// Topmost-first list of mapped windows.
    pub fn stacking_top_first(&self) -> Vec<Window> {
        let mut v: Vec<Window> = self.space.elements().cloned().collect();
        v.reverse();
        v
    }
}

pub struct Shell {
    pub workspaces: Vec<Workspace>,
    /// Index of the active workspace (0-based).
    pub active: usize,
    pub outputs: Vec<Output>,
    /// Toplevels that exist but have no buffer yet (or unmapped themselves).
    pub pending: Vec<Window>,
    /// Unmanaged X11 windows (menus, tooltips) drawn above everything else.
    pub override_redirect: Vec<Window>,
    next_id: u64,
    pub wrap_around: bool,
    /// Something IPC clients care about changed.
    pub dirty: bool,
}

impl Shell {
    pub fn new(config: &Config) -> Self {
        let mut s = Self {
            workspaces: Vec::new(),
            active: 0,
            outputs: Vec::new(),
            pending: Vec::new(),
            override_redirect: Vec::new(),
            next_id: 1,
            wrap_around: false,
            dirty: true,
        };
        s.apply_config(config);
        s
    }

    pub fn next_window_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Apply workspace count/names/layout defaults. Shrinking moves windows of
    /// removed workspaces onto the new last one; nothing is ever closed.
    pub fn apply_config(&mut self, config: &Config) {
        let wc = &config.workspaces;
        self.wrap_around = wc.wrap_around;
        while self.workspaces.len() < wc.count {
            let idx = self.workspaces.len() + 1;
            let mut ws = Workspace::new(wc.name_of(idx), wc.default_layout);
            for o in &self.outputs {
                ws.space.map_output(o, o.current_location());
            }
            self.workspaces.push(ws);
        }
        while self.workspaces.len() > wc.count.max(1) {
            let Some(removed) = self.workspaces.pop() else { break };
            let last = self.workspaces.len() - 1;
            for w in removed.windows {
                let loc = removed.space.element_location(&w);
                let target = &mut self.workspaces[last];
                if let Some(loc) = loc {
                    target.space.map_element(w.clone(), loc, false);
                }
                target.windows.push(w);
            }
        }
        for (i, ws) in self.workspaces.iter_mut().enumerate() {
            ws.name = wc.name_of(i + 1);
        }
        self.active = self.active.min(self.workspaces.len() - 1);
        self.dirty = true;
    }

    pub fn active_ws(&self) -> &Workspace {
        &self.workspaces[self.active]
    }

    pub fn active_ws_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active]
    }

    pub fn active_space(&self) -> &Space<Window> {
        &self.workspaces[self.active].space
    }

    // ---- outputs -------------------------------------------------------

    pub fn add_output(&mut self, output: &Output) {
        if !self.outputs.contains(output) {
            self.outputs.push(output.clone());
        }
        let loc = output.current_location();
        for ws in &mut self.workspaces {
            ws.space.map_output(output, loc);
        }
        self.dirty = true;
    }

    /// Re-map an output after its position, mode, scale or transform changed.
    pub fn output_changed(&mut self, output: &Output) {
        self.add_output(output);
    }

    pub fn remove_output(&mut self, output: &Output) {
        self.outputs.retain(|o| o != output);
        for ws in &mut self.workspaces {
            ws.space.unmap_output(output);
        }
        let name = output.name();
        let fallback = self.outputs.first().map(|o| o.name());
        for w in self.all_windows() {
            let mut st = w.st_mut();
            if st.output.as_deref() == Some(name.as_str()) {
                st.output = fallback.clone();
            }
        }
        self.dirty = true;
    }

    /// Logical geometry of an output in the global space.
    pub fn output_geometry(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        self.workspaces.first().and_then(|ws| ws.space.output_geometry(output))
    }

    /// The part of an output not reserved by panels (layer-shell exclusive
    /// zones), in global coordinates.
    pub fn usable_area(&self, output: &Output) -> Rectangle<i32, Logical> {
        let geo = self.output_geometry(output).unwrap_or_default();
        let mut zone = layer_map_for_output(output).non_exclusive_zone();
        zone.loc += geo.loc;
        if zone.size.w <= 0 || zone.size.h <= 0 { geo } else { zone }
    }

    pub fn output_at(&self, p: Point<f64, Logical>) -> Option<&Output> {
        self.outputs.iter().find(|o| self.output_geometry(o).is_some_and(|g| g.to_f64().contains(p)))
    }

    pub fn output_by_name(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.name() == name)
    }

    /// The output a window belongs to: its home output if still connected,
    /// otherwise the output containing its center, otherwise the first one.
    pub fn output_of(&self, w: &Window) -> Option<Output> {
        if let Some(name) = w.st().output.clone() {
            if let Some(o) = self.output_by_name(&name) {
                return Some(o.clone());
            }
        }
        if let Some(ws) = self.workspace_of(w) {
            if let Some(geo) = self.workspaces[ws].space.element_geometry(w) {
                let c = window::rect_center(geo).to_f64();
                if let Some(o) = self.output_at(c) {
                    return Some(o.clone());
                }
            }
        }
        self.outputs.first().cloned()
    }

    /// Bounding box of all outputs.
    pub fn global_bounds(&self) -> Rectangle<i32, Logical> {
        self.outputs.iter().filter_map(|o| self.output_geometry(o)).reduce(|a, b| a.merge(b)).unwrap_or_default()
    }

    // ---- windows -------------------------------------------------------

    pub fn all_windows(&self) -> Vec<Window> {
        self.workspaces.iter().flat_map(|ws| ws.windows.iter().cloned()).collect()
    }

    pub fn workspace_of(&self, w: &Window) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.windows.contains(w))
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.windows.iter())
            .chain(self.pending.iter())
            .find(|w| w.surface().as_ref() == Some(surface))
            .cloned()
    }

    pub fn mapped_window_for_surface(&self, surface: &WlSurface) -> Option<(usize, Window)> {
        self.workspaces.iter().enumerate().find_map(|(i, ws)| {
            ws.windows.iter().find(|w| w.surface().as_ref() == Some(surface)).map(|w| (i, w.clone()))
        })
    }

    pub fn window_by_id(&self, id: u64) -> Option<Window> {
        self.workspaces.iter().flat_map(|ws| ws.windows.iter()).find(|w| w.id() == id).cloned()
    }

    /// The topmost fullscreen window on `output` in the active workspace.
    pub fn fullscreen_on(&self, output: &Output) -> Option<Window> {
        let ws = self.active_ws();
        ws.stacking_top_first().into_iter().find(|w| w.st().fullscreen && self.output_of(w).as_ref() == Some(output))
    }

    /// Drop dead windows and let the spaces update output enter/leave.
    pub fn refresh(&mut self) {
        let before: usize = self.workspaces.iter().map(|w| w.windows.len()).sum();
        for ws in &mut self.workspaces {
            ws.windows.retain(|w| w.alive());
            ws.mru.retain(|w| w.alive());
        }
        self.pending.retain(|w| w.alive());
        self.override_redirect.retain(|w| w.alive());
        self.workspaces[self.active].space.refresh();
        let after: usize = self.workspaces.iter().map(|w| w.windows.len()).sum();
        if before != after {
            self.dirty = true;
        }
    }
}
