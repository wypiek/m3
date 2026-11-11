//! System status shared by the panel, quick settings and OSDs.

use m3_services::{BatteryState, BluetoothState, NetworkState, PowerProfiles, audio::AudioState, power::ChargeState};

#[derive(Debug, Clone, Default)]
pub struct SysState {
    pub network: Option<NetworkState>,
    pub bluetooth: Option<BluetoothState>,
    pub battery: Option<BatteryState>,
    pub profiles: Option<PowerProfiles>,
    pub brightness: Option<f64>,
    pub audio: Option<AudioState>,
    pub dnd: bool,
    pub unread: usize,
}

pub fn network_icon(n: &NetworkState) -> Option<(&'static str, String)> {
    if !n.available {
        return None;
    }
    if n.ethernet_connected {
        return Some(("lan", "Wired connection".into()));
    }
    if !n.has_wifi_device {
        return n.ethernet_present.then(|| ("lan", "Ethernet disconnected".into()));
    }
    if !n.wifi_enabled {
        return Some(("wifi_off", "Wi-Fi off".into()));
    }
    match (&n.connected_ssid, n.signal) {
        (Some(ssid), Some(s)) => {
            let icon = match s {
                75.. => "network_wifi",
                50..=74 => "network_wifi_3_bar",
                25..=49 => "network_wifi_2_bar",
                _ => "network_wifi_1_bar",
            };
            Some((icon, format!("Wi-Fi: {ssid}, signal {s}%")))
        }
        _ => Some(("signal_wifi_0_bar", "Wi-Fi: not connected".into())),
    }
}

pub fn volume_icon(volume: f64, muted: bool) -> &'static str {
    if muted || volume <= 0.005 {
        "volume_off"
    } else if volume < 0.34 {
        "volume_mute"
    } else if volume < 0.67 {
        "volume_down"
    } else {
        "volume_up"
    }
}

pub fn battery_icon(b: &BatteryState) -> &'static str {
    if b.state == ChargeState::Charging || b.state == ChargeState::Full && !b.on_battery {
        return "battery_charging_full";
    }
    match b.percentage as u32 {
        95.. => "battery_full",
        83..=94 => "battery_6_bar",
        67..=82 => "battery_5_bar",
        50..=66 => "battery_4_bar",
        33..=49 => "battery_3_bar",
        20..=32 => "battery_2_bar",
        8..=19 => "battery_1_bar",
        _ => "battery_alert",
    }
}

pub fn battery_text(b: &BatteryState) -> String {
    let pct = format!("{:.0}%", b.percentage);
    let dur = |s: i64| {
        let (h, m) = (s / 3600, (s % 3600) / 60);
        if h > 0 { format!("{h} h {m} min") } else { format!("{m} min") }
    };
    match b.state {
        ChargeState::Charging => match b.time_to_full {
            Some(t) => format!("{pct} · full in {}", dur(t)),
            None => format!("{pct} · charging"),
        },
        ChargeState::Discharging => match b.time_to_empty {
            Some(t) => format!("{pct} · {} left", dur(t)),
            None => pct,
        },
        ChargeState::Full => format!("{pct} · fully charged"),
        ChargeState::NotCharging => format!("{pct} · not charging"),
        ChargeState::Unknown => pct,
    }
}

pub fn profile_label(p: &str) -> (&'static str, &'static str) {
    match p {
        "power-saver" => ("eco", "Power saver"),
        "performance" => ("speed", "Performance"),
        _ => ("balance", "Balanced"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons() {
        assert_eq!(volume_icon(0.5, true), "volume_off");
        assert_eq!(volume_icon(0.9, false), "volume_up");
        let b = BatteryState {
            present: true,
            percentage: 55.0,
            state: ChargeState::Discharging,
            time_to_empty: Some(5400),
            time_to_full: None,
            on_battery: true,
        };
        assert_eq!(battery_icon(&b), "battery_4_bar");
        assert_eq!(battery_text(&b), "55% · 1 h 30 min left");
        let n = NetworkState {
            available: true,
            wifi_enabled: true,
            has_wifi_device: true,
            connected_ssid: Some("home".into()),
            signal: Some(80),
            ..Default::default()
        };
        assert_eq!(network_icon(&n).unwrap().0, "network_wifi");
        assert!(network_icon(&NetworkState::default()).is_none());
    }
}
