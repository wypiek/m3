//! NetworkManager integration (org.freedesktop.NetworkManager, system bus).

use std::collections::HashMap;

use futures_util::stream::BoxStream;
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant as OwnedValue, Variant as Value};

use crate::power::{call, prop, proxy, signal_stream};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";

const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_WIFI: u32 = 2;
const DEVICE_STATE_ACTIVATED: u32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct AccessPoint {
    pub ssid: String,
    /// 0 … 100
    pub strength: u8,
    pub secure: bool,
    pub active: bool,
    pub saved: bool,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NetworkState {
    pub available: bool,
    pub wifi_enabled: bool,
    pub wifi_hardware_enabled: bool,
    pub has_wifi_device: bool,
    pub ethernet_connected: bool,
    pub ethernet_present: bool,
    pub connected_ssid: Option<String>,
    pub signal: Option<u8>,
    pub access_points: Vec<AccessPoint>,
}

pub async fn watch(sys: &zbus::Connection) -> Option<BoxStream<'static, ()>> {
    signal_stream(sys, NM).await
}

async fn devices(sys: &zbus::Connection) -> Result<Vec<(OwnedObjectPath, u32, u32)>, String> {
    let nm = proxy(sys, NM, NM_PATH, NM).await?;
    let paths: Vec<OwnedObjectPath> = call(&nm, "GetDevices", &()).await?;
    let mut out = Vec::new();
    for p in paths {
        let d = proxy(sys, NM, p.as_str(), DEVICE).await?;
        let kind: u32 = prop(&d, "DeviceType").await.unwrap_or(0);
        let state: u32 = prop(&d, "State").await.unwrap_or(0);
        out.push((p, kind, state));
    }
    Ok(out)
}

async fn saved_ssids(sys: &zbus::Connection) -> HashMap<String, OwnedObjectPath> {
    let mut out = HashMap::new();
    let Ok(settings) = proxy(sys, NM, SETTINGS_PATH, SETTINGS).await else { return out };
    let Ok(conns): Result<Vec<OwnedObjectPath>, _> = call(&settings, "ListConnections", &()).await else { return out };
    for c in conns {
        let Ok(p) = proxy(sys, NM, c.as_str(), CONNECTION).await else { continue };
        let Ok(s): Result<HashMap<String, HashMap<String, OwnedValue>>, _> = call(&p, "GetSettings", &()).await else {
            continue;
        };
        if let Some(ssid) =
            s.get("802-11-wireless").and_then(|w| w.get("ssid")).and_then(|v| Vec::<u8>::try_from(v.clone()).ok())
        {
            out.insert(String::from_utf8_lossy(&ssid).into_owned(), c);
        }
    }
    out
}

pub async fn state(sys: &zbus::Connection) -> Option<NetworkState> {
    let nm = proxy(sys, NM, NM_PATH, NM).await.ok()?;
    let Ok(wifi_enabled) = prop::<bool>(&nm, "WirelessEnabled").await else {
        return Some(NetworkState::default());
    };
    let mut st = NetworkState {
        available: true,
        wifi_enabled,
        wifi_hardware_enabled: prop(&nm, "WirelessHardwareEnabled").await.unwrap_or(true),
        ..Default::default()
    };
    let saved = saved_ssids(sys).await;
    for (path, kind, dstate) in devices(sys).await.unwrap_or_default() {
        match kind {
            DEVICE_TYPE_ETHERNET => {
                st.ethernet_present = true;
                st.ethernet_connected |= dstate == DEVICE_STATE_ACTIVATED;
            }
            DEVICE_TYPE_WIFI => {
                st.has_wifi_device = true;
                let Ok(w) = proxy(sys, NM, path.as_str(), WIRELESS).await else { continue };
                let active: Option<OwnedObjectPath> = prop(&w, "ActiveAccessPoint").await.ok();
                let aps: Vec<OwnedObjectPath> = call(&w, "GetAccessPoints", &()).await.unwrap_or_default();
                let mut by_ssid: HashMap<String, AccessPoint> = HashMap::new();
                for ap_path in aps {
                    let Ok(ap) = proxy(sys, NM, ap_path.as_str(), AP).await else { continue };
                    let ssid: Vec<u8> = prop(&ap, "Ssid").await.unwrap_or_default();
                    if ssid.is_empty() {
                        continue; // hidden network
                    }
                    let ssid = String::from_utf8_lossy(&ssid).into_owned();
                    let strength: u8 = prop(&ap, "Strength").await.unwrap_or(0);
                    let wpa: u32 = prop(&ap, "WpaFlags").await.unwrap_or(0);
                    let rsn: u32 = prop(&ap, "RsnFlags").await.unwrap_or(0);
                    let flags: u32 = prop(&ap, "Flags").await.unwrap_or(0);
                    let is_active = active.as_ref().is_some_and(|a| a.as_str() == ap_path.as_str());
                    let entry = AccessPoint {
                        saved: saved.contains_key(&ssid),
                        ssid: ssid.clone(),
                        strength,
                        secure: wpa != 0 || rsn != 0 || flags & 1 != 0,
                        active: is_active,
                        path: ap_path.to_string(),
                    };
                    if is_active {
                        st.connected_ssid = Some(ssid.clone());
                        st.signal = Some(strength);
                    }
                    match by_ssid.get(&ssid) {
                        Some(prev) if prev.active || (prev.strength >= strength && !is_active) => {}
                        _ => {
                            by_ssid.insert(ssid, entry);
                        }
                    }
                }
                let mut list: Vec<AccessPoint> = by_ssid.into_values().collect();
                list.sort_by(|a, b| {
                    b.active.cmp(&a.active).then(b.saved.cmp(&a.saved)).then(b.strength.cmp(&a.strength))
                });
                st.access_points = list;
            }
            _ => {}
        }
    }
    Some(st)
}

