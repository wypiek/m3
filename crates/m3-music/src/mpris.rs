//! MPRIS 2 (`org.mpris.MediaPlayer2.m3music`): lets the m3 shell's media
//! keys and any MPRIS client control playback.
//!
//! The D-Bus server runs on its own thread. Method calls are forwarded to
//! the GTK thread as [`Command`]s; the GTK thread pushes [`Update`]s back,
//! which update the exported state and emit `PropertiesChanged` / `Seeked`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::object_server::SignalEmitter;
use zbus::{ObjectPath, OwnedVariant, Variant, interface};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.m3music";
const PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// Requests from MPRIS clients.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative seek in microseconds.
    Seek(i64),
    /// Absolute position (µs) for the given track id.
    SetPosition(String, i64),
    SetVolume(f64),
    SetShuffle(bool),
    SetLoop(String),
    OpenUri(String),
    Raise,
    Quit,
}

/// Metadata of the current track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub id: usize,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub length_us: i64,
    pub track_no: Option<u32>,
    pub art_url: Option<String>,
    pub url: String,
}

/// Object path for a track id.
pub fn track_path(id: usize) -> String {
    format!("/org/m3/Music/Track/{id}")
}

/// The MPRIS metadata map (`a{sv}`) for `meta`.
pub fn metadata(meta: Option<&Meta>) -> HashMap<String, OwnedVariant> {
    let mut m = HashMap::new();
    let mut put = |k: &str, v: Variant<'_>| {
        if let Ok(v) = OwnedVariant::try_from(v) {
            m.insert(k.to_owned(), v);
        }
    };
    let Some(meta) = meta else {
        if let Ok(p) = ObjectPath::try_from(NO_TRACK) {
            put("mpris:trackid", Variant::from(p));
        }
        return m;
    };
    if let Ok(p) = ObjectPath::try_from(track_path(meta.id)) {
        put("mpris:trackid", Variant::from(p));
    }
    put("xesam:title", Variant::from(meta.title.as_str()));
    if !meta.artist.is_empty() {
        put("xesam:artist", Variant::from(vec![meta.artist.as_str()]));
    }
    if !meta.album.is_empty() {
        put("xesam:album", Variant::from(meta.album.as_str()));
    }
    if !meta.album_artist.is_empty() {
        put("xesam:albumArtist", Variant::from(vec![meta.album_artist.as_str()]));
    }
    if meta.length_us > 0 {
        put("mpris:length", Variant::from(meta.length_us));
    }
    if let Some(n) = meta.track_no {
        put("xesam:trackNumber", Variant::from(n as i32));
    }
    if let Some(a) = &meta.art_url {
        put("mpris:artUrl", Variant::from(a.as_str()));
    }
    put("xesam:url", Variant::from(meta.url.as_str()));
    m
}

