//! Headless backend: virtual outputs rendered offscreen with GLES on an EGL
//! device (Mesa's software llvmpipe device works without any GPU). Used for
//! automated tests and CI; screenshots read the offscreen framebuffers.

use std::time::Duration;

use anyhow::Context;
use smithay::{
    backend::{
        allocator::Fourcc,
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{
            Bind, ImportDma, Offscreen,
            damage::OutputDamageTracker,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        wayland_server::DisplayHandle,
    },
    utils::{Size, Transform},
    wayland::dmabuf::DmabufState,
};
use tracing::{info, warn};

use crate::{
    Args,
    render::output_elements,
    state::{M3, OutputState, State},
};

const FRAME: Duration = Duration::from_micros(16_667);

struct HeadlessOutput {
    output: Output,
    texture: GlesTexture,
    damage_tracker: OutputDamageTracker,
    timer_pending: bool,
}

pub struct HeadlessBackend {
    pub renderer: GlesRenderer,
    outputs: Vec<HeadlessOutput>,
    pub dmabuf_state: DmabufState,
}

/// Parse `1920x1080,1280x720@1.5` into (width, height, scale) triples.
pub fn parse_output_spec(spec: &str) -> anyhow::Result<Vec<(i32, i32, f64)>> {
    spec.split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let (res, scale) = match s.trim().split_once('@') {
                Some((r, sc)) => (r, sc.parse::<f64>().context("bad scale")?),
                None => (s.trim(), 1.0),
            };
            let (w, h) = res.split_once('x').context("expected WIDTHxHEIGHT")?;
            let (w, h): (i32, i32) = (w.parse()?, h.parse()?);
            anyhow::ensure!(w > 0 && h > 0 && w <= 8192 && h <= 8192, "output size out of range");
            anyhow::ensure!((0.5..=4.0).contains(&scale), "scale out of range");
            Ok((w, h, scale))
        })
        .collect()
}

impl HeadlessBackend {
    pub fn new(_args: &Args) -> anyhow::Result<Self> {
        let devices: Vec<EGLDevice> = EGLDevice::enumerate().context("enumerating EGL devices")?.collect();
        // Prefer a hardware device, fall back to software rendering.
        let device = devices
            .iter()
            .find(|d| !d.is_software())
            .or_else(|| devices.first())
            .cloned()
            .context("no EGL device available (install Mesa)")?;
        info!(software = device.is_software(), "headless rendering on EGL device");
        // SAFETY: the device outlives the display, which the context keeps alive.
        let display = unsafe { EGLDisplay::new(device) }.context("creating EGL display")?;
        let context = EGLContext::new(&display).context("creating EGL context")?;
        // SAFETY: the context is current only on this thread.
        let renderer = unsafe { GlesRenderer::new(context) }.context("creating GLES renderer")?;
        Ok(Self { renderer, outputs: Vec::new(), dmabuf_state: DmabufState::new() })
    }

    pub fn add_outputs(&mut self, m3: &mut M3, args: &Args) -> anyhow::Result<()> {
        let specs = parse_output_spec(&args.headless_outputs)?;
        let mut x = 0;
        for (i, (w, h, scale)) in specs.into_iter().enumerate() {
            let output = Output::new(
                format!("HEADLESS-{}", i + 1),
                PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: Subpixel::Unknown,
                    make: "m3".into(),
                    model: "Virtual".into(),
                },
            );
            let mode = Mode { size: (w, h).into(), refresh: 60_000 };
            output.create_global::<State>(&m3.display_handle);
            output.change_current_state(
                Some(mode),
                Some(Transform::Normal),
                Some(Scale::Fractional(scale)),
                Some((x, 0).into()),
            );
            output.set_preferred(mode);
            x += (w as f64 / scale).round() as i32;
            let texture = self
                .renderer
                .create_buffer(Fourcc::Abgr8888, Size::from((w, h)))
                .context("allocating an offscreen framebuffer")?;
            m3.shell.add_output(&output);
            m3.outputs.insert(output.clone(), OutputState { redraw_queued: true, ..Default::default() });
            self.outputs.push(HeadlessOutput {
                damage_tracker: OutputDamageTracker::from_output(&output),
                output,
                texture,
                timer_pending: false,
            });
        }
        self.apply_output_config(m3);
        Ok(())
    }

    /// Apply scale/transform/position from config.toml to virtual outputs.
    pub fn apply_output_config(&mut self, m3: &mut M3) {
        for o in &self.outputs {
            if let Some(cfg) = m3.config.compositor.outputs.get(&o.output.name()) {
                let scale = cfg.scale.map(Scale::Fractional);
                let pos = cfg.position.map(|[x, y]| (x, y).into());
                o.output.change_current_state(
                    None,
                    Some(crate::backend::to_smithay_transform(cfg.transform)),
                    scale,
                    pos,
                );
                m3.shell.output_changed(&o.output);
            }
        }
    }

    pub fn init_dmabuf(&mut self, dh: &DisplayHandle) {
        let formats = self.renderer.dmabuf_formats();
        self.dmabuf_state.create_global::<State>(dh, formats);
    }

    pub fn render(&mut self, m3: &mut M3, output: &Output) {
        let Some(ho) = self.outputs.iter_mut().find(|o| &o.output == output) else { return };
        // Pace frames at 60 Hz like a real display would.
        let last = m3.outputs.get(output).and_then(|s| s.last_frame);
        if let Some(last) = last {
            let since = last.elapsed();
            if since < FRAME {
                if !ho.timer_pending {
                    ho.timer_pending = true;
                    let name = output.name();
                    let r = m3.loop_handle.insert_source(Timer::from_duration(FRAME - since), move |_, _, state| {
                        if let crate::backend::Backend::Headless(b) = &mut state.backend {
                            if let Some(o) = b.outputs.iter_mut().find(|o| o.output.name() == name) {
                                o.timer_pending = false;
                            }
                        }
                        TimeoutAction::Drop
                    });
                    if let Err(e) = r {
                        warn!("failed to schedule frame: {e}");
                    }
                }
                return;
            }
        }
        if let Some(s) = m3.outputs.get_mut(output) {
            s.redraw_queued = false;
        }

        let clear = crate::render::color32(m3.theme.colors.surface_dim);
        let elements = output_elements(m3, &mut self.renderer, output, true);
        let result = match self.renderer.bind(&mut ho.texture) {
            Ok(mut fb) => ho
                .damage_tracker
                .render_output(&mut self.renderer, &mut fb, 0, &elements, clear)
                .map(|r| r.states)
                .map_err(|e| format!("{e:?}")),
            Err(e) => Err(format!("{e:?}")),
        };
        match result {
            Ok(states) => {
                let feedback = m3.post_render(output, &states);
                crate::render::present_now(feedback, output, &m3.clock);
            }
            Err(e) => warn!("headless render failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_output_spec;

    #[test]
    fn output_specs() {
        assert_eq!(parse_output_spec("1920x1080").unwrap(), vec![(1920, 1080, 1.0)]);
        assert_eq!(parse_output_spec("800x600,1280x720@1.5").unwrap(), vec![(800, 600, 1.0), (1280, 720, 1.5)]);
        assert!(parse_output_spec("800").is_err());
        assert!(parse_output_spec("800x600@9").is_err());
    }
}
