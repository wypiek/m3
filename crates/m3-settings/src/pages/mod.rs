mod about;
mod appearance;
mod applications;
mod desktop;
mod devices;
mod display;
mod input;
mod lock;
mod notifications;
mod power;
mod widgets;

use crate::Ctx;

pub struct Page {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    pub keywords: &'static [&'static str],
    pub build: fn(&std::rc::Rc<Ctx>) -> gtk::Widget,
}

pub const ALL: &[Page] = &[
    Page {
        id: "appearance",
        icon: "palette",
        title: "Appearance",
        keywords: &["theme", "dark", "color", "wallpaper", "font", "icon", "cursor"],
        build: appearance::build,
    },
    Page {
        id: "display",
        icon: "monitor",
        title: "Display",
        keywords: &["resolution", "scale", "rotation", "brightness", "monitor"],
        build: display::build,
    },
    Page {
        id: "widgets",
        icon: "widgets",
        title: "Widgets",
        keywords: &[
            "widgets",
            "clock",
            "greeting",
            "weather",
            "countdown",
            "media",
            "storage",
            "cpu",
            "ram",
            "desktop",
        ],
        build: widgets::build,
    },
    Page {
        id: "lock",
        icon: "lock_clock",
        title: "Lock screen",
        keywords: &["lock", "clock", "wallpaper", "blur", "avatar", "picture", "message", "media", "battery"],
        build: lock::build,
    },
    Page {
        id: "desktop",
        icon: "space_dashboard",
        title: "Desktop",
        keywords: &["workspace", "window", "panel", "tiling", "shortcut", "hot corner"],
        build: desktop::build,
    },
    Page {
        id: "input",
        icon: "keyboard",
        title: "Input",
        keywords: &["mouse", "touchpad", "keyboard", "layout", "accessibility"],
        build: input::build,
    },
    Page {
        id: "network",
        icon: "wifi",
        title: "Network",
        keywords: &["wifi", "ethernet", "internet"],
        build: devices::network,
    },
    Page {
        id: "bluetooth",
        icon: "bluetooth",
        title: "Bluetooth",
        keywords: &["pair", "headphones", "devices"],
        build: devices::bluetooth,
    },
    Page {
        id: "sound",
        icon: "volume_up",
        title: "Sound",
        keywords: &["audio", "volume", "microphone", "output"],
        build: devices::sound,
    },
    Page {
        id: "notifications",
        icon: "notifications",
        title: "Notifications",
        keywords: &["do not disturb", "popups"],
        build: notifications::build,
    },
    Page {
        id: "power",
        icon: "battery_full",
        title: "Power",
        keywords: &["battery", "suspend", "profile", "screen"],
        build: power::build,
    },
    Page {
        id: "applications",
        icon: "apps",
        title: "Applications",
        keywords: &["default", "browser", "installed"],
        build: applications::build,
    },
    Page {
        id: "privacy",
        icon: "lock",
        title: "Privacy & Security",
        keywords: &["lock", "screen lock", "password"],
        build: power::privacy,
    },
    Page {
        id: "about",
        icon: "info",
        title: "About",
        keywords: &["version", "system", "hardware", "license"],
        build: about::build,
    },
];
