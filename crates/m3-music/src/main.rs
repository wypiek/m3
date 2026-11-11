//! m3 Music: a Material 3 Expressive music player for the m3 desktop.
//!
//! Plays the audio files in the XDG music folder with GStreamer, shows
//! them as songs, albums and artists, colours the now-playing view from the
//! album art, and is controllable over MPRIS (media keys).

mod art;
mod library;
mod mpris;
mod player;
mod queue;
mod state;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

thread_local! {
    static MUSIC: RefCell<Option<Rc<ui::Music>>> = const { RefCell::new(None) };
}

fn music(app: &gtk::Application) -> Rc<ui::Music> {
    if let Some(m) = MUSIC.with(|m| m.borrow().clone()) {
        return m;
    }
    let style = m3_widgets::AppStyle::install(include_str!("music.css"));
    let m = ui::Music::new(app, style);
    MUSIC.with(|slot| *slot.borrow_mut() = Some(m.clone()));
    m
}

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    if let Err(e) = gstreamer::init() {
        eprintln!("m3-music: cannot initialise GStreamer: {e}");
        return glib::ExitCode::FAILURE;
    }
    let app =
        gtk::Application::builder().application_id("org.m3.Music").flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    app.connect_activate(|app| music(app).window().present());
    app.connect_open(|app, files, _| {
        let m = music(app);
        m.window().present();
        let paths = files.iter().filter_map(|f| f.path()).collect();
        m.open_files(paths);
    });
    app.connect_shutdown(|_| {
        if let Some(m) = MUSIC.with(|m| m.borrow_mut().take()) {
            m.save_state();
        }
    });
    app.run()
}
