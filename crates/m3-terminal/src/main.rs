//! m3-terminal: the m3 terminal emulator (VTE) with Material 3 Expressive
//! chrome and colours derived from the desktop theme.

mod links;
mod palette;
mod shell;
mod window;

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use m3_widgets::AppStyle;

const APP_ID: &str = "org.m3.Terminal";

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::empty()).build();

    let style: Rc<std::cell::OnceCell<Rc<AppStyle>>> = Rc::default();
    {
        let style = style.clone();
        app.connect_startup(move |app| {
            let s = AppStyle::install(include_str!("terminal.css"));
            let _ = style.set(s);
            window::install_accels(app);
            let new_window = gio::SimpleAction::new("new-window", None);
            let a = app.clone();
            let st = style.clone();
            new_window.connect_activate(move |_, _| {
                if let Some(s) = st.get() {
                    window::TerminalWindow::open(&a, s.clone(), None);
                }
            });
            app.add_action(&new_window);
        });
    }
    // Every activation (also a second `m3-terminal` launch) opens a window.
    app.connect_activate(move |app| {
        if let Some(s) = style.get() {
            window::TerminalWindow::open(app, s.clone(), None);
        }
    });
    app.run()
}
