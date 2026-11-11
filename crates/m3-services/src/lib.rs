//! System integrations for m3.
//!
//! m3 integrates with existing Linux services instead of replacing them:
//!
//! | Area | Service | Module |
//! |---|---|---|
//! | Notifications | serves `org.freedesktop.Notifications` 1.2 | [`notifications`] |
//! | Networking | NetworkManager (system bus) | [`network`] |
//! | Bluetooth | BlueZ (system bus) | [`bluetooth`] |
//! | Battery | UPower (system bus) | [`power`] |
//! | Power profiles | power-profiles-daemon (system bus) | [`power`] |
//! | Brightness | sysfs backlight + logind `SetBrightness` | [`power`] |
//! | Session power | logind (system bus) | [`power`] |
//! | Media keys | MPRIS (session bus) | [`media`] |
//! | Audio | PulseAudio API (PipeWire via pipewire-pulse) | [`audio`] |
//! | System tray | serves `org.kde.StatusNotifierWatcher`, reads `com.canonical.dbusmenu` | [`tray`] |
//!
//! D-Bus work happens on one background thread running an async loop; the UI
//! sends [`Command`]s and receives [`Event`]s over channels, so GTK code never
//! blocks on the bus. A missing service is reported as unavailable rather
//! than faked.

pub mod audio;
pub mod bluetooth;
pub mod media;
pub mod network;
pub mod notifications;
pub mod power;
pub mod tray;
pub mod weather;

use futures_util::{StreamExt, stream::BoxStream};
use tracing::{info, warn};

pub use bluetooth::BluetoothState;
pub use network::NetworkState;
pub use notifications::{CloseReason, Notification};
pub use power::{BatteryState, PowerProfiles};

/// Something changed in the system.
#[derive(Debug, Clone)]
pub enum Event {
    Notification(Notification),
    NotificationClosed {
        id: u32,
    },
    /// `org.m3.Notifications.ClearAll` was called (e.g. by `m3ctl`).
    ClearAllNotifications,
    DoNotDisturb(bool),
    Network(NetworkState),
    Bluetooth(BluetoothState),
    Battery(BatteryState),
    PowerProfiles(PowerProfiles),
    Brightness(Option<f64>),
    /// The system is about to suspend: lock now, then send
    /// [`Command::ReadyForSleep`].
    PrepareForSleep,
    /// A command failed; the message is suitable for a snackbar.
    Error(String),
}

/// Requests from the UI.
#[derive(Debug, Clone)]
pub enum Command {
    CloseNotification {
        id: u32,
        reason: CloseReason,
    },
    InvokeAction {
        id: u32,
        key: String,
    },
    SetDoNotDisturb(bool),
    SetWifiEnabled(bool),
    ScanWifi,
    ConnectWifi {
        ssid: String,
        password: Option<String>,
    },
    DisconnectWifi,
    SetBluetoothPowered(bool),
    SetBluetoothDiscovery(bool),
    ConnectBluetooth {
        path: String,
    },
    DisconnectBluetooth {
        path: String,
    },
    PairBluetooth {
        path: String,
    },
    ForgetBluetooth {
        path: String,
    },
    SetPowerProfile(String),
    /// 0.0 … 1.0
    SetBrightness(f64),
    StepBrightness(f64),
    MediaPlayPause,
    MediaNext,
    MediaPrevious,
    PowerOff,
    Reboot,
    Suspend,
    /// The screen is locked; let the pending suspend proceed.
    ReadyForSleep,
    Refresh,
}

/// Handle to the service thread.
#[derive(Clone)]
pub struct Services {
    tx: async_channel::Sender<Command>,
}

