use smithay::{
    backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state},
    delegate_compositor, delegate_shm,
    desktop::{PopupKind, WindowSurfaceType, layer_map_for_output},
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState, is_sync_subsurface, with_states},
        shell::{
            wlr_layer::LayerSurfaceData,
            xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData},
        },
        shm::{ShmHandler, ShmState},
    },
    xwayland::XWaylandClientData,
};

use super::root_surface;
use crate::{shell::WindowExt, state::State};

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.m3.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        if let Some(state) = client.get_data::<crate::state::ClientState>() {
            return &state.compositor_state;
        }
        unreachable!("every client is inserted with ClientState or XWaylandClientData")
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.backend.early_import(surface);

        if is_sync_subsurface(surface) {
            return;
        }
        let root = root_surface(surface);

        // Toplevels: map on first buffer, unmap on null buffer.
        if let Some(window) = self.m3.shell.window_for_surface(&root) {
            window.on_commit();
            if &root == surface {
                let mapped_now = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
                let in_workspace = self.m3.shell.workspace_of(&window).is_some();
                let pending = self.m3.shell.pending.contains(&window);
                if pending && mapped_now {
                    self.map_window(window.clone());
                } else if in_workspace && !mapped_now && !window.is_x11() {
                    self.unmap_window(&window);
                }
                if let Some(t) = window.toplevel() {
                    let initial_sent = with_states(surface, |s| {
                        s.data_map
                            .get::<XdgToplevelSurfaceData>()
                            .is_some_and(|d| d.lock().is_ok_and(|d| d.initial_configure_sent))
                    });
                    if !initial_sent {
                        self.prepare_initial_configure(&window);
                        t.send_configure();
                    }
                }
            }
            self.on_window_commit(&window);
        }

        // Popups.
        self.m3.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.m3.popups.find_popup(surface) {
            {
                let initial_sent = with_states(surface, |s| {
                    s.data_map
                        .get::<XdgPopupSurfaceData>()
                        .is_some_and(|d| d.lock().is_ok_and(|d| d.initial_configure_sent))
                });
                if !initial_sent {
                    if let Err(e) = xdg.send_configure() {
                        tracing::debug!("popup initial configure failed: {e}");
                    }
                }
            }
        }

        // Layer surfaces.
        let mut layer_output = None;
        for output in self.m3.shell.outputs.clone() {
            let mut map = layer_map_for_output(&output);
            if map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL).is_none() {
                continue;
            }
            let zone_before = map.non_exclusive_zone();
            map.arrange();
            let zone_changed = map.non_exclusive_zone() != zone_before;
            let initial_sent = with_states(&root, |s| {
                s.data_map.get::<LayerSurfaceData>().is_some_and(|d| d.lock().is_ok_and(|d| d.initial_configure_sent))
            });
            if !initial_sent {
                if let Some(layer) = map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL) {
                    layer.layer_surface().send_configure();
                }
            }
            drop(map);
            if zone_changed {
                for ws in 0..self.m3.shell.workspaces.len() {
                    self.arrange(ws);
                }
            }
            layer_output = Some(output);
            break;
        }
        if layer_output.is_some() {
            self.update_pointer_focus();
        }

        // Lock surfaces and the cursor surface always trigger a redraw too.
        self.m3.queue_redraw_all();
    }
}

impl State {
    /// Decide the initial state of a new toplevel before its first configure,
    /// so rules like "start maximized" don't cause a visible resize.
    fn prepare_initial_configure(&mut self, window: &smithay::desktop::Window) {
        let (app_id, title) = (window.app_id(), window.title());
        let rules = self.m3.config.compositor.window_rules.clone();
        for rule in rules.iter().filter(|r| r.matches(app_id.as_deref(), title.as_deref())) {
            if let Some([w, h]) = rule.size {
                window.request_size(Some((w, h).into()));
            }
        }
        if let Some(t) = window.toplevel() {
            let mode = self.preferred_decoration_mode();
            t.with_pending_state(|s| {
                if s.decoration_mode.is_none() {
                    s.decoration_mode = Some(mode);
                }
            });
        }
        // Tell the client which output it will appear on (scale hints).
        if let Some(output) = self.pointer_output() {
            if let Some(surface) = window.surface() {
                output.enter(&surface);
            }
        }
    }

    fn on_window_commit(&mut self, window: &smithay::desktop::Window) {
        crate::grabs::handle_resize_commit(self, window);
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.m3.shm_state
    }
}

delegate_compositor!(State);
delegate_shm!(State);
