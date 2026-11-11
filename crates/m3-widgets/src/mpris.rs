//! Now playing: the most relevant MPRIS player
//! (`org.mpris.MediaPlayer2.*` on the session bus) with play/pause and next.
//! Uses GIO's D-Bus client; only MPRIS methods are ever called.

use std::collections::HashMap;
use std::path::PathBuf;

use gtk::{gio, glib, prelude::*};

const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub bus_name: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Local cover art (`mpris:artUrl` with a `file://` URL).
    pub art: Option<PathBuf>,
}

/// Index of the player to show: a playing one, else a paused one. Stopped
/// players are not shown (pure; tested).
pub fn pick(statuses: &[&str]) -> Option<usize> {
    statuses.iter().position(|s| *s == "Playing").or_else(|| statuses.iter().position(|s| *s == "Paused"))
}

/// `(title, artist, art)` from MPRIS metadata (pure; tested).
pub fn parse_metadata(meta: &HashMap<String, glib::Variant>) -> (String, String, Option<PathBuf>) {
    let title = meta.get("xesam:title").and_then(|v| v.get::<String>()).unwrap_or_default();
    let artist = meta
        .get("xesam:artist")
        .and_then(|v| v.get::<Vec<String>>().map(|a| a.join(", ")).or_else(|| v.get::<String>()))
        .unwrap_or_default();
    let art = meta
        .get("mpris:artUrl")
        .and_then(|v| v.get::<String>())
        .filter(|u| u.starts_with("file://"))
        .and_then(|u| gio::File::for_uri(&u).path())
        .filter(|p| p.is_file());
    (title, artist, art)
}

async fn call(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    params: Option<&glib::Variant>,
) -> Option<glib::Variant> {
    conn.call_future(Some(dest), path, iface, method, params, None, gio::DBusCallFlags::NONE, 1500).await.ok()
}

/// What the most relevant player is doing, if anything is playing/paused.
pub async fn query(conn: &gio::DBusConnection) -> Option<NowPlaying> {
    let names = call(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", None)
        .await?
        .child_value(0)
        .get::<Vec<String>>()?;
    let mut players = Vec::new();
    for name in names.into_iter().filter(|n| n.starts_with("org.mpris.MediaPlayer2.")) {
        let params = (PLAYER,).to_variant();
        let Some(reply) = call(conn, &name, PATH, "org.freedesktop.DBus.Properties", "GetAll", Some(&params)).await
        else {
            continue;
        };
        let Some(props) = reply.child_value(0).get::<HashMap<String, glib::Variant>>() else { continue };
        let status = props.get("PlaybackStatus").and_then(|v| v.get::<String>()).unwrap_or_default();
        let meta = props.get("Metadata").and_then(|v| v.get::<HashMap<String, glib::Variant>>()).unwrap_or_default();
        players.push((name, status, meta));
    }
    let statuses: Vec<&str> = players.iter().map(|p| p.1.as_str()).collect();
    let (name, status, meta) = players.get(pick(&statuses)?)?;
    let (title, artist, art) = parse_metadata(meta);
    Some(NowPlaying { bus_name: name.clone(), title, artist, playing: status == "Playing", art })
}

/// `PlayPause`, `Next` or `Previous` on `player`.
pub async fn control(conn: &gio::DBusConnection, player: &str, method: &'static str) {
    if call(conn, player, PATH, PLAYER, method, None).await.is_none() {
        tracing::warn!("media control {method} on {player} failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_playing_then_paused() {
        assert_eq!(pick(&["Stopped", "Paused", "Playing"]), Some(2));
        assert_eq!(pick(&["Stopped", "Paused"]), Some(1));
        assert_eq!(pick(&["Stopped"]), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn dictionary_values_unbox() {
        let dict = glib::VariantDict::new(None);
        dict.insert_value("PlaybackStatus", &"Playing".to_variant());
        let v = dict.end();
        let m = v.get::<HashMap<String, glib::Variant>>().expect("a{sv}");
        let s = m.get("PlaybackStatus").expect("key");
        // GLib hands dictionary values out already unboxed.
        assert_eq!(s.get::<String>().as_deref(), Some("Playing"), "type {}", s.type_());
    }

    #[test]
    fn metadata() {
        let mut m = HashMap::new();
        m.insert("xesam:title".to_owned(), "Glass Harbor".to_variant());
        m.insert("xesam:artist".to_owned(), vec!["Aurora Lines".to_owned(), "Guest".to_owned()].to_variant());
        m.insert("mpris:artUrl".to_owned(), "https://example.com/a.png".to_variant());
        let (t, a, art) = parse_metadata(&m);
        assert_eq!((t.as_str(), a.as_str(), art), ("Glass Harbor", "Aurora Lines, Guest", None));
        assert_eq!(parse_metadata(&HashMap::new()), (String::new(), String::new(), None));
    }
}
