//! Audio through the PulseAudio client API.
//!
//! On PipeWire systems this talks to `pipewire-pulse` (WirePlumber manages
//! the graph); on PulseAudio systems it talks to PulseAudio directly. The
//! PulseAudio API is the most widely supported, stable way for a desktop to
//! control volumes, devices and per-application streams.
//!
//! A threaded libpulse mainloop runs on its own thread; changes are pushed
//! through `subscribe` (no polling) and every change publishes a full
//! [`AudioState`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::subscribe::InterestMaskSet;
use pulse::context::{Context, FlagSet, State as CtxState};
use pulse::mainloop::threaded::Mainloop;
use pulse::proplist::Proplist;
use pulse::volume::{ChannelVolumes, Volume};
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub index: u32,
    pub name: String,
    pub description: String,
    /// 0.0 … 1.5 (100% = 1.0)
    pub volume: f64,
    pub muted: bool,
    pub channels: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub index: u32,
    pub app_name: String,
    pub icon: Option<String>,
    pub volume: f64,
    pub muted: bool,
    pub channels: u8,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioState {
    pub available: bool,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    pub streams: Vec<Stream>,
}

impl AudioState {
    pub fn output(&self) -> Option<&Device> {
        self.sinks.iter().find(|d| Some(&d.name) == self.default_sink.as_ref()).or(self.sinks.first())
    }
    pub fn input(&self) -> Option<&Device> {
        self.sources.iter().find(|d| Some(&d.name) == self.default_source.as_ref()).or(self.sources.first())
    }
}

#[derive(Debug, Clone)]
pub enum AudioCmd {
    SetOutputVolume(f64),
    StepOutputVolume(f64),
    ToggleOutputMute,
    SetOutputMute(bool),
    SetInputVolume(f64),
    ToggleInputMute,
    SetInputMute(bool),
    SetDefaultOutput(String),
    SetDefaultInput(String),
    SetStreamVolume { index: u32, volume: f64 },
    SetStreamMute { index: u32, muted: bool },
}

#[derive(Clone)]
pub struct Audio {
    tx: mpsc::Sender<AudioCmd>,
}

impl Audio {
    pub fn start() -> (Self, async_channel::Receiver<AudioState>) {
        let (tx, rx) = mpsc::channel();
        let (st_tx, st_rx) = async_channel::unbounded();
        let spawned = std::thread::Builder::new().name("m3-audio".into()).spawn(move || {
            if let Err(e) = run(rx, st_tx.clone()) {
                warn!("audio integration unavailable: {e}");
                let _ = st_tx.try_send(AudioState::default());
            }
        });
        if let Err(e) = spawned {
            warn!("cannot start the audio thread: {e}");
        }
        (Self { tx }, st_rx)
    }

    pub fn send(&self, cmd: AudioCmd) {
        let _ = self.tx.send(cmd);
    }
}

fn avg(cv: &ChannelVolumes) -> f64 {
    cv.avg().0 as f64 / Volume::NORMAL.0 as f64
}

fn volume_for(channels: u8, v: f64) -> ChannelVolumes {
    let mut cv = ChannelVolumes::default();
    let raw = (v.clamp(0.0, 1.5) * Volume::NORMAL.0 as f64).round() as u32;
    cv.set(channels.max(1), Volume(raw));
    cv
}

#[derive(Default)]
struct Partial {
    pending: u8,
    state: AudioState,
}

/// Re-read everything and publish once all four queries completed.
fn refresh(ctx: &Context, out: &async_channel::Sender<AudioState>, latest: &Rc<RefCell<AudioState>>) {
    let partial =
        Rc::new(RefCell::new(Partial { pending: 4, state: AudioState { available: true, ..Default::default() } }));
    let finish = {
        let out = out.clone();
        let latest = latest.clone();
        move |p: &Rc<RefCell<Partial>>| {
            let mut p = p.borrow_mut();
            p.pending -= 1;
            if p.pending == 0 {
                *latest.borrow_mut() = p.state.clone();
                let _ = out.try_send(p.state.clone());
            }
        }
    };
    let intro = ctx.introspect();
    {
        let p = partial.clone();
        let f = finish.clone();
        intro.get_server_info(move |info| {
            p.borrow_mut().state.default_sink = info.default_sink_name.as_ref().map(|s| s.to_string());
            p.borrow_mut().state.default_source = info.default_source_name.as_ref().map(|s| s.to_string());
            f(&p);
        });
    }
    {
        let p = partial.clone();
        let f = finish.clone();
        intro.get_sink_info_list(move |r| match r {
            ListResult::Item(s) => p.borrow_mut().state.sinks.push(Device {
                index: s.index,
                name: s.name.as_deref().unwrap_or_default().to_owned(),
                description: s.description.as_deref().unwrap_or_default().to_owned(),
                volume: avg(&s.volume),
                muted: s.mute,
                channels: s.volume.len(),
            }),
            ListResult::End | ListResult::Error => f(&p),
        });
    }
    {
        let p = partial.clone();
        let f = finish.clone();
        intro.get_source_info_list(move |r| match r {
            // Skip monitor sources: they are not microphones.
            ListResult::Item(s) if s.monitor_of_sink.is_none() => p.borrow_mut().state.sources.push(Device {
                index: s.index,
                name: s.name.as_deref().unwrap_or_default().to_owned(),
                description: s.description.as_deref().unwrap_or_default().to_owned(),
                volume: avg(&s.volume),
                muted: s.mute,
                channels: s.volume.len(),
            }),
            ListResult::Item(_) => {}
            ListResult::End | ListResult::Error => f(&p),
        });
    }
    {
        let p = partial.clone();
        let f = finish;
        intro.get_sink_input_info_list(move |r| match r {
            ListResult::Item(s) => {
                let app = s
                    .proplist
                    .get_str(pulse::proplist::properties::APPLICATION_NAME)
                    .or_else(|| s.name.as_ref().map(|n| n.to_string()))
                    .unwrap_or_else(|| "Application".into());
                p.borrow_mut().state.streams.push(Stream {
                    index: s.index,
                    app_name: app,
                    icon: s.proplist.get_str(pulse::proplist::properties::APPLICATION_ICON_NAME),
                    volume: avg(&s.volume),
                    muted: s.mute,
                    channels: s.volume.len(),
                });
            }
            ListResult::End | ListResult::Error => f(&p),
        });
    }
}

