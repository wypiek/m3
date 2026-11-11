//! DRM/KMS + libinput backend for real sessions.
//!
//! * Session and device access through libseat (logind or seatd); m3 never
//!   needs root.
//! * GPUs are discovered and hot-plugged through udev; every connected
//!   connector becomes an [`Output`]. Rendering uses a `GpuManager` so
//!   outputs on secondary GPUs are rendered on the primary GPU and copied.
//! * Frames are paced by vblank: an output renders at most one frame per
//!   vblank. When nothing changed, frame callbacks are throttled with an
//!   estimated-vblank timer so idle clients cannot busy-loop the compositor.

use std::{any::Any, collections::HashMap, path::Path, time::Duration};

use anyhow::Context;
use smithay::{
    backend::{
        allocator::{
            Fourcc, Modifier,
            dmabuf::Dmabuf,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventMetadata, DrmEventTime, DrmNode, NodeType,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLDevice, EGLDisplay, context::ContextPriority},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportDma, ImportMemWl,
            gles::GlesRenderer,
            multigpu::{GpuManager, MultiRenderer, gbm::GbmGlesBackend},
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend as UdevMonitor, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::utils::OutputPresentationFeedback,
    input::keyboard::LedState,
    output::{Mode as WlMode, Output, PhysicalProperties, Scale},
    reexports::{
        calloop::{
            EventLoop, LoopHandle, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        drm::{
            Device as _,
            control::{Device as _, ModeTypeFlags, connector, crtc},
        },
        input::{self as libinput, AccelProfile as LiAccel, ClickMethod, DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{DisplayHandle, backend::GlobalId, protocol::wl_surface::WlSurface},
    },
    utils::DeviceFd,
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufState},
        presentation::Refresh,
    },
};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{debug, error, info, warn};

use crate::{
    render::{OutputElement, output_elements},
    state::{M3, OutputState, State},
};

const SUPPORTED_FORMATS: &[Fourcc] = &[Fourcc::Abgr2101010, Fourcc::Argb2101010, Fourcc::Abgr8888, Fourcc::Argb8888];

type GbmGles = GbmGlesBackend<GlesRenderer, DrmDeviceFd>;
type UdevRenderer<'a> = MultiRenderer<'a, 'a, GbmGles, GbmGles>;
type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
type Feedback = Option<OutputPresentationFeedback>;
type OutputManager = DrmOutputManager<Allocator, Exporter, Feedback, DrmDeviceFd>;
type DrmOut = DrmOutput<Allocator, Exporter, Feedback, DrmDeviceFd>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputId {
    node: DrmNode,
    crtc: crtc::Handle,
}

struct Surface {
    output: Output,
    drm_output: DrmOut,
    connector: connector::Info,
    global: Option<GlobalId>,
    /// A frame is queued or we're waiting for the estimated vblank.
    frame_pending: bool,
    estimated_vblank: Option<RegistrationToken>,
}

struct GpuDevice {
    /// Rendering happens in software on this device.
    software: bool,
    manager: OutputManager,
    scanner: DrmScanner,
    render_node: Option<DrmNode>,
    surfaces: HashMap<crtc::Handle, Surface>,
    token: RegistrationToken,
}

pub struct UdevBackend {
    session: LibSeatSession,
    libinput: Libinput,
    primary_gpu: DrmNode,
    gpus: GpuManager<GbmGles>,
    devices: HashMap<DrmNode, GpuDevice>,
    pub dmabuf_state: DmabufState,
    dmabuf_global: Option<DmabufGlobal>,
    input_devices: Vec<libinput::Device>,
    input_config: m3_core::config::Input,
    loop_handle: LoopHandle<'static, State>,
    dh: DisplayHandle,
}

fn backend(state: &mut State) -> Option<(&mut UdevBackend, &mut M3)> {
    match &mut state.backend {
        crate::backend::Backend::Udev(b) => Some((b, &mut state.m3)),
        _ => None,
    }
}

impl UdevBackend {
    pub fn new(event_loop: &mut EventLoop<'static, State>, dh: &DisplayHandle) -> anyhow::Result<Self> {
        let (session, notifier) = LibSeatSession::new()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("opening a seat session (is logind or seatd running, and is this a real VT?)")?;
        let seat = session.seat();
        info!(%seat, "session opened");

        let primary = std::env::var("M3_DRM_DEVICE")
            .ok()
            .and_then(|p| DrmNode::from_path(p).ok())
            .or_else(|| {
                primary_gpu(&seat)
                    .ok()
                    .flatten()
                    .and_then(|p| DrmNode::from_path(p).ok())
                    .and_then(|n| n.node_with_type(NodeType::Render).and_then(Result::ok).or(Some(n)))
            })
            .or_else(|| all_gpus(&seat).ok()?.into_iter().find_map(|p| DrmNode::from_path(p).ok()))
            .context("no GPU found")?;
        info!(%primary, "primary GPU");

        let gpus = GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("initializing the GPU manager")?;

        let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
        libinput.udev_assign_seat(&seat).map_err(|()| anyhow::anyhow!("libinput: cannot assign seat {seat}"))?;
        let input_backend = LibinputInputBackend::new(libinput.clone());

        let handle = event_loop.handle();
        handle
            .insert_source(input_backend, |event, _, state| {
                if let InputEvent::DeviceRemoved { device } = &event {
                    if let Some((b, _)) = backend(state) {
                        b.input_devices.retain(|d| d != device);
                    }
                }
                state.process_input_event(event);
            })
            .map_err(|e| anyhow::anyhow!("inserting libinput source: {e}"))?;

        handle
            .insert_source(notifier, |event, _, state| match event {
                SessionEvent::PauseSession => {
                    info!("session paused (VT switch away)");
                    if let Some((b, _)) = backend(state) {
                        b.libinput.suspend();
                        for d in b.devices.values_mut() {
                            d.manager.pause();
                            for s in d.surfaces.values_mut() {
                                s.frame_pending = false;
                            }
                        }
                    }
                }
                SessionEvent::ActivateSession => {
                    info!("session resumed");
                    if let Some((b, m3)) = backend(state) {
                        if b.libinput.resume().is_err() {
                            error!("failed to resume libinput");
                        }
                        for d in b.devices.values_mut() {
                            if let Err(e) = d.manager.activate(false) {
                                error!("failed to reactivate DRM device: {e}");
                            }
                        }
                        m3.queue_redraw_all();
                        // Another DRM master may have reset the gamma.
                        m3.night_light_applied = None;
                        let _ = m3.loop_handle.insert_idle(|state| state.update_night_light());
                    }
                }
            })
            .map_err(|e| anyhow::anyhow!("inserting session source: {e}"))?;

        let monitor = UdevMonitor::new(&seat).context("starting the udev monitor")?;
        // Hot-plug events are inserted after the initial scan (add_outputs).
        handle
            .insert_source(monitor, |event, _, state| {
                match event {
                    UdevEvent::Added { device_id, path } => {
                        if let Ok(node) = DrmNode::from_dev_id(device_id) {
                            if let Some((b, m3)) = backend(state) {
                                if let Err(e) = b.gpu_added(m3, node, &path) {
                                    warn!("skipping GPU {path:?}: {e:#}");
                                }
                            }
                        }
                    }
                    UdevEvent::Changed { device_id } => {
                        if let Ok(node) = DrmNode::from_dev_id(device_id) {
                            if let Some((b, m3)) = backend(state) {
                                b.device_changed(m3, node);
                            }
                        }
                    }
                    UdevEvent::Removed { device_id } => {
                        if let Ok(node) = DrmNode::from_dev_id(device_id) {
                            if let Some((b, m3)) = backend(state) {
                                b.device_removed(m3, node);
                            }
                        }
                    }
                }
                state.outputs_changed();
            })
            .map_err(|e| anyhow::anyhow!("inserting udev source: {e}"))?;

        Ok(Self {
            session,
            libinput,
            primary_gpu: primary,
            gpus,
            devices: HashMap::new(),
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            input_devices: Vec::new(),
            input_config: Default::default(),
            loop_handle: handle,
            dh: dh.clone(),
        })
    }

    /// Initial scan of all GPUs and connectors.
    pub fn add_outputs(&mut self, m3: &mut M3) -> anyhow::Result<()> {
        self.input_config = m3.config.compositor.input.clone();
        let seat = self.session.seat();
        let gpus = all_gpus(&seat).context("listing GPUs")?;
        // Primary GPU first so secondary GPUs can share its allocator.
        let mut paths: Vec<_> = gpus.into_iter().collect();
        paths.sort_by_key(|p| {
            DrmNode::from_path(p)
                .ok()
                .and_then(|n| n.node_with_type(NodeType::Render).and_then(Result::ok))
                .map(|n| n != self.primary_gpu)
                .unwrap_or(true)
        });
        for path in paths {
            let Ok(node) = DrmNode::from_path(&path) else { continue };
            if let Err(e) = self.gpu_added(m3, node, &path) {
                warn!("skipping GPU {}: {e:#}", path.display());
            }
        }
        anyhow::ensure!(!self.devices.is_empty(), "no usable GPU could be opened");
        Ok(())
    }

    /// Render `output` offscreen on the primary GPU (screenshots).
    pub fn capture(
        &mut self,
        m3: &mut M3,
        output: &Output,
        include_cursor: bool,
    ) -> anyhow::Result<crate::screenshot::Capture> {
        let primary = self.primary_gpu;
        let mut renderer = self.gpus.single_renderer(&primary).map_err(|e| anyhow::anyhow!("{e}"))?;
        crate::screenshot::capture(&mut renderer, m3, output, include_cursor)
    }

    /// Night light: a gamma ramp for `kelvin` on every CRTC (legacy gamma
    /// ioctl; atomic drivers implement it through the GAMMA_LUT helper).
    pub fn set_color_temperature(&mut self, kelvin: f64) {
        use smithay::reexports::drm::control::Device as _;
        let (r, g, b) = crate::nightlight::white_point(kelvin);
        for d in self.devices.values() {
            let dev = d.manager.device();
            for crtc in d.surfaces.keys() {
                let len = match dev.get_crtc(*crtc) {
                    Ok(info) => info.gamma_length() as usize,
                    Err(e) => {
                        warn!("night light: cannot query CRTC: {e}");
                        continue;
                    }
                };
                if len < 2 {
                    continue;
                }
                let (rr, gg, bb) =
                    (crate::nightlight::ramp(len, r), crate::nightlight::ramp(len, g), crate::nightlight::ramp(len, b));
                if let Err(e) = dev.set_gamma(*crtc, &rr, &gg, &bb) {
                    warn!("night light: setting gamma failed: {e}");
                }
            }
        }
    }

    /// Power displays off (DPMS) or allow them back on; the next frame
    /// re-enables a cleared output.
    pub fn set_displays_powered(&mut self, on: bool) {
        for d in self.devices.values_mut() {
            for s in d.surfaces.values_mut() {
                if !on {
                    if let Err(e) = s.drm_output.with_compositor(|c| c.clear()) {
                        warn!("turning {} off failed: {e}", s.output.name());
                    }
                }
                s.frame_pending = false;
            }
        }
    }

    pub fn shm_formats(&mut self) -> Vec<smithay::reexports::wayland_server::protocol::wl_shm::Format> {
        let primary = self.primary_gpu;
        self.gpus.single_renderer(&primary).map(|r| r.shm_formats().collect()).unwrap_or_default()
    }

    pub fn init_dmabuf(&mut self, dh: &DisplayHandle) {
        let primary = self.primary_gpu;
        let Ok(renderer) = self.gpus.single_renderer(&primary) else { return };
        let formats = renderer.dmabuf_formats();
        match DmabufFeedbackBuilder::new(primary.dev_id(), formats.clone()).build() {
            Ok(feedback) => {
                self.dmabuf_global =
                    Some(self.dmabuf_state.create_global_with_default_feedback::<State>(dh, &feedback));
            }
            Err(e) => {
                warn!("dmabuf feedback unavailable ({e}); using dmabuf v3");
                self.dmabuf_global = Some(self.dmabuf_state.create_global::<State>(dh, formats));
            }
        }
    }

    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        let primary = self.primary_gpu;
        self.gpus.single_renderer(&primary).is_ok_and(|mut r| r.import_dmabuf(dmabuf, None).is_ok())
    }

    pub fn early_import(&mut self, surface: &WlSurface) {
        let primary = self.primary_gpu;
        if let Err(e) = self.gpus.early_import(primary, surface) {
            debug!("early import failed: {e}");
        }
    }

    // ---- devices -----------------------------------------------------------

    fn gpu_added(&mut self, m3: &mut M3, node: DrmNode, path: &Path) -> anyhow::Result<()> {
        if self.devices.contains_key(&node) {
            return Ok(());
        }
        let fd = self
            .session
            .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("opening the device through the session")?;
        let fd = DrmDeviceFd::new(DeviceFd::from(fd));
        let (drm, notifier) = DrmDevice::new(fd.clone(), true).context("initializing DRM")?;
        let gbm = GbmDevice::new(fd).context("initializing GBM")?;

        let token = self
            .loop_handle
            .insert_source(notifier, move |event, metadata, state| match event {
                DrmEvent::VBlank(crtc) => {
                    if let Some((b, m3)) = backend(state) {
                        b.on_vblank(m3, node, crtc, metadata);
                    }
                    state.render_queued();
                }
                DrmEvent::Error(e) => error!("DRM error: {e}"),
            })
            .map_err(|e| anyhow::anyhow!("inserting DRM source: {e}"))?;

        // SAFETY: the GBM device stays alive as long as the EGL display.
        // Prefer hardware rendering; fall back to Mesa's software renderer
        // (llvmpipe via kms_swrast) only when no hardware renderer exists,
        // e.g. virtual machines without 3D acceleration.
        let have_hw = self.devices.values().any(|d| d.render_node.is_some() && !d.software);
        let mut software = false;
        let render_node = unsafe { EGLDisplay::new(gbm.clone()) }
            .ok()
            .and_then(|display| EGLDevice::device_for_display(&display).ok())
            .filter(|dev| {
                software = dev.is_software();
                if software && have_hw {
                    return false;
                }
                if software {
                    warn!("no hardware renderer for {node}; using software rendering (slow)");
                }
                true
            })
            .map(|dev| dev.try_get_render_node().ok().flatten().unwrap_or(node))
            .and_then(|rn| match self.gpus.as_mut().add_node(rn, gbm.clone()) {
                Ok(()) => Some(rn),
                Err(e) => {
                    warn!("cannot render on {rn}: {e}");
                    None
                }
            });

        let allocator = match render_node {
            Some(_) => GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT),
            None => self
                .devices
                .values()
                .find(|d| d.render_node == Some(self.primary_gpu))
                .map(|d| d.manager.allocator().clone())
                .context("display-only GPU but the primary GPU is not initialized")?,
        };
        // The first device that can render becomes the primary GPU if the
        // udev-reported primary cannot.
        if let Some(rn) = render_node {
            if self.gpus.single_renderer(&self.primary_gpu).is_err() {
                info!(%rn, "using {rn} as the primary GPU");
                self.primary_gpu = rn;
            }
        }
        let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);
        let render_with = render_node.unwrap_or(self.primary_gpu);
        let mut renderer = self.gpus.single_renderer(&render_with).map_err(|e| anyhow::anyhow!("{e}"))?;
        let render_formats: FormatSet = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .filter(|f| render_node.is_some() || f.modifier == Modifier::Linear)
            .copied()
            .collect();
        let manager = DrmOutputManager::new(
            drm,
            allocator,
            exporter,
            Some(gbm),
            SUPPORTED_FORMATS.iter().copied(),
            render_formats,
        );
        info!(%node, ?render_node, "GPU added");
        self.devices.insert(
            node,
            GpuDevice { software, manager, scanner: DrmScanner::new(), render_node, surfaces: HashMap::new(), token },
        );
        self.device_changed(m3, node);
        Ok(())
    }

    fn device_changed(&mut self, m3: &mut M3, node: DrmNode) {
        let Some(device) = self.devices.get_mut(&node) else { return };
        let events = match device.scanner.scan_connectors(device.manager.device()) {
            Ok(r) => r,
            Err(e) => {
                warn!("connector scan failed: {e}");
                return;
            }
        };
        for event in events {
            match event {
                DrmScanEvent::Connected { connector, crtc: Some(crtc) } => {
                    self.connector_connected(m3, node, connector, crtc)
                }
                DrmScanEvent::Disconnected { connector, crtc: Some(crtc) } => {
                    self.connector_disconnected(m3, node, connector, crtc)
                }
                _ => {}
            }
        }
    }

    fn device_removed(&mut self, m3: &mut M3, node: DrmNode) {
        let Some(device) = self.devices.get(&node) else { return };
        let crtcs: Vec<_> = device.scanner.crtcs().map(|(info, crtc)| (info.clone(), crtc)).collect();
        for (conn, crtc) in crtcs {
            self.connector_disconnected(m3, node, conn, crtc);
        }
        if let Some(d) = self.devices.remove(&node) {
            if let Some(rn) = d.render_node {
                self.gpus.as_mut().remove_node(&rn);
            }
            self.loop_handle.remove(d.token);
        }
        info!(%node, "GPU removed");
    }

    fn connector_connected(&mut self, m3: &mut M3, node: DrmNode, conn: connector::Info, crtc: crtc::Handle) {
        let Self { devices, gpus, primary_gpu, dh, .. } = self;
        let primary = *primary_gpu;
        let Some(device) = devices.get_mut(&node) else { return };
        let name = format!("{}-{}", conn.interface().as_str(), conn.interface_id());
        let drm = device.manager.device();

        let non_desktop = drm
            .get_properties(conn.handle())
            .ok()
            .and_then(|props| {
                props.into_iter().find_map(|(h, v)| {
                    let info = drm.get_property(h).ok()?;
                    (info.name().to_str() == Ok("non-desktop"))
                        .then(|| info.value_type().convert_value(v).as_boolean())?
                })
            })
            .unwrap_or(false);
        if non_desktop {
            info!("{name} is a non-desktop display (VR headset); not using it");
            return;
        }

        let info = edid_info(drm, conn.handle());
        let make = info.as_ref().and_then(|i| i.make()).unwrap_or_else(|| "Unknown".into());
        let model = info.as_ref().and_then(|i| i.model()).unwrap_or_else(|| "Unknown".into());
        let serial = info.as_ref().and_then(|i| i.serial()).unwrap_or_default();

        // Configuration may name the output by connector or by "make model serial".
        let desc = format!("{make} {model} {serial}").trim().to_owned();
        let cfg = m3.config.compositor.outputs.get(&name).or_else(|| m3.config.compositor.outputs.get(&desc)).cloned();
        if cfg.as_ref().is_some_and(|c| !c.enabled) {
            info!("{name} is disabled in config.toml");
            return;
        }

        let modes = conn.modes();
        if modes.is_empty() {
            warn!("{name} reports no modes");
            return;
        }
        let wanted =
            cfg.as_ref().and_then(|c| c.mode.as_deref()).and_then(|m| m3_core::config::OutputMode::parse(m).ok());
        let drm_mode = wanted
            .and_then(|w| pick_mode(modes, w.width, w.height, w.refresh_mhz))
            .or_else(|| modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)).copied())
            .unwrap_or(modes[0]);
        let wl_mode = WlMode::from(drm_mode);

        let (mm_w, mm_h) = conn.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (mm_w as i32, mm_h as i32).into(),
                subpixel: conn.subpixel().into(),
                make,
                model,
            },
        );
        let global = output.create_global::<State>(dh);
        for m in modes {
            output.add_mode(WlMode::from(*m));
        }
        output.set_preferred(wl_mode);

        let scale = cfg.as_ref().and_then(|c| c.scale).unwrap_or_else(|| auto_scale(wl_mode.size.w, mm_w));
        let transform = cfg
            .as_ref()
            .map(|c| crate::backend::to_smithay_transform(c.transform))
            .unwrap_or(smithay::utils::Transform::Normal);
        let position = cfg.as_ref().and_then(|c| c.position).map(|[x, y]| (x, y).into()).unwrap_or_else(|| {
            // Right of everything that is already there.
            let x = m3.shell.global_bounds();
            (if m3.shell.outputs.is_empty() { 0 } else { x.loc.x + x.size.w }, 0).into()
        });
        output.change_current_state(Some(wl_mode), Some(transform), Some(Scale::Fractional(scale)), Some(position));
        output.user_data().insert_if_missing(|| OutputId { node, crtc });

        let render_node = device.render_node.unwrap_or(primary);
        let mut renderer = match gpus.single_renderer(&render_node) {
            Ok(r) => r,
            Err(e) => {
                warn!("no renderer for {name}: {e}");
                return;
            }
        };
        let mut planes = drm.planes(&crtc).ok();
        // NVIDIA's overlay planes are unreliable; composite instead.
        if drm.get_driver().map(|d| d.name().to_string_lossy().to_lowercase().contains("nvidia")).unwrap_or(false) {
            if let Some(p) = planes.as_mut() {
                p.overlay.clear();
            }
        }
        let drm_output = match device.manager.initialize_output::<_, OutputElement<UdevRenderer<'_>>>(
            crtc,
            drm_mode,
            &[conn.handle()],
            &output,
            planes,
            &mut renderer,
            &DrmOutputRenderElements::default(),
        ) {
            Ok(o) => o,
            Err(e) => {
                warn!("failed to light up {name}: {e}");
                return;
            }
        };
        info!(%name, mode = ?wl_mode, scale, "output connected");
        device.surfaces.insert(
            crtc,
            Surface {
                output: output.clone(),
                drm_output,
                connector: conn,
                global: Some(global),
                frame_pending: false,
                estimated_vblank: None,
            },
        );
        m3.shell.add_output(&output);
        m3.outputs.insert(output, OutputState { redraw_queued: true, ..Default::default() });
    }

    fn connector_disconnected(&mut self, m3: &mut M3, node: DrmNode, _conn: connector::Info, crtc: crtc::Handle) {
        let Self { devices, gpus, primary_gpu, dh, loop_handle, .. } = self;
        let Some(device) = devices.get_mut(&node) else { return };
        if let Some(mut s) = device.surfaces.remove(&crtc) {
            info!(name = s.output.name(), "output disconnected");
            if let Some(t) = s.estimated_vblank.take() {
                loop_handle.remove(t);
            }
            m3.shell.remove_output(&s.output);
            m3.outputs.remove(&s.output);
            if let Some(g) = s.global.take() {
                dh.disable_global::<State>(g.clone());
                // Give clients a moment to release it before removing.
                let dh = dh.clone();
                let _ = loop_handle.insert_source(Timer::from_duration(Duration::from_secs(5)), move |_, _, _| {
                    dh.remove_global::<State>(g.clone());
                    TimeoutAction::Drop
                });
            }
        }
        let render_node = device.render_node.unwrap_or(*primary_gpu);
        if let Ok(mut renderer) = gpus.single_renderer(&render_node) {
            let _ = device.manager.try_to_restore_modifiers::<_, OutputElement<UdevRenderer<'_>>>(
                &mut renderer,
                &DrmOutputRenderElements::default(),
            );
        }
    }

    // ---- rendering ---------------------------------------------------------

    pub fn render(&mut self, m3: &mut M3, output: &Output) {
        let Some(&OutputId { node, crtc }) = output.user_data().get::<OutputId>() else { return };
        let Self { devices, gpus, session, loop_handle, primary_gpu, .. } = self;
        let primary = *primary_gpu;
        let Some(device) = devices.get_mut(&node) else { return };
        let render_node = device.render_node.unwrap_or(primary);
        let Some(surface) = device.surfaces.get_mut(&crtc) else { return };
        if surface.frame_pending || !session.is_active() {
            return;
        }
        if let Some(s) = m3.outputs.get_mut(output) {
            s.redraw_queued = false;
        }

        let renderer = if render_node == primary {
            gpus.single_renderer(&render_node)
        } else {
            let format = surface.drm_output.format();
            gpus.renderer(&primary, &render_node, format)
        };
        let mut renderer = match renderer {
            Ok(r) => r,
            Err(e) => {
                warn!("no renderer for {}: {e}", output.name());
                return;
            }
        };

        let clear = crate::render::color32(m3.theme.colors.surface_dim);
        let elements = output_elements(m3, &mut renderer, output, true);
        let result = surface.drm_output.render_frame(&mut renderer, &elements, clear, FrameFlags::DEFAULT);
        let (empty, states) = match result {
            Ok(r) => (r.is_empty, r.states),
            Err(e) => {
                warn!("rendering {} failed: {e}", output.name());
                return;
            }
        };
        drop(elements);
        drop(renderer);
        let feedback = m3.post_render(output, &states);
        if !empty {
            match surface.drm_output.queue_frame(Some(feedback)) {
                Ok(()) => {
                    surface.frame_pending = true;
                    return;
                }
                Err(e) => warn!("queueing frame on {} failed: {e}", output.name()),
            }
        }
        // Nothing changed (or queueing failed): wait about one refresh before
        // allowing another frame, so frame callbacks stay paced.
        surface.frame_pending = true;
        let refresh = output.current_mode().map(|m| m.refresh).unwrap_or(60_000).max(1);
        let duration = Duration::from_micros(1_000_000_000 / refresh as u64);
        let token = loop_handle.insert_source(Timer::from_duration(duration), move |_, _, state| {
            if let Some((b, _)) = backend(state) {
                if let Some(s) = b.devices.get_mut(&node).and_then(|d| d.surfaces.get_mut(&crtc)) {
                    s.frame_pending = false;
                    s.estimated_vblank = None;
                }
            }
            state.render_queued();
            TimeoutAction::Drop
        });
        surface.estimated_vblank = token.ok();
    }

    fn on_vblank(&mut self, m3: &mut M3, node: DrmNode, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
        let Some(surface) = self.devices.get_mut(&node).and_then(|d| d.surfaces.get_mut(&crtc)) else { return };
        surface.frame_pending = false;
        let output = surface.output.clone();
        let refresh = output.current_mode().map(|m| m.refresh).unwrap_or(60_000).max(1);
        let frame = Duration::from_micros(1_000_000_000 / refresh as u64);
        let (time, flags) = match metadata.as_ref().map(|m| m.time) {
            Some(DrmEventTime::Monotonic(t)) if !t.is_zero() => (
                t.into(),
                wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwClock
                    | wp_presentation_feedback::Kind::HwCompletion,
            ),
            _ => (m3.clock.now(), wp_presentation_feedback::Kind::Vsync),
        };
        let seq = metadata.as_ref().map(|m| m.sequence).unwrap_or(0);
        match surface.drm_output.frame_submitted() {
            Ok(Some(Some(mut feedback))) => feedback.presented(time, Refresh::fixed(frame), seq as u64, flags),
            Ok(_) => {}
            Err(e) => warn!("frame submission on {} failed: {e}", output.name()),
        }
    }

    // ---- misc ------------------------------------------------------------------

    pub fn update_leds(&mut self, leds: LedState) {
        for d in &mut self.input_devices {
            if d.has_capability(DeviceCapability::Keyboard) {
                d.led_update(leds.into());
            }
        }
    }

    pub fn switch_vt(&mut self, vt: i32) {
        if let Err(e) = self.session.change_vt(vt) {
            warn!("switching to VT {vt} failed: {e}");
        }
    }

    pub fn device_added<D: smithay::backend::input::Device + 'static>(&mut self, m3: &mut M3, device: D) {
        let Some(dev) = (&device as &dyn Any).downcast_ref::<libinput::Device>() else { return };
        let mut dev = dev.clone();
        configure_input_device(&mut dev, &self.input_config);
        if dev.has_capability(DeviceCapability::Keyboard) {
            if let Some(k) = m3.seat.get_keyboard() {
                dev.led_update(k.led_state().into());
            }
        }
        if dev.has_capability(DeviceCapability::Touch) && m3.seat.get_touch().is_none() {
            m3.seat.add_touch();
        }
        info!(name = dev.name(), "input device added");
        self.input_devices.push(dev);
    }

    pub fn apply_input_config(&mut self, input: &m3_core::config::Input) {
        self.input_config = input.clone();
        for d in &mut self.input_devices {
            configure_input_device(d, input);
        }
    }

    pub fn apply_output_config(&mut self, m3: &mut M3) {
        for device in self.devices.values() {
            for s in device.surfaces.values() {
                let Some(cfg) = m3.config.compositor.outputs.get(&s.output.name()) else { continue };
                let scale = cfg.scale.map(Scale::Fractional);
                let pos = cfg.position.map(|[x, y]| (x, y).into());
                s.output.change_current_state(
                    None,
                    Some(crate::backend::to_smithay_transform(cfg.transform)),
                    scale,
                    pos,
                );
                m3.shell.output_changed(&s.output);
                crate::backend::layer_arrange(&s.output);
            }
        }
        m3.queue_redraw_all();
    }

    pub fn configure_output(
        &mut self,
        m3: &mut M3,
        output: &Output,
        change: &m3_protocol::OutputChange,
    ) -> Result<(), String> {
        let m3_protocol::OutputChange::Mode { width, height, refresh_mhz } = change else {
            return crate::backend::generic_configure_output(m3, output, change);
        };
        let &OutputId { node, crtc } = output.user_data().get::<OutputId>().ok_or("unknown output")?;
        let Self { devices, gpus, primary_gpu, .. } = self;
        let device = devices.get_mut(&node).ok_or("unknown GPU")?;
        let render_node = device.render_node.unwrap_or(*primary_gpu);
        let surface = device.surfaces.get_mut(&crtc).ok_or("output is not active")?;
        let mode = pick_mode(surface.connector.modes(), *width, *height, *refresh_mhz)
            .ok_or_else(|| format!("{} does not support {width}x{height}", output.name()))?;
        let mut renderer = gpus.single_renderer(&render_node).map_err(|e| e.to_string())?;
        surface
            .drm_output
            .use_mode::<_, OutputElement<UdevRenderer<'_>>>(mode, &mut renderer, &DrmOutputRenderElements::default())
            .map_err(|e| format!("mode change failed: {e}"))?;
        let wl = WlMode::from(mode);
        output.change_current_state(Some(wl), None, None, None);
        m3.shell.output_changed(output);
        crate::backend::layer_arrange(output);
        m3.queue_redraw_all();
        Ok(())
    }
}

