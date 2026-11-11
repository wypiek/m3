//! Nested backend: m3 runs as a window inside another Wayland/X11 session.

use anyhow::Context;
use smithay::{
    backend::{
        egl::EGLDevice,
        renderer::{ImportDma, damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{calloop::EventLoop, wayland_server::DisplayHandle},
    utils::Transform,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
};
use tracing::{error, warn};

use crate::{
    render::output_elements,
    state::{M3, OutputState, State},
};

pub struct WinitBackend {
    backend: WinitGraphicsBackend<GlesRenderer>,
    output: Option<Output>,
    damage_tracker: Option<OutputDamageTracker>,
    pub dmabuf_state: DmabufState,
    full_redraw: u8,
}

impl WinitBackend {
    pub fn new(event_loop: &mut EventLoop<'static, State>) -> anyhow::Result<Self> {
        let (backend, winit) = winit::init::<GlesRenderer>()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("opening the nested m3 window (is a Wayland or X11 session running?)")?;
        event_loop
            .handle()
            .insert_source(winit, |event, _, state| match event {
                WinitEvent::Resized { size, .. } => {
                    if let crate::backend::Backend::Winit(b) = &mut state.backend {
                        b.full_redraw = 4;
                        if let Some(output) = b.output.clone() {
                            let mode = Mode { size, refresh: 60_000 };
                            output.change_current_state(Some(mode), None, None, None);
                            output.set_preferred(mode);
                            state.m3.shell.output_changed(&output);
                            crate::backend::layer_arrange(&output);
                            for ws in 0..state.m3.shell.workspaces.len() {
                                state.arrange(ws);
                            }
                            state.m3.ipc_outputs_changed();
                        }
                    }
                    state.m3.queue_redraw_all();
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => state.m3.queue_redraw_all(),
                WinitEvent::CloseRequested => state.m3.quit(),
                WinitEvent::Focus(_) => {}
            })
            .map_err(|e| anyhow::anyhow!("inserting the winit source: {e}"))?;
        Ok(Self { backend, output: None, damage_tracker: None, dmabuf_state: DmabufState::new(), full_redraw: 0 })
    }

    pub fn renderer(&mut self) -> &mut GlesRenderer {
        self.backend.renderer()
    }

    pub fn add_output(&mut self, m3: &mut M3) -> anyhow::Result<()> {
        let size = self.backend.window_size();
        let mode = Mode { size, refresh: 60_000 };
        let output = Output::new(
            "winit-0".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "m3".into(),
                model: "Nested".into(),
            },
        );
        output.create_global::<State>(&m3.display_handle);
        output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
        output.set_preferred(mode);
        m3.shell.add_output(&output);
        m3.outputs.insert(output.clone(), OutputState { redraw_queued: true, ..Default::default() });
        self.damage_tracker = Some(OutputDamageTracker::from_output(&output));
        self.output = Some(output);
        // The nested window draws our cursor; hide the host's.
        self.backend.window().set_cursor_visible(false);
        Ok(())
    }

    pub fn init_dmabuf(&mut self, dh: &DisplayHandle) {
        let formats = self.backend.renderer().dmabuf_formats();
        let node = EGLDevice::device_for_display(self.backend.renderer().egl_context().display())
            .ok()
            .and_then(|d| d.try_get_render_node().ok().flatten());
        match node.and_then(|n| DmabufFeedbackBuilder::new(n.dev_id(), formats.clone()).build().ok()) {
            Some(feedback) => {
                self.dmabuf_state.create_global_with_default_feedback::<State>(dh, &feedback);
            }
            None => {
                warn!("no render node for dmabuf feedback; using dmabuf v3");
                self.dmabuf_state.create_global::<State>(dh, formats);
            }
        }
    }

    pub fn render(&mut self, m3: &mut M3, output: &Output) {
        if let Some(s) = m3.outputs.get_mut(output) {
            s.redraw_queued = false;
        }
        let Some(damage_tracker) = self.damage_tracker.as_mut() else { return };
        self.full_redraw = self.full_redraw.saturating_sub(1);
        let age = if self.full_redraw > 0 { 0 } else { self.backend.buffer_age().unwrap_or(0) };

        let result = {
            let (renderer, mut fb) = match self.backend.bind() {
                Ok(x) => x,
                Err(e) => {
                    error!("failed to bind the nested window: {e}");
                    return;
                }
            };
            let clear = crate::render::color32(m3.theme.colors.surface_dim);
            let elements = output_elements(m3, renderer, output, true);
            damage_tracker
                .render_output(renderer, &mut fb, age, &elements, clear)
                .map(|r| (r.damage.cloned(), r.states))
        };
        match result {
            Ok((damage, states)) => {
                if let Some(damage) = damage {
                    if let Err(e) = self.backend.submit(Some(&damage)) {
                        warn!("failed to present nested frame: {e}");
                    }
                }
                let feedback = m3.post_render(output, &states);
                crate::render::present_now(feedback, output, &m3.clock);
            }
            Err(e) => warn!("rendering failed: {e:?}"),
        }
    }
}