impl Services {
    /// Start the service thread. Returns the handle and the event stream.
    pub fn start() -> (Self, async_channel::Receiver<Event>) {
        let (cmd_tx, cmd_rx) = async_channel::unbounded::<Command>();
        let (ev_tx, ev_rx) = async_channel::unbounded::<Event>();
        let spawned = std::thread::Builder::new().name("m3-dbus".into()).spawn(move || {
            zbus::block_on(run(cmd_rx, ev_tx));
        });
        if let Err(e) = spawned {
            warn!("cannot start the D-Bus thread: {e}");
        }
        (Self { tx: cmd_tx }, ev_rx)
    }

    pub fn send(&self, cmd: Command) {
        if self.tx.try_send(cmd).is_err() {
            warn!("service thread is gone; command dropped");
        }
    }
}

/// Internal wake-ups for the service loop.
enum Wake {
    Command(Command),
    Network,
    Bluetooth,
    Battery,
    Profiles,
    Sleep(bool),
}

async fn run(cmd_rx: async_channel::Receiver<Command>, ev: async_channel::Sender<Event>) {
    let session = match zbus::Connection::session().await {
        Ok(c) => Some(c),
        Err(e) => {
            warn!("no session bus ({e}); notifications and media keys are unavailable");
            None
        }
    };
    let system = match zbus::Connection::system().await {
        Ok(c) => Some(c),
        Err(e) => {
            warn!("no system bus ({e}); network, bluetooth and power integration are unavailable");
            None
        }
    };

    let notif = match &session {
        Some(s) => notifications::serve(s, ev.clone()).await,
        None => None,
    };

    let mut streams: Vec<BoxStream<'static, Wake>> = vec![cmd_rx.map(Wake::Command).boxed()];
    if let Some(sys) = &system {
        streams.extend(network::watch(sys).await.into_iter().map(|s| s.map(|_| Wake::Network).boxed()));
        streams.extend(bluetooth::watch(sys).await.into_iter().map(|s| s.map(|_| Wake::Bluetooth).boxed()));
        streams.extend(power::watch_battery(sys).await.into_iter().map(|s| s.map(|_| Wake::Battery).boxed()));
        streams.extend(power::watch_profiles(sys).await.into_iter().map(|s| s.map(|_| Wake::Profiles).boxed()));
        if let Err(e) = bluetooth::register_agent(sys).await {
            info!("bluetooth pairing agent unavailable: {e}");
        }
        if let Some(s) = power::watch_sleep(sys).await {
            streams.push(s.map(Wake::Sleep).boxed());
        }
    }
    // Holding a "delay" inhibitor gives us time to lock before suspend.
    let mut sleep_inhibitor = match &system {
        Some(sys) => power::sleep_inhibitor(sys).await,
        None => None,
    };
    let mut all = futures_util::stream::select_all(streams);

    // Initial state.
    publish_all(&system, &ev).await;

    while let Some(w) = all.next().await {
        let Some(sys) = &system else {
            if let Wake::Command(c) = w {
                handle_session_only(c, &session, &notif, &ev).await;
            }
            continue;
        };
        match w {
            Wake::Network => publish(&ev, network::state(sys).await.map(Event::Network)),
            Wake::Bluetooth => publish(&ev, Some(Event::Bluetooth(bluetooth::state(sys).await.unwrap_or_default()))),
            Wake::Battery => publish(&ev, power::battery(sys).await.map(Event::Battery)),
            Wake::Profiles => publish(&ev, power::profiles(sys).await.map(Event::PowerProfiles)),
            Wake::Sleep(true) => {
                if sleep_inhibitor.is_some() {
                    publish(&ev, Some(Event::PrepareForSleep));
                }
            }
            Wake::Sleep(false) => {
                // Resumed: take a fresh inhibitor for the next suspend.
                if sleep_inhibitor.is_none() {
                    sleep_inhibitor = power::sleep_inhibitor(sys).await;
                }
                publish(&ev, Some(Event::Brightness(power::brightness())));
            }
            Wake::Command(Command::ReadyForSleep) => {
                // Dropping the fd releases the delay lock.
                sleep_inhibitor = None;
            }
            Wake::Command(c) => {
                let r: Result<(), String> = match c {
                    Command::SetWifiEnabled(on) => network::set_wifi_enabled(sys, on).await,
                    Command::ScanWifi => network::scan(sys).await,
                    Command::ConnectWifi { ssid, password } => network::connect(sys, &ssid, password.as_deref()).await,
                    Command::DisconnectWifi => network::disconnect_wifi(sys).await,
                    Command::SetBluetoothPowered(on) => bluetooth::set_powered(sys, on).await,
                    Command::SetBluetoothDiscovery(on) => bluetooth::set_discovery(sys, on).await,
                    Command::ConnectBluetooth { path } => bluetooth::device_call(sys, &path, "Connect").await,
                    Command::DisconnectBluetooth { path } => bluetooth::device_call(sys, &path, "Disconnect").await,
                    Command::PairBluetooth { path } => bluetooth::pair(sys, &path).await,
                    Command::ForgetBluetooth { path } => bluetooth::forget(sys, &path).await,
                    Command::SetPowerProfile(p) => power::set_profile(sys, &p).await,
                    Command::SetBrightness(v) => {
                        power::set_brightness(sys, v).await.map(|v| publish(&ev, Some(Event::Brightness(Some(v)))))
                    }
                    Command::StepBrightness(d) => {
                        power::step_brightness(sys, d).await.map(|v| publish(&ev, Some(Event::Brightness(Some(v)))))
                    }
                    Command::PowerOff => power::logind_call(sys, "PowerOff").await,
                    Command::Reboot => power::logind_call(sys, "Reboot").await,
                    Command::Suspend => power::logind_call(sys, "Suspend").await,
                    Command::Refresh => {
                        publish_all(&system, &ev).await;
                        Ok(())
                    }
                    other => {
                        handle_session_only(other, &session, &notif, &ev).await;
                        Ok(())
                    }
                };
                if let Err(e) = r {
                    warn!("{e}");
                    publish(&ev, Some(Event::Error(e)));
                }
            }
        }
    }
}

