//! Battery (UPower), power profiles (power-profiles-daemon), backlight and
//! logind session power actions — plus small D-Bus helpers shared by the
//! other modules.

use std::path::PathBuf;

use futures_util::{StreamExt, stream::BoxStream};
use serde::de::DeserializeOwned;
use zbus::OwnedVariant as OwnedValue;
use zbus::zvariant::DynamicType;

// ---- helpers ----------------------------------------------------------------

pub(crate) async fn proxy<'a>(
    conn: &zbus::Connection,
    dest: &'a str,
    path: &'a str,
    iface: &'a str,
) -> Result<zbus::Proxy<'a>, String> {
    zbus::Proxy::new(conn, dest, path, iface).await.map_err(|e| e.to_string())
}

pub(crate) async fn prop<T>(p: &zbus::Proxy<'_>, name: &str) -> Result<T, String>
where
    T: DeserializeOwned + zbus::zvariant::Type,
{
    p.get_property::<T>(name).await.map_err(|e| e.to_string())
}

pub(crate) async fn call<B, R>(p: &zbus::Proxy<'_>, method: &str, body: &B) -> Result<R, String>
where
    B: serde::Serialize + DynamicType,
    R: DeserializeOwned + zbus::zvariant::Type,
{
    p.call::<_, _, R>(method, body).await.map_err(|e| e.to_string())
}

/// A stream that fires (coalesced) whenever `sender` emits any signal.
pub(crate) async fn signal_stream(conn: &zbus::Connection, sender: &'static str) -> Option<BoxStream<'static, ()>> {
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(sender).build().ok()?;
    let stream = zbus::MessageStream::for_match_rule(rule, conn, std::num::NonZeroUsize::new(256)).await.ok()?;
    Some(stream.ready_chunks(256).map(|_| ()).boxed())
}

// ---- battery -------------------------------------------------------------------

const UPOWER: &str = "org.freedesktop.UPower";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeState {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryState {
    pub present: bool,
    pub percentage: f64,
    pub state: ChargeState,
    /// Seconds until empty / full, if known.
    pub time_to_empty: Option<i64>,
    pub time_to_full: Option<i64>,
    pub on_battery: bool,
}

pub async fn watch_battery(sys: &zbus::Connection) -> Option<BoxStream<'static, ()>> {
    signal_stream(sys, UPOWER).await
}

pub async fn battery(sys: &zbus::Connection) -> Option<BatteryState> {
    let dev = proxy(sys, UPOWER, "/org/freedesktop/UPower/devices/DisplayDevice", "org.freedesktop.UPower.Device")
        .await
        .ok()?;
    let root = proxy(sys, UPOWER, "/org/freedesktop/UPower", UPOWER).await.ok()?;
    let present: bool = prop(&dev, "IsPresent").await.ok()?;
    let state: u32 = prop(&dev, "State").await.unwrap_or(0);
    let pos = |v: i64| (v > 0).then_some(v);
    Some(BatteryState {
        present,
        percentage: prop(&dev, "Percentage").await.unwrap_or(0.0),
        state: match state {
            1 | 5 => ChargeState::Charging,
            2 | 6 => ChargeState::Discharging,
            4 => ChargeState::Full,
            3 => ChargeState::NotCharging,
            _ => ChargeState::Unknown,
        },
        time_to_empty: prop::<i64>(&dev, "TimeToEmpty").await.ok().and_then(pos),
        time_to_full: prop::<i64>(&dev, "TimeToFull").await.ok().and_then(pos),
        on_battery: prop(&root, "OnBattery").await.unwrap_or(false),
    })
}

// ---- power profiles ------------------------------------------------------------

const PPD: &str = "net.hadess.PowerProfiles";
const PPD_PATH: &str = "/net/hadess/PowerProfiles";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PowerProfiles {
    pub active: String,
    /// e.g. ["power-saver", "balanced", "performance"]
    pub available: Vec<String>,
}

pub async fn watch_profiles(sys: &zbus::Connection) -> Option<BoxStream<'static, ()>> {
    signal_stream(sys, PPD).await
}

pub async fn profiles(sys: &zbus::Connection) -> Option<PowerProfiles> {
    let p = proxy(sys, PPD, PPD_PATH, PPD).await.ok()?;
    let active: String = prop(&p, "ActiveProfile").await.ok()?;
    let list: Vec<std::collections::HashMap<String, OwnedValue>> = p.get_property("Profiles").await.unwrap_or_default();
    let available =
        list.iter().filter_map(|m| m.get("Profile").and_then(|v| String::try_from(v.clone()).ok())).collect();
    Some(PowerProfiles { active, available })
}

pub async fn set_profile(sys: &zbus::Connection, profile: &str) -> Result<(), String> {
    let p = proxy(sys, PPD, PPD_PATH, PPD).await?;
    p.set_property("ActiveProfile", profile).await.map_err(|e| format!("cannot set power profile: {e}"))
}

// ---- backlight -------------------------------------------------------------------

struct Backlight {
    name: String,
    dir: PathBuf,
    max: u32,
}

/// The preferred backlight device: firmware > platform > raw interfaces.
fn backlight() -> Option<Backlight> {
    let mut best: Option<(u8, Backlight)> = None;
    for entry in std::fs::read_dir("/sys/class/backlight").ok()?.flatten() {
        let dir = entry.path();
        let Some(max) =
            std::fs::read_to_string(dir.join("max_brightness")).ok().and_then(|s| s.trim().parse::<u32>().ok())
        else {
            continue;
        };
        if max == 0 {
            continue;
        }
        let rank = match std::fs::read_to_string(dir.join("type")).unwrap_or_default().trim() {
            "firmware" => 3,
            "platform" => 2,
            _ => 1,
        };
        if best.as_ref().is_none_or(|(r, _)| rank > *r) {
            let name = entry.file_name().to_string_lossy().into_owned();
            best = Some((rank, Backlight { name, dir, max }));
        }
    }
    best.map(|(_, b)| b)
}

/// Current brightness 0.0 … 1.0, if the machine has a backlight.
pub fn brightness() -> Option<f64> {
    let b = backlight()?;
    let cur: u32 = std::fs::read_to_string(b.dir.join("brightness")).ok()?.trim().parse().ok()?;
    Some(cur as f64 / b.max as f64)
}

/// Set brightness through logind (no root, no udev rules needed).
pub async fn set_brightness(sys: &zbus::Connection, value: f64) -> Result<f64, String> {
    let b = backlight().ok_or("this computer has no adjustable backlight")?;
    // Never go fully dark through the slider.
    let v = value.clamp(0.01, 1.0);
    let raw = ((v * b.max as f64).round() as u32).max(1);
    let session =
        proxy(sys, "org.freedesktop.login1", "/org/freedesktop/login1/session/auto", "org.freedesktop.login1.Session")
            .await?;
    call::<_, ()>(&session, "SetBrightness", &("backlight", b.name.as_str(), raw))
        .await
        .map_err(|e| format!("cannot set brightness: {e}"))?;
    Ok(raw as f64 / b.max as f64)
}

pub async fn step_brightness(sys: &zbus::Connection, delta: f64) -> Result<f64, String> {
    let cur = brightness().ok_or("this computer has no adjustable backlight")?;
    set_brightness(sys, cur + delta).await
}

// ---- logind ------------------------------------------------------------------------

pub async fn logind_call(sys: &zbus::Connection, method: &str) -> Result<(), String> {
    let m = proxy(sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await?;
    // `true` = interactive: polkit may ask for authentication.
    call::<_, ()>(&m, method, &(true,)).await.map_err(|e| format!("{method} failed: {e}"))
}

/// `PrepareForSleep(bool)` from logind.
pub async fn watch_sleep(sys: &zbus::Connection) -> Option<BoxStream<'static, bool>> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")
        .interface("org.freedesktop.login1.Manager")
        .member("PrepareForSleep")
        .build()
        .ok()?;
    let stream = zbus::MessageStream::for_match_rule(rule, sys, std::num::NonZeroUsize::new(16)).await.ok()?;
    Some(stream.filter_map(|m| async move { m.ok()?.body().deserialize::<bool>().ok() }).boxed())
}

/// A logind "delay" inhibitor for sleep; held until dropped.
pub async fn sleep_inhibitor(sys: &zbus::Connection) -> Option<zbus::OwnedFd> {
    let m =
        proxy(sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").await.ok()?;
    match call::<_, zbus::OwnedFd>(&m, "Inhibit", &("sleep", "m3", "Lock the screen before suspending", "delay")).await
    {
        Ok(fd) => Some(fd),
        Err(e) => {
            tracing::info!("cannot delay suspend to lock first: {e}");
            None
        }
    }
}