/// Parse the connector's EDID (monitor make, model, serial).
fn edid_info(drm: &DrmDevice, conn: connector::Handle) -> Option<libdisplay_info::info::Info> {
    let props = drm.get_properties(conn).ok()?;
    let (handles, values) = props.as_props_and_values();
    let blob = handles.iter().zip(values).find_map(|(h, v)| {
        let info = drm.get_property(*h).ok()?;
        (info.name().to_str() == Ok("EDID")).then_some(*v)
    })?;
    let data = drm.get_property_blob(blob).ok()?;
    libdisplay_info::info::Info::parse_edid(&data).ok()
}

/// Closest mode with the requested size (and refresh, if given).
fn pick_mode(
    modes: &[smithay::reexports::drm::control::Mode],
    w: i32,
    h: i32,
    refresh_mhz: Option<i32>,
) -> Option<smithay::reexports::drm::control::Mode> {
    modes
        .iter()
        .filter(|m| m.size() == (w as u16, h as u16))
        .min_by_key(|m| {
            let r = WlMode::from(**m).refresh;
            match refresh_mhz {
                Some(want) => (r - want).abs(),
                // Prefer the highest refresh rate when none is requested.
                None => -r,
            }
        })
        .copied()
}

/// Pick a comfortable default scale from pixel density.
fn auto_scale(width_px: i32, width_mm: u32) -> f64 {
    if width_mm == 0 {
        return 1.0;
    }
    let dpi = width_px as f64 / (width_mm as f64 / 25.4);
    match dpi {
        d if d < 144.0 => 1.0,
        d if d < 168.0 => 1.25,
        d if d < 192.0 => 1.5,
        d if d < 240.0 => 1.75,
        _ => 2.0,
    }
}

