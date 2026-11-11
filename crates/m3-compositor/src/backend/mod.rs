//! Backends: where frames go and where input comes from.

pub mod headless;
pub mod udev;
pub mod winit;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        renderer::{ImportDma, gles::GlesRenderer},
    },
    delegate_dmabuf,
    input::keyboard::LedState,
    output::Output,
    reexports::{
        calloop::EventLoop,
        wayland_server::{DisplayHandle, protocol::wl_surface::WlSurface},
    },
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};

use crate::{
    Args, BackendKind,
    state::{M3, State},
};

// One backend for the whole process lifetime; boxing would only add an
// indirection on every render.
#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Winit(winit::WinitBackend),
    Headless(headless::HeadlessBackend),
    Udev(udev::UdevBackend),
}

impl Backend {
    pub fn init(
        kind: BackendKind,
        args: &Args,
        event_loop: &mut EventLoop<'static, State>,
        dh: &DisplayHandle,
    ) -> anyhow::Result<Self> {
        Ok(match kind {
            BackendKind::Winit => Backend::Winit(winit::WinitBackend::new(event_loop)?),
            BackendKind::Headless => Backend::Headless(headless::HeadlessBackend::new(args)?),
            BackendKind::Udev => Backend::Udev(udev::UdevBackend::new(event_loop, dh)?),
            BackendKind::Auto => unreachable!("resolved in main"),
        })
    }

    pub fn add_initial_outputs(&mut self, m3: &mut M3, args: &Args) -> anyhow::Result<()> {
        match self {
            Backend::Winit(b) => b.add_output(m3),
            Backend::Headless(b) => b.add_outputs(m3, args),
            Backend::Udev(b) => b.add_outputs(m3),
        }
    }

    pub fn shm_formats(&mut self) -> Vec<smithay::reexports::wayland_server::protocol::wl_shm::Format> {
        use smithay::backend::renderer::ImportMemWl;
        match self {
            Backend::Udev(b) => b.shm_formats(),
            _ => self.gles().map(|r| r.shm_formats().collect()).unwrap_or_default(),
        }
    }

    /// The renderer of single-GPU backends.
    fn gles(&mut self) -> Option<&mut GlesRenderer> {
        match self {
            Backend::Winit(b) => Some(b.renderer()),
            Backend::Headless(b) => Some(&mut b.renderer),
            Backend::Udev(_) => None,
        }
    }

    /// Render an output offscreen and read it back (screenshots).
    pub fn capture(
        &mut self,
        m3: &mut M3,
        output: &Output,
        include_cursor: bool,
    ) -> anyhow::Result<crate::screenshot::Capture> {
        match self {
            Backend::Udev(b) => b.capture(m3, output, include_cursor),
            _ => {
                let r = self.gles().ok_or_else(|| anyhow::anyhow!("no renderer"))?;
                crate::screenshot::capture(r, m3, output, include_cursor)
            }
        }
    }

    pub fn init_dmabuf(&mut self, dh: &DisplayHandle) {
        match self {
            Backend::Winit(b) => b.init_dmabuf(dh),
            Backend::Headless(b) => b.init_dmabuf(dh),
            Backend::Udev(b) => b.init_dmabuf(dh),
        }
    }

    fn dmabuf_state(&mut self) -> &mut DmabufState {
        match self {
            Backend::Winit(b) => &mut b.dmabuf_state,
            Backend::Headless(b) => &mut b.dmabuf_state,
            Backend::Udev(b) => &mut b.dmabuf_state,
        }
    }

    fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        match self {
            Backend::Udev(b) => b.import_dmabuf(dmabuf),
            _ => self.gles().is_some_and(|r| r.import_dmabuf(dmabuf, None).is_ok()),
        }
    }

    pub fn render(&mut self, m3: &mut M3, output: &Output) {
        match self {
            Backend::Winit(b) => b.render(m3, output),
            Backend::Headless(b) => b.render(m3, output),
            Backend::Udev(b) => b.render(m3, output),
        }
    }

    pub fn early_import(&mut self, surface: &WlSurface) {
        if let Backend::Udev(b) = self {
            b.early_import(surface);
        }
    }

    pub fn update_leds(&mut self, leds: LedState) {
        if let Backend::Udev(b) = self {
            b.update_leds(leds);
        }
    }

    pub fn switch_vt(&mut self, vt: i32) {
        if let Backend::Udev(b) = self {
            b.switch_vt(vt);
        }
    }

    pub fn device_added<D: smithay::backend::input::Device + 'static>(&mut self, m3: &mut M3, device: D) {
        if let Backend::Udev(b) = self {
            b.device_added(m3, device);
        }
    }

    /// Night light: apply a colour temperature to every display. Returns
    /// whether the backend can do it (only real displays can).
    pub fn set_color_temperature(&mut self, kelvin: f64) -> bool {
        match self {
            Backend::Udev(b) => {
                b.set_color_temperature(kelvin);
                true
            }
            _ => false,
        }
    }

    /// Turn displays off (idle) or let them come back on.
    pub fn set_displays_powered(&mut self, on: bool) {
        if let Backend::Udev(b) = self {
            b.set_displays_powered(on);
        }
    }

    pub fn apply_input_config(&mut self, input: &m3_core::config::Input) {
        if let Backend::Udev(b) = self {
            b.apply_input_config(input);
        }
    }

    pub fn apply_output_config(&mut self, m3: &mut M3) {
        match self {
            Backend::Udev(b) => b.apply_output_config(m3),
            Backend::Headless(b) => b.apply_output_config(m3),
            Backend::Winit(_) => {}
        }
    }

    /// Change a single output setting at runtime (IPC).
    pub fn configure_output(
        &mut self,
        m3: &mut M3,
        output: &Output,
        change: &m3_protocol::OutputChange,
    ) -> Result<(), String> {
        match self {
            Backend::Udev(b) => b.configure_output(m3, output, change),
            _ => generic_configure_output(m3, output, change),
        }
    }
}

/// Scale / transform / position changes that need no mode-setting.
pub fn generic_configure_output(
    m3: &mut M3,
    output: &Output,
    change: &m3_protocol::OutputChange,
) -> Result<(), String> {
    use m3_protocol::OutputChange;
    use smithay::output::Scale;
    match change {
        OutputChange::Scale { scale } => {
            if !(0.5..=4.0).contains(scale) {
                return Err(format!("scale {scale} is outside 0.5 … 4.0"));
            }
            // Round to the 1/120 steps wp-fractional-scale can express.
            let s = (scale * 120.0).round() / 120.0;
            output.change_current_state(None, None, Some(Scale::Fractional(s)), None);
        }
        OutputChange::Transform { transform } => {
            output.change_current_state(None, Some(to_smithay_transform(*transform)), None, None);
        }
        OutputChange::Position { x, y } => {
            output.change_current_state(None, None, None, Some((*x, *y).into()));
        }
        OutputChange::Mode { .. } => return Err("this backend cannot change modes".into()),
        OutputChange::Enabled { .. } => return Err("this backend cannot disable outputs".into()),
    }
    m3.shell.output_changed(output);
    layer_arrange(output);
    m3.queue_redraw_all();
    Ok(())
}

pub fn layer_arrange(output: &Output) {
    smithay::desktop::layer_map_for_output(output).arrange();
}

pub fn to_smithay_transform(t: m3_protocol::Transform) -> smithay::utils::Transform {
    use m3_protocol::Transform as T;
    use smithay::utils::Transform as S;
    match t {
        T::Normal => S::Normal,
        T::Rotate90 => S::_90,
        T::Rotate180 => S::_180,
        T::Rotate270 => S::_270,
        T::Flipped => S::Flipped,
        T::Flipped90 => S::Flipped90,
        T::Flipped180 => S::Flipped180,
        T::Flipped270 => S::Flipped270,
    }
}

pub fn from_smithay_transform(t: smithay::utils::Transform) -> m3_protocol::Transform {
    use m3_protocol::Transform as T;
    use smithay::utils::Transform as S;
    match t {
        S::Normal => T::Normal,
        S::_90 => T::Rotate90,
        S::_180 => T::Rotate180,
        S::_270 => T::Rotate270,
        S::Flipped => T::Flipped,
        S::Flipped90 => T::Flipped90,
        S::Flipped180 => T::Flipped180,
        S::Flipped270 => T::Flipped270,
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        self.backend.dmabuf_state()
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend.import_dmabuf(&dmabuf) {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State);
