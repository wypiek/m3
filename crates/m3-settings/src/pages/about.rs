//! About: versions, system information, licenses.

use std::rc::Rc;

use gtk::prelude::*;
use m3_protocol::{Request, Response};

use crate::{Ctx, store, ui};

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_owned()
}

fn os_name() -> String {
    read("/etc/os-release")
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned()))
        .unwrap_or_else(|| "Linux".into())
}

fn cpu() -> String {
    let info = read("/proc/cpuinfo");
    let model = info
        .lines()
        .find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_owned()))
        .unwrap_or_else(|| std::env::consts::ARCH.to_owned());
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    format!("{model} × {cores}")
}

fn memory() -> String {
    let kb: u64 = read("/proc/meminfo")
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:").and_then(|v| v.split_whitespace().next()?.parse().ok()))
        .unwrap_or(0);
    format!("{:.1} GiB", kb as f64 / 1024.0 / 1024.0)
}

pub fn build(_ctx: &Rc<Ctx>) -> gtk::Widget {
    let (page, content) = ui::page("About", "");
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 4);
    hero.add_css_class("m3-about-hero");
    // The logo sits on an expressive shape that morphs under the pointer.
    let logo = m3_widgets::ShapeIcon::new(m3_widgets::Shape::Cookie(9), "m3", 152, false);
    logo.label().add_css_class("m3-display-large-emphasized");
    logo.widget().add_css_class("m3-about-logo");
    m3_widgets::morph_on_interaction(
        logo.widget(),
        logo.shape(),
        m3_widgets::Shape::Cookie(9),
        m3_widgets::Shape::Sunny(8),
        m3_widgets::Shape::Clover(4),
    );
    let logo = logo.widget().clone();
    let tag = gtk::Label::new(Some("A Material 3 Expressive desktop for Linux"));
    tag.add_css_class("m3-title-medium");
    hero.append(&logo);
    hero.append(&tag);
    content.append(&hero);

    let card = ui::card(&content, "Software");
    ui::info(&card, "m3", m3_core::VERSION);
    let comp = match store::query(Request::Version) {
        Some(Response::Version { compositor, protocol }) => format!("{compositor} (IPC {protocol})"),
        _ => "not running".into(),
    };
    ui::info(&card, "Compositor", &comp);
    ui::info(&card, "Operating system", &os_name());
    ui::info(&card, "Kernel", &read("/proc/sys/kernel/osrelease"));
    ui::info(&card, "GTK", &format!("{}.{}.{}", gtk::major_version(), gtk::minor_version(), gtk::micro_version()));

    let card = ui::card(&content, "Hardware");
    ui::info(&card, "Device name", &read("/proc/sys/kernel/hostname"));
    let vendor = read("/sys/class/dmi/id/sys_vendor");
    let product = read("/sys/class/dmi/id/product_name");
    if !vendor.is_empty() || !product.is_empty() {
        ui::info(&card, "Model", format!("{vendor} {product}").trim());
    }
    ui::info(&card, "Processor", &cpu());
    ui::info(&card, "Memory", &memory());

    let card = ui::card(&content, "Licenses");
    ui::info(&card, "m3", "GPL-3.0-or-later");
    ui::info(&card, "Smithay (compositor library)", "MIT");
    ui::info(&card, "Material Symbols (icons)", "Apache-2.0");
    ui::info(&card, "Material Color Utilities port", "Apache-2.0");
    ui::info(&card, "GTK", "LGPL-2.1-or-later");

    page.upcast()
}