fn configure_input_device(d: &mut libinput::Device, cfg: &m3_core::config::Input) {
    let touchpad = d.config_tap_finger_count() > 0;
    if touchpad {
        let t = &cfg.touchpad;
        let _ = d.config_tap_set_enabled(t.tap_to_click);
        let _ = d.config_scroll_set_natural_scroll_enabled(t.natural_scroll);
        let _ = d.config_accel_set_speed(t.accel_speed);
        let _ = d.config_dwt_set_enabled(t.disable_while_typing);
        let method = if t.clickfinger { ClickMethod::Clickfinger } else { ClickMethod::ButtonAreas };
        let _ = d.config_click_set_method(method);
    } else if d.has_capability(DeviceCapability::Pointer) {
        let p = &cfg.pointer;
        let _ = d.config_accel_set_speed(p.accel_speed);
        let profile = match p.accel_profile {
            m3_core::config::AccelProfile::Adaptive => LiAccel::Adaptive,
            m3_core::config::AccelProfile::Flat => LiAccel::Flat,
        };
        let _ = d.config_accel_set_profile(profile);
        let _ = d.config_scroll_set_natural_scroll_enabled(p.natural_scroll);
        let _ = d.config_left_handed_set(p.left_handed);
    }
}

#[cfg(test)]
mod tests {
    use super::auto_scale;

    #[test]
    fn auto_scale_buckets() {
        assert_eq!(auto_scale(1920, 0), 1.0);
        assert_eq!(auto_scale(1920, 527), 1.0); // 24" 1080p
        assert_eq!(auto_scale(1920, 309), 1.25); // 14" 1080p laptop (158 dpi)
        assert_eq!(auto_scale(2560, 344), 1.5); // 15.6" 1440p (189 dpi)
        assert_eq!(auto_scale(3840, 344), 2.0); // 15.6" 4K
    }
}