async fn handle_session_only(
    c: Command,
    session: &Option<zbus::Connection>,
    notif: &Option<notifications::ServerHandle>,
    ev: &async_channel::Sender<Event>,
) {
    let r = match c {
        Command::CloseNotification { id, reason } => match notif {
            Some(n) => n.closed(id, reason).await,
            None => Ok(()),
        },
        Command::InvokeAction { id, key } => match notif {
            Some(n) => n.action_invoked(id, &key).await,
            None => Ok(()),
        },
        Command::SetDoNotDisturb(on) => {
            publish(ev, Some(Event::DoNotDisturb(on)));
            match notif {
                Some(n) => n.set_dnd(on).await,
                None => Ok(()),
            }
        }
        Command::MediaPlayPause | Command::MediaNext | Command::MediaPrevious => match session {
            Some(s) => media::control(s, &c).await,
            None => Err("no session bus for media control".into()),
        },
        _ => Err("this action needs the system D-Bus, which is unavailable".into()),
    };
    if let Err(e) = r {
        warn!("{e}");
        publish(ev, Some(Event::Error(e)));
    }
}

async fn publish_all(system: &Option<zbus::Connection>, ev: &async_channel::Sender<Event>) {
    if let Some(sys) = system {
        publish(ev, network::state(sys).await.map(Event::Network));
        // A missing daemon is reported as "unavailable", never silently skipped.
        publish(ev, Some(Event::Bluetooth(bluetooth::state(sys).await.unwrap_or_default())));
        publish(ev, power::battery(sys).await.map(Event::Battery));
        publish(ev, power::profiles(sys).await.map(Event::PowerProfiles));
    }
    publish(ev, Some(Event::Brightness(power::brightness())));
}

fn publish(ev: &async_channel::Sender<Event>, e: Option<Event>) {
    if let Some(e) = e {
        let _ = ev.try_send(e);
    }
}
