//! m3-shell — the m3 desktop shell.
//!
//! One GTK application owning every shell surface (wallpaper, panel,
//! launcher, …) as wlr-layer-shell surfaces. It talks to the compositor over
//! the m3 IPC socket and can be restarted at any time without affecting
//! running applications.

mod apps;
mod background;
mod desktop;
mod icons;
mod ipc;
mod launcher;
mod notifications;
mod osd;
mod overview;
mod panel;
mod placement;
mod quick_settings;
mod screenshot;
mod shell;
mod status;
mod style;
mod tray;

use gtk::{gio, glib, prelude::*};

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    let app =
        gtk::Application::builder().application_id("org.m3.Shell").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_startup(|app| {
        if !gtk4_layer_shell::is_supported() {
            tracing::error!("the compositor does not support wlr-layer-shell; m3-shell needs the m3 compositor");
            app.quit();
            return;
        }
        shell::Shell::start(app);
    });
    app.connect_activate(|_| {});
    app.run_with_args::<&str>(&[])
}
