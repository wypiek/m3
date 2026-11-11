//! Bridge between the compositor IPC socket and the GTK main loop.

use std::time::Duration;

use gtk::{gio, glib};
use m3_protocol::{Action, Client, Event, Request};
use tracing::{debug, info, warn};

pub enum IpcMsg {
    Connected,
    Event(Event),
    Disconnected,
}

/// Subscribe to compositor events on a background thread. Reconnects
/// automatically (the compositor may restart, or start after the shell).
pub fn subscribe() -> async_channel::Receiver<IpcMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("m3-ipc".into())
        .spawn(move || {
            let mut delay = Duration::from_millis(100);
            loop {
                match Client::connect().and_then(|c| c.subscribe()) {
                    Ok(stream) => {
                        info!("connected to the compositor");
                        delay = Duration::from_millis(100);
                        if tx.send_blocking(IpcMsg::Connected).is_err() {
                            return;
                        }
                        for event in stream {
                            match event {
                                Ok(e) => {
                                    if tx.send_blocking(IpcMsg::Event(e)).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    warn!("IPC stream error: {e}");
                                    break;
                                }
                            }
                        }
                        if tx.send_blocking(IpcMsg::Disconnected).is_err() {
                            return;
                        }
                    }
                    Err(e) => debug!("compositor not reachable yet: {e}"),
                }
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(2));
            }
        })
        .expect("spawning the IPC thread");
    rx
}

/// Send a request without blocking the UI. Errors are logged.
pub fn send(req: Request) {
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(move || Client::connect().and_then(|mut c| c.request(&req))).await;
        match r {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => warn!("compositor request failed: {e}"),
            Err(_) => warn!("compositor request panicked"),
        }
    });
}

/// Send requests in order over one connection (e.g. focus a window, then
/// act on it). Errors are logged.
pub fn send_all(reqs: Vec<Request>) {
    glib::spawn_future_local(async move {
        let r = gio::spawn_blocking(move || -> Result<(), String> {
            let mut c = Client::connect().map_err(|e| e.to_string())?;
            for req in &reqs {
                c.request(req).map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .await;
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("compositor request failed: {e}"),
            Err(_) => warn!("compositor request panicked"),
        }
    });
}

pub fn action(a: Action) {
    send(Request::Action { action: a });
}