fn run(rx: mpsc::Receiver<AudioCmd>, out: async_channel::Sender<AudioState>) -> Result<(), String> {
    let mut props = Proplist::new().ok_or("proplist")?;
    let _ = props.set_str(pulse::proplist::properties::APPLICATION_NAME, "m3");
    let _ = props.set_str(pulse::proplist::properties::APPLICATION_ID, "org.m3.Shell");
    let ml = Rc::new(RefCell::new(Mainloop::new().ok_or("cannot create the audio mainloop")?));
    let ctx = Rc::new(RefCell::new(
        Context::new_with_proplist(&*ml.borrow(), "m3", &props).ok_or("cannot create the audio context")?,
    ));

    ctx.borrow_mut()
        .connect(None, FlagSet::NOFAIL, None)
        .map_err(|e| format!("connecting to the sound server: {e}"))?;
    ml.borrow_mut().start().map_err(|e| format!("starting the audio mainloop: {e}"))?;
    // Wait (bounded) for the connection. Only done once at startup; after
    // that everything is event driven.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        ml.borrow_mut().lock();
        let state = ctx.borrow().get_state();
        ml.borrow_mut().unlock();
        match state {
            CtxState::Ready => break,
            CtxState::Failed | CtxState::Terminated => {
                return Err("no sound server (is PipeWire or PulseAudio running?)".into());
            }
            _ if std::time::Instant::now() > deadline => return Err("the sound server did not answer".into()),
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    info!("connected to the sound server");
    ml.borrow_mut().lock();

    let latest = Rc::new(RefCell::new(AudioState::default()));
    {
        let ctx2 = ctx.clone();
        let out2 = out.clone();
        let latest2 = latest.clone();
        ctx.borrow_mut().set_subscribe_callback(Some(Box::new(move |_, _, _| {
            if let Ok(c) = ctx2.try_borrow() {
                refresh(&c, &out2, &latest2);
            }
        })));
    }
    ctx.borrow_mut().subscribe(
        InterestMaskSet::SINK | InterestMaskSet::SOURCE | InterestMaskSet::SINK_INPUT | InterestMaskSet::SERVER,
        |_| {},
    );
    refresh(&ctx.borrow(), &out, &latest);
    ml.borrow_mut().unlock();

    while let Ok(cmd) = rx.recv() {
        ml.borrow_mut().lock();
        {
            let c = ctx.borrow();
            let mut intro = c.introspect();
            let st = latest.borrow().clone();
            match cmd {
                AudioCmd::SetOutputVolume(v) | AudioCmd::StepOutputVolume(v) => {
                    if let Some(d) = st.output() {
                        let target = if matches!(cmd, AudioCmd::StepOutputVolume(_)) { d.volume + v } else { v };
                        intro.set_sink_volume_by_name(&d.name, &volume_for(d.channels, target.clamp(0.0, 1.0)), None);
                        if d.muted && target > d.volume {
                            intro.set_sink_mute_by_name(&d.name, false, None);
                        }
                    }
                }
                AudioCmd::ToggleOutputMute => {
                    if let Some(d) = st.output() {
                        intro.set_sink_mute_by_name(&d.name, !d.muted, None);
                    }
                }
                AudioCmd::SetOutputMute(m) => {
                    if let Some(d) = st.output() {
                        intro.set_sink_mute_by_name(&d.name, m, None);
                    }
                }
                AudioCmd::SetInputVolume(v) => {
                    if let Some(d) = st.input() {
                        intro.set_source_volume_by_name(&d.name, &volume_for(d.channels, v.clamp(0.0, 1.0)), None);
                    }
                }
                AudioCmd::ToggleInputMute => {
                    if let Some(d) = st.input() {
                        intro.set_source_mute_by_name(&d.name, !d.muted, None);
                    }
                }
                AudioCmd::SetInputMute(m) => {
                    if let Some(d) = st.input() {
                        intro.set_source_mute_by_name(&d.name, m, None);
                    }
                }
                AudioCmd::SetDefaultOutput(name) => {
                    drop(intro);
                    drop(c);
                    ctx.borrow_mut().set_default_sink(&name, |_| {});
                    ml.borrow_mut().unlock();
                    continue;
                }
                AudioCmd::SetDefaultInput(name) => {
                    drop(intro);
                    drop(c);
                    ctx.borrow_mut().set_default_source(&name, |_| {});
                    ml.borrow_mut().unlock();
                    continue;
                }
                AudioCmd::SetStreamVolume { index, volume } => {
                    let ch = st.streams.iter().find(|s| s.index == index).map(|s| s.channels).unwrap_or(2);
                    intro.set_sink_input_volume(index, &volume_for(ch, volume), None);
                }
                AudioCmd::SetStreamMute { index, muted } => {
                    intro.set_sink_input_mute(index, muted, None);
                }
            }
        }
        ml.borrow_mut().unlock();
    }
    Ok(())
}