/// State pushed from the player.
#[derive(Debug, Clone)]
pub enum Update {
    /// Playing / Paused / Stopped.
    Status(&'static str),
    Track(Option<Meta>),
    /// Current position in µs (not signalled, read on demand).
    Position(i64),
    /// The position jumped (seek): emits `Seeked`.
    Seeked(i64),
    Volume(f64),
    Shuffle(bool),
    Loop(&'static str),
    Can {
        next: bool,
        previous: bool,
        play: bool,
    },
}

#[derive(Debug, Clone)]
struct State {
    status: &'static str,
    meta: Option<Meta>,
    position: i64,
    volume: f64,
    shuffle: bool,
    loop_status: &'static str,
    can_next: bool,
    can_previous: bool,
    can_play: bool,
}

type Shared = Arc<Mutex<State>>;

fn with<T>(s: &Shared, f: impl FnOnce(&State) -> T) -> T {
    match s.lock() {
        Ok(g) => f(&g),
        Err(p) => f(&p.into_inner()),
    }
}

fn send(tx: &async_channel::Sender<Command>, c: Command) {
    let _ = tx.try_send(c);
}

struct Root {
    tx: async_channel::Sender<Command>,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        send(&self.tx, Command::Raise);
    }
    fn quit(&self) {
        send(&self.tx, Command::Quit);
    }
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        "Music".into()
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "org.m3.Music".into()
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        vec!["file".into()]
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        ["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-vorbis+ogg", "audio/opus", "audio/mp4", "audio/x-wav"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }
}

struct PlayerIface {
    tx: async_channel::Sender<Command>,
    state: Shared,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerIface {
    fn next(&self) {
        send(&self.tx, Command::Next);
    }
    fn previous(&self) {
        send(&self.tx, Command::Previous);
    }
    fn pause(&self) {
        send(&self.tx, Command::Pause);
    }
    fn play_pause(&self) {
        send(&self.tx, Command::PlayPause);
    }
    fn stop(&self) {
        send(&self.tx, Command::Stop);
    }
    fn play(&self) {
        send(&self.tx, Command::Play);
    }
    fn seek(&self, offset: i64) {
        send(&self.tx, Command::Seek(offset));
    }
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        send(&self.tx, Command::SetPosition(track_id.to_string(), position));
    }
    fn open_uri(&self, uri: String) {
        send(&self.tx, Command::OpenUri(uri));
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        with(&self.state, |s| s.status.to_owned())
    }
    #[zbus(property)]
    fn loop_status(&self) -> String {
        with(&self.state, |s| s.loop_status.to_owned())
    }
    #[zbus(property)]
    fn set_loop_status(&mut self, v: String) {
        send(&self.tx, Command::SetLoop(v));
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn set_rate(&mut self, _v: f64) {
        // Only 1.0 is supported (MinimumRate = MaximumRate = 1).
    }
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        with(&self.state, |s| s.shuffle)
    }
    #[zbus(property)]
    fn set_shuffle(&mut self, v: bool) {
        send(&self.tx, Command::SetShuffle(v));
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedVariant> {
        with(&self.state, |s| metadata(s.meta.as_ref()))
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        with(&self.state, |s| s.volume)
    }
    #[zbus(property)]
    fn set_volume(&mut self, v: f64) {
        send(&self.tx, Command::SetVolume(v));
    }
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        with(&self.state, |s| s.position)
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        with(&self.state, |s| s.can_next)
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        with(&self.state, |s| s.can_previous)
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        with(&self.state, |s| s.can_play)
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        with(&self.state, |s| s.can_play)
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        with(&self.state, |s| s.meta.as_ref().is_some_and(|m| m.length_us > 0))
    }
    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

/// Handle used by the GTK side to publish state.
#[derive(Clone)]
pub struct Mpris {
    tx: async_channel::Sender<Update>,
}

impl Mpris {
    pub fn update(&self, u: Update) {
        let _ = self.tx.try_send(u);
    }
}

/// Start the MPRIS server thread. Commands arrive on the returned receiver.
pub fn start() -> (Mpris, async_channel::Receiver<Command>) {
    let (cmd_tx, cmd_rx) = async_channel::unbounded();
    let (up_tx, up_rx) = async_channel::unbounded::<Update>();
    let spawned = std::thread::Builder::new().name("m3-music-mpris".into()).spawn(move || {
        zbus::block_on(async move {
            if let Err(e) = serve(cmd_tx, up_rx).await {
                tracing::warn!("MPRIS is unavailable: {e}");
            }
        });
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the MPRIS thread: {e}");
    }
    (Mpris { tx: up_tx }, cmd_rx)
}

async fn serve(tx: async_channel::Sender<Command>, updates: async_channel::Receiver<Update>) -> zbus::Result<()> {
    let state = Arc::new(Mutex::new(State {
        status: "Stopped",
        meta: None,
        position: 0,
        volume: 1.0,
        shuffle: false,
        loop_status: "None",
        can_next: false,
        can_previous: false,
        can_play: false,
    }));
    let conn = zbus::connection::Builder::session()
        .name(BUS_NAME)
        .serve_at(PATH, Root { tx: tx.clone() })
        .serve_at(PATH, PlayerIface { tx, state: state.clone() })
        .build()
        .await?;
    tracing::info!("MPRIS: serving {BUS_NAME}");
    let iface = conn.object_server().interface::<_, PlayerIface>(PATH).await?;
    while let Ok(u) = updates.recv().await {
        let emitter = iface.signal_emitter();
        {
            let mut s = match state.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            match &u {
                Update::Status(v) => s.status = v,
                Update::Track(m) => s.meta = m.clone(),
                Update::Position(p) | Update::Seeked(p) => s.position = *p,
                Update::Volume(v) => s.volume = *v,
                Update::Shuffle(v) => s.shuffle = *v,
                Update::Loop(v) => s.loop_status = v,
                Update::Can { next, previous, play } => {
                    s.can_next = *next;
                    s.can_previous = *previous;
                    s.can_play = *play;
                }
            }
        }
        let p = iface.get().await;
        let r = match u {
            Update::Status(_) => p.playback_status_changed(emitter).await,
            Update::Track(_) => {
                let _ = p.can_seek_changed(emitter).await;
                p.metadata_changed(emitter).await
            }
            Update::Position(_) => Ok(()),
            Update::Seeked(pos) => PlayerIface::seeked(emitter, pos).await,
            Update::Volume(_) => p.volume_changed(emitter).await,
            Update::Shuffle(_) => p.shuffle_changed(emitter).await,
            Update::Loop(_) => p.loop_status_changed(emitter).await,
            Update::Can { .. } => {
                let _ = p.can_go_next_changed(emitter).await;
                let _ = p.can_go_previous_changed(emitter).await;
                let _ = p.can_pause_changed(emitter).await;
                p.can_play_changed(emitter).await
            }
        };
        if let Err(e) = r {
            tracing::debug!("MPRIS signal failed: {e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_mapping() {
        let m = Meta {
            id: 7,
            title: "Song".into(),
            artist: "Band".into(),
            album: "Record".into(),
            album_artist: String::new(),
            length_us: 61_000_000,
            track_no: Some(3),
            art_url: Some("file:///c.jpg".into()),
            url: "file:///s.ogg".into(),
        };
        let map = metadata(Some(&m));
        let get_str = |k: &str| map.get(k).and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        assert_eq!(get_str("xesam:title").as_deref(), Some("Song"));
        assert_eq!(get_str("mpris:artUrl").as_deref(), Some("file:///c.jpg"));
        let len = map.get("mpris:length").and_then(|v| i64::try_from(v.try_clone().ok()?).ok());
        assert_eq!(len, Some(61_000_000));
        let artists = map.get("xesam:artist").and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok());
        assert_eq!(artists, Some(vec!["Band".to_owned()]));
        assert!(map.contains_key("mpris:trackid") && !map.contains_key("xesam:albumArtist"));
        let none = metadata(None);
        assert_eq!(none.len(), 1, "only the NoTrack id without a track");
        assert_eq!(track_path(7), "/org/m3/Music/Track/7");
    }
}
