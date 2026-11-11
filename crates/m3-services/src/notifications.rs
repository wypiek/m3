//! The freedesktop notification server (Desktop Notifications Specification
//! 1.2) plus m3's own control interface `org.m3.Notifications`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use tracing::{info, warn};
use zbus::{OwnedVariant as OwnedValue, interface, object_server::SignalEmitter};

use crate::Event;

pub const PATH: &str = "/org/freedesktop/Notifications";
pub const M3_PATH: &str = "/org/m3/Notifications";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
    Undefined = 4,
}

/// Raw image data from the `image-data` hint (RGB/RGBA rows).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageData {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Notification {
    pub id: u32,
    /// Set when this notification replaces an earlier one with the same id.
    pub replaces: bool,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    /// `(key, label)` pairs.
    pub actions: Vec<(String, String)>,
    pub urgency: Urgency,
    pub desktop_entry: Option<String>,
    pub category: Option<String>,
    pub image_path: Option<String>,
    #[serde(skip)]
    pub image: Option<ImageData>,
    pub transient: bool,
    pub resident: bool,
    /// Milliseconds; -1 = server default, 0 = never expire.
    pub expire_timeout: i32,
    /// Unix seconds.
    pub timestamp: i64,
}

struct Server {
    next_id: Arc<AtomicU32>,
    events: async_channel::Sender<Event>,
}

fn hint_str(h: &HashMap<String, OwnedValue>, k: &str) -> Option<String> {
    h.get(k).and_then(|v| String::try_from(v.clone()).ok()).filter(|s| !s.is_empty())
}

fn hint_bool(h: &HashMap<String, OwnedValue>, k: &str) -> bool {
    h.get(k).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(false)
}

fn hint_image(h: &HashMap<String, OwnedValue>) -> Option<ImageData> {
    let v = h.get("image-data").or_else(|| h.get("image_data")).or_else(|| h.get("icon_data"))?;
    let (width, height, rowstride, has_alpha, _bits, _channels, data): (i32, i32, i32, bool, i32, i32, Vec<u8>) =
        v.clone().try_into().ok()?;
    // Reject inconsistent sizes rather than trusting the sender.
    if width <= 0 || height <= 0 || width > 4096 || height > 4096 || rowstride < width * if has_alpha { 4 } else { 3 } {
        return None;
    }
    if data.len() < (rowstride as usize) * (height as usize - 1) + (width as usize) * if has_alpha { 4 } else { 3 } {
        return None;
    }
    Some(ImageData { width, height, rowstride, has_alpha, data })
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["actions", "body", "icon-static", "persistence"]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 { replaces_id } else { self.next_id.fetch_add(1, Ordering::Relaxed) };
        let urgency = match hints.get("urgency").and_then(|v| u8::try_from(v.clone()).ok()) {
            Some(0) => Urgency::Low,
            Some(2) => Urgency::Critical,
            _ => Urgency::Normal,
        };
        let actions = actions.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect();
        let n = Notification {
            id,
            replaces: replaces_id != 0,
            app_name: truncate(app_name, 128),
            app_icon,
            summary: truncate(summary, 512),
            body: truncate(body, 4096),
            actions,
            urgency,
            desktop_entry: hint_str(&hints, "desktop-entry"),
            category: hint_str(&hints, "category"),
            image_path: hint_str(&hints, "image-path").or_else(|| hint_str(&hints, "image_path")),
            image: hint_image(&hints),
            transient: hint_bool(&hints, "transient"),
            resident: hint_bool(&hints, "resident"),
            expire_timeout,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        };
        if self.events.try_send(Event::Notification(n)).is_err() {
            warn!("notification UI is not listening");
        }
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.events.try_send(Event::NotificationClosed { id });
    }

    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("m3", "m3 Desktop", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    async fn notification_closed(emitter: &SignalEmitter<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, action_key: &str) -> zbus::Result<()>;
}

/// `org.m3.Notifications`: control for m3ctl / the settings app.
struct Control {
    dnd: Arc<AtomicBool>,
    events: async_channel::Sender<Event>,
}

#[interface(name = "org.m3.Notifications")]
impl Control {
    fn clear_all(&self) {
        let _ = self.events.try_send(Event::ClearAllNotifications);
    }

    #[zbus(property)]
    fn do_not_disturb(&self) -> bool {
        self.dnd.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn set_do_not_disturb(&mut self, on: bool) {
        self.dnd.store(on, Ordering::Relaxed);
        let _ = self.events.try_send(Event::DoNotDisturb(on));
    }
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    s
}

/// Emits signals for notifications the UI closed or acted on.
pub struct ServerHandle {
    conn: zbus::Connection,
    dnd: Arc<AtomicBool>,
}

impl ServerHandle {
    pub async fn closed(&self, id: u32, reason: CloseReason) -> Result<(), String> {
        let iface = self.conn.object_server().interface::<_, Server>(PATH).await.map_err(|e| e.to_string())?;
        Server::notification_closed(iface.signal_emitter(), id, reason as u32).await.map_err(|e| e.to_string())
    }

    pub async fn action_invoked(&self, id: u32, key: &str) -> Result<(), String> {
        let iface = self.conn.object_server().interface::<_, Server>(PATH).await.map_err(|e| e.to_string())?;
        Server::action_invoked(iface.signal_emitter(), id, key).await.map_err(|e| e.to_string())
    }

    pub async fn set_dnd(&self, on: bool) -> Result<(), String> {
        self.dnd.store(on, Ordering::Relaxed);
        let iface = self.conn.object_server().interface::<_, Control>(M3_PATH).await.map_err(|e| e.to_string())?;
        iface.get().await.do_not_disturb_changed(iface.signal_emitter()).await.map_err(|e| e.to_string())
    }
}

/// Export the servers and claim `org.freedesktop.Notifications`. Returns
/// `None` if another notification daemon owns the name.
pub async fn serve(conn: &zbus::Connection, events: async_channel::Sender<Event>) -> Option<ServerHandle> {
    let dnd = Arc::new(AtomicBool::new(false));
    let server = Server { next_id: Arc::new(AtomicU32::new(1)), events: events.clone() };
    let control = Control { dnd: dnd.clone(), events };
    if let Err(e) = conn.object_server().at(PATH, server).await {
        warn!("cannot export the notification server: {e}");
        return None;
    }
    if let Err(e) = conn.object_server().at(M3_PATH, control).await {
        warn!("cannot export org.m3.Notifications: {e}");
    }
    if let Err(e) = conn.request_name("org.m3.Notifications").await {
        warn!("cannot own org.m3.Notifications: {e}");
    }
    match conn.request_name("org.freedesktop.Notifications").await {
        Ok(_) => info!("serving org.freedesktop.Notifications"),
        Err(e) => {
            warn!("another notification server is running ({e}); m3 notifications are disabled");
            return None;
        }
    }
    Some(ServerHandle { conn: conn.clone(), dnd })
}
