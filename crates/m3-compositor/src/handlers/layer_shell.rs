use smithay::{
    delegate_layer_shell,
    desktop::{LayerSurface, PopupKind, WindowSurfaceType, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::wl_output::WlOutput,
    wayland::shell::{
        wlr_layer::{Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState},
        xdg::PopupSurface,
    },
};
use tracing::{debug, warn};

use crate::state::State;

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.m3.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        wl_output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.pointer_output())
            .or_else(|| self.m3.shell.outputs.first().cloned());
        let Some(output) = output else {
            warn!("layer surface `{namespace}` created without any output; closing it");
            surface.send_close();
            return;
        };
        debug!(%namespace, ?layer, output = output.name(), "new layer surface");
        let mut map = layer_map_for_output(&output);
        if let Err(e) = map.map_layer(&LayerSurface::new(surface, namespace)) {
            warn!("failed to map layer surface: {e}");
        }
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
        if let Err(e) = self.m3.popups.track_popup(PopupKind::Xdg(popup)) {
            debug!("failed to track layer popup: {e}");
        }
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let mut changed = false;
        for output in self.m3.shell.outputs.clone() {
            let mut map = layer_map_for_output(&output);
            let found = map.layer_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL).cloned();
            if let Some(layer) = found {
                map.unmap_layer(&layer);
                map.arrange();
                changed = true;
            }
        }
        if changed {
            for ws in 0..self.m3.shell.workspaces.len() {
                self.arrange(ws);
            }
            self.m3.queue_redraw_all();
            self.ensure_focus();
            self.update_pointer_focus();
        }
    }
}
delegate_layer_shell!(State);