pub async fn set_wifi_enabled(sys: &zbus::Connection, on: bool) -> Result<(), String> {
    let nm = proxy(sys, NM, NM_PATH, NM).await?;
    nm.set_property("WirelessEnabled", on).await.map_err(|e| format!("cannot switch Wi-Fi: {e}"))
}

async fn wifi_device(sys: &zbus::Connection) -> Result<OwnedObjectPath, String> {
    devices(sys)
        .await?
        .into_iter()
        .find(|(_, k, _)| *k == DEVICE_TYPE_WIFI)
        .map(|(p, _, _)| p)
        .ok_or_else(|| "no Wi-Fi adapter".to_owned())
}

pub async fn scan(sys: &zbus::Connection) -> Result<(), String> {
    let dev = wifi_device(sys).await?;
    let w = proxy(sys, NM, dev.as_str(), WIRELESS).await?;
    let opts: HashMap<&str, Value> = HashMap::new();
    // NetworkManager rate-limits scans; "not allowed" right after a scan is fine.
    match call::<_, ()>(&w, "RequestScan", &(opts,)).await {
        Ok(()) => Ok(()),
        Err(e) if e.contains("Scanning not allowed") => Ok(()),
        Err(e) => Err(format!("Wi-Fi scan failed: {e}")),
    }
}

pub async fn connect(sys: &zbus::Connection, ssid: &str, password: Option<&str>) -> Result<(), String> {
    let dev = wifi_device(sys).await?;
    let nm = proxy(sys, NM, NM_PATH, NM).await?;
    let root = ObjectPath::try_from("/").map_err(|e| e.to_string())?;
    if let Some(conn) = saved_ssids(sys).await.get(ssid) {
        if password.is_none() {
            let _: OwnedObjectPath = call(&nm, "ActivateConnection", &(conn, &dev, &root))
                .await
                .map_err(|e| format!("cannot connect to {ssid}: {e}"))?;
            return Ok(());
        }
    }
    let mut wireless: HashMap<&str, Value> = HashMap::new();
    wireless.insert("ssid", Value::from(ssid.as_bytes().to_vec()));
    wireless.insert("mode", Value::from("infrastructure"));
    let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    let mut c: HashMap<&str, Value> = HashMap::new();
    c.insert("id", Value::from(ssid));
    c.insert("type", Value::from("802-11-wireless"));
    settings.insert("connection", c);
    settings.insert("802-11-wireless", wireless);
    if let Some(psk) = password {
        let mut sec: HashMap<&str, Value> = HashMap::new();
        sec.insert("key-mgmt", Value::from("wpa-psk"));
        sec.insert("psk", Value::from(psk));
        settings.insert("802-11-wireless-security", sec);
    }
    let _: (OwnedObjectPath, OwnedObjectPath) = call(&nm, "AddAndActivateConnection", &(settings, &dev, &root))
        .await
        .map_err(|e| format!("cannot connect to {ssid}: {e}"))?;
    Ok(())
}

pub async fn disconnect_wifi(sys: &zbus::Connection) -> Result<(), String> {
    let dev = wifi_device(sys).await?;
    let d = proxy(sys, NM, dev.as_str(), DEVICE).await?;
    call::<_, ()>(&d, "Disconnect", &()).await.map_err(|e| format!("cannot disconnect: {e}"))
}
