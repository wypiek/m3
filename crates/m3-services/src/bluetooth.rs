//! Bluetooth through BlueZ (org.bluez, system bus).

use std::collections::HashMap;

use futures_util::stream::BoxStream;
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant as OwnedValue, interface};

use crate::power::{call, proxy, signal_stream};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const AGENT_PATH: &str = "/org/m3/BluetoothAgent";

#[derive(Debug, Clone, PartialEq)]
pub struct BluetoothDevice {
    pub path: String,
    pub name: String,
    pub address: String,
    /// freedesktop icon name from BlueZ (`audio-headset`, `input-mouse`, …)
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
    pub battery: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BluetoothState {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    pub adapter: Option<String>,
    pub devices: Vec<BluetoothDevice>,
}

type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

async fn managed(sys: &zbus::Connection) -> Result<Managed, String> {
    let om = proxy(sys, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager").await?;
    call(&om, "GetManagedObjects", &()).await
}

fn get<T: TryFrom<OwnedValue>>(m: &HashMap<String, OwnedValue>, k: &str) -> Option<T> {
    m.get(k).and_then(|v| T::try_from(v.clone()).ok())
}

pub async fn watch(sys: &zbus::Connection) -> Option<BoxStream<'static, ()>> {
    signal_stream(sys, BLUEZ).await
}

pub async fn state(sys: &zbus::Connection) -> Option<BluetoothState> {
    let objects = managed(sys).await.ok()?;
    let mut st = BluetoothState { available: true, ..Default::default() };
    for (path, ifaces) in &objects {
        if let Some(a) = ifaces.get(ADAPTER) {
            if st.adapter.is_none() {
                st.adapter = Some(path.to_string());
                st.powered = get(a, "Powered").unwrap_or(false);
                st.discovering = get(a, "Discovering").unwrap_or(false);
            }
        }
    }
    if st.adapter.is_none() {
        return Some(BluetoothState::default());
    }
    for (path, ifaces) in &objects {
        let Some(d) = ifaces.get(DEVICE) else { continue };
        let name: Option<String> = get(d, "Alias").or_else(|| get(d, "Name"));
        let paired: bool = get(d, "Paired").unwrap_or(false);
        // Skip anonymous unpaired devices (beacons) to keep the list useful.
        if name.is_none() && !paired {
            continue;
        }
        let battery = ifaces.get("org.bluez.Battery1").and_then(|b| get::<u8>(b, "Percentage"));
        st.devices.push(BluetoothDevice {
            path: path.to_string(),
            address: get(d, "Address").unwrap_or_default(),
            name: name.unwrap_or_else(|| get(d, "Address").unwrap_or_default()),
            icon: get(d, "Icon").unwrap_or_else(|| "bluetooth".into()),
            paired,
            connected: get(d, "Connected").unwrap_or(false),
            battery,
        });
    }
    st.devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.paired.cmp(&a.paired)).then(a.name.cmp(&b.name)));
    Some(st)
}

async fn adapter_path(sys: &zbus::Connection) -> Result<String, String> {
    state(sys).await.and_then(|s| s.adapter).ok_or_else(|| "no Bluetooth adapter".to_owned())
}

pub async fn set_powered(sys: &zbus::Connection, on: bool) -> Result<(), String> {
    let path = adapter_path(sys).await?;
    let a = proxy(sys, BLUEZ, &path, ADAPTER).await?;
    a.set_property("Powered", on).await.map_err(|e| format!("cannot switch Bluetooth: {e}"))
}

pub async fn set_discovery(sys: &zbus::Connection, on: bool) -> Result<(), String> {
    let path = adapter_path(sys).await?;
    let a = proxy(sys, BLUEZ, &path, ADAPTER).await?;
    let method = if on { "StartDiscovery" } else { "StopDiscovery" };
    match call::<_, ()>(&a, method, &()).await {
        Ok(()) => Ok(()),
        // Already in the requested state.
        Err(e) if e.contains("InProgress") || e.contains("NotReady") && !on => Ok(()),
        Err(e) => Err(format!("Bluetooth discovery: {e}")),
    }
}

pub async fn device_call(sys: &zbus::Connection, path: &str, method: &str) -> Result<(), String> {
    let d = proxy(sys, BLUEZ, path, DEVICE).await?;
    call::<_, ()>(&d, method, &()).await.map_err(|e| format!("Bluetooth {}: {e}", method.to_lowercase()))
}

pub async fn pair(sys: &zbus::Connection, path: &str) -> Result<(), String> {
    device_call(sys, path, "Pair").await?;
    let d = proxy(sys, BLUEZ, path, DEVICE).await?;
    // Trust so it can reconnect by itself, then connect.
    let _ = d.set_property("Trusted", true).await;
    device_call(sys, path, "Connect").await
}

pub async fn forget(sys: &zbus::Connection, path: &str) -> Result<(), String> {
    let adapter = adapter_path(sys).await?;
    let a = proxy(sys, BLUEZ, &adapter, ADAPTER).await?;
    let dev = ObjectPath::try_from(path).map_err(|e| e.to_string())?;
    call::<_, ()>(&a, "RemoveDevice", &(dev,)).await.map_err(|e| format!("cannot forget device: {e}"))
}

/// A pairing agent with no input/output capability: BlueZ then uses "just
/// works" pairing, which covers headsets, mice, keyboards with no PIN, and
/// speakers. Devices that require entering a PIN are rejected (with a
/// clear error) until a PIN dialog exists.
struct Agent;

#[interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}
    fn request_pin_code(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::Failed("PIN entry is not supported yet".into()))
    }
    fn request_passkey(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::Failed("passkey entry is not supported yet".into()))
    }
    fn display_pin_code(&self, _device: OwnedObjectPath, _pin: String) {}
    fn display_passkey(&self, _device: OwnedObjectPath, _passkey: u32, _entered: u16) {}
    fn request_confirmation(&self, _device: OwnedObjectPath, _passkey: u32) {}
    fn request_authorization(&self, _device: OwnedObjectPath) {}
    fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) {}
    fn cancel(&self) {}
}

pub async fn register_agent(sys: &zbus::Connection) -> Result<(), String> {
    sys.object_server().at(AGENT_PATH, Agent).await.map_err(|e| e.to_string())?;
    let mgr = proxy(sys, BLUEZ, "/org/bluez", "org.bluez.AgentManager1").await?;
    let path = ObjectPath::try_from(AGENT_PATH).map_err(|e| e.to_string())?;
    call::<_, ()>(&mgr, "RegisterAgent", &(&path, "NoInputNoOutput")).await?;
    call::<_, ()>(&mgr, "RequestDefaultAgent", &(&path,)).await
}
