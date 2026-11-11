//! Battery level for the lock screen, read from `/sys/class/power_supply`.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Battery {
    pub percent: u32,
    pub charging: bool,
}

/// One supply's `(type, scope, capacity, status)` → its contribution, if it
/// is a system battery (peripherals report `scope = Device`) (pure; tested).
pub fn system_battery(kind: &str, scope: &str, capacity: &str, status: &str) -> Option<Battery> {
    if kind.trim() != "Battery" || scope.trim() == "Device" {
        return None;
    }
    let percent = capacity.trim().parse::<u32>().ok()?.min(100);
    let status = status.trim();
    Some(Battery { percent, charging: status == "Charging" || status == "Full" })
}

/// Combined system batteries (average level), `None` without a battery.
pub fn read() -> Option<Battery> {
    read_from(Path::new("/sys/class/power_supply"))
}

fn read_from(dir: &Path) -> Option<Battery> {
    let r = |p: &Path, f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
    let found: Vec<Battery> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            system_battery(&r(&p, "type"), &r(&p, "scope"), &r(&p, "capacity"), &r(&p, "status"))
        })
        .collect();
    if found.is_empty() {
        return None;
    }
    let percent = found.iter().map(|b| b.percent).sum::<u32>() / found.len() as u32;
    Some(Battery { percent, charging: found.iter().any(|b| b.charging) })
}

/// Material Symbols name for a battery level.
pub fn icon(b: Battery) -> &'static str {
    if b.charging {
        return "battery_charging_full";
    }
    match b.percent {
        90.. => "battery_full",
        75..=89 => "battery_6_bar",
        60..=74 => "battery_5_bar",
        45..=59 => "battery_4_bar",
        30..=44 => "battery_3_bar",
        15..=29 => "battery_2_bar",
        5..=14 => "battery_1_bar",
        _ => "battery_alert",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_peripherals_and_parses() {
        assert_eq!(
            system_battery("Battery\n", "", "85\n", "Discharging\n"),
            Some(Battery { percent: 85, charging: false })
        );
        assert_eq!(system_battery("Battery", "System", "100", "Full").map(|b| b.charging), Some(true));
        assert_eq!(system_battery("Battery", "Device", "50", "Discharging"), None);
        assert_eq!(system_battery("Mains", "", "", ""), None);
        assert_eq!(system_battery("Battery", "", "garbage", ""), None);
    }

    #[test]
    fn reads_a_sysfs_tree() {
        let dir = tempdir();
        for (name, kind, cap, status) in [("BAT0", "Battery", "40", "Discharging"), ("AC", "Mains", "", "")] {
            let p = dir.join(name);
            std::fs::create_dir_all(&p).expect("mkdir");
            std::fs::write(p.join("type"), kind).expect("write");
            std::fs::write(p.join("capacity"), cap).expect("write");
            std::fs::write(p.join("status"), status).expect("write");
        }
        assert_eq!(read_from(&dir), Some(Battery { percent: 40, charging: false }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icons() {
        assert_eq!(icon(Battery { percent: 95, charging: false }), "battery_full");
        assert_eq!(icon(Battery { percent: 3, charging: false }), "battery_alert");
        assert_eq!(icon(Battery { percent: 3, charging: true }), "battery_charging_full");
    }

    fn tempdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("m3-lock-battery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
}
