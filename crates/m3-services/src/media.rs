//! Media keys via MPRIS (org.mpris.MediaPlayer2.*, session bus).

use crate::{
    Command,
    power::{call, prop, proxy},
};

/// Send play/pause/next/previous to the most relevant player: one that is
/// playing, else the first player found.
pub async fn control(session: &zbus::Connection, cmd: &Command) -> Result<(), String> {
    let dbus = proxy(session, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").await?;
    let names: Vec<String> = call(&dbus, "ListNames", &()).await?;
    let players: Vec<String> = names.into_iter().filter(|n| n.starts_with("org.mpris.MediaPlayer2.")).collect();
    if players.is_empty() {
        return Err("no media player is running".into());
    }
    let mut target = players[0].clone();
    for p in &players {
        let Ok(px) = proxy(session, p, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").await else {
            continue;
        };
        if prop::<String>(&px, "PlaybackStatus").await.as_deref() == Ok("Playing") {
            target = p.clone();
            break;
        }
    }
    let player = proxy(session, &target, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player").await?;
    let method = match cmd {
        Command::MediaNext => "Next",
        Command::MediaPrevious => "Previous",
        _ => "PlayPause",
    };
    call::<_, ()>(&player, method, &()).await
}
