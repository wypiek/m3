//! m3-files: the m3 file manager (GIO), Material 3 Expressive.

mod dialogs;
mod ops;
mod sidebar;
mod thumbs;
mod trash;
mod util;
mod window;

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use m3_widgets::AppStyle;
use window::{FilesWindow, Location};

const APP_ID: &str = "org.m3.Files";

const FM1_XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
    <method name="ShowItems"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
    <method name="ShowItemProperties"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
  </interface>
</node>"#;

fn install_accels(app: &gtk::Application) {
    for (action, keys) in [
        ("win.back", &["<Alt>Left"][..]),
        ("win.forward", &["<Alt>Right"]),
        ("win.up", &["<Alt>Up"]),
        ("win.home", &["<Alt>Home"]),
        ("win.location", &["<Ctrl>l"]),
        ("win.search", &["<Ctrl>f"]),
        ("win.show-hidden", &["<Ctrl>h"]),
        ("win.bookmark", &["<Ctrl>d"]),
        ("win.new-folder", &["<Ctrl><Shift>n"]),
        ("win.refresh", &["F5", "<Ctrl>r"]),
        ("win.toggle-view", &["<Ctrl>1"]),
        ("win.close", &["<Ctrl>w"]),
        ("app.new-window", &["<Ctrl>n"]),
    ] {
        app.set_accels_for_action(action, keys);
    }
}

/// Show `file`: folders open in a window; files open their folder with the
/// file selected.
fn show(app: &gtk::Application, file: gio::File, as_folder: bool) -> Rc<FilesWindow> {
    let is_dir =
        file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) == gio::FileType::Directory;
    if as_folder && is_dir {
        return FilesWindow::open(app, Location::Folder(file), None);
    }
    match file.parent() {
        Some(p) if !is_dir || !as_folder => FilesWindow::open(app, Location::Folder(p), Some(file)),
        _ => FilesWindow::open(app, Location::Folder(file), None),
    }
}

/// `org.freedesktop.FileManager1`, used by browsers ("Show in folder") and
/// the m3 shell's screenshot notification.
fn own_file_manager1(app: &gtk::Application) {
    let app = app.clone();
    let iface = match gio::DBusNodeInfo::for_xml(FM1_XML) {
        Ok(n) => n.lookup_interface("org.freedesktop.FileManager1"),
        Err(e) => {
            tracing::warn!("FileManager1 introspection: {e}");
            return;
        }
    };
    let Some(iface) = iface else { return };
    let _id = gio::bus_own_name(
        gio::BusType::Session,
        "org.freedesktop.FileManager1",
        gio::BusNameOwnerFlags::ALLOW_REPLACEMENT,
        move |conn, _| {
            let app = app.clone();
            let r = conn
                .register_object("/org/freedesktop/FileManager1", &iface)
                .method_call(move |_conn, _sender, _path, _iface, method, params, invocation| {
                    let Some((uris, _startup)) = params.get::<(Vec<String>, String)>() else {
                        invocation.return_error(gio::IOErrorEnum::InvalidArgument, "expected (as, s)");
                        return;
                    };
                    for uri in uris {
                        let f = gio::File::for_uri(&uri);
                        match method {
                            "ShowFolders" => {
                                show(&app, f, true);
                            }
                            "ShowItems" => {
                                show(&app, f, false);
                            }
                            "ShowItemProperties" => {
                                let w = show(&app, f.clone(), false);
                                dialogs::properties(w.window.upcast_ref(), &f);
                            }
                            _ => {}
                        }
                    }
                    invocation.return_value(None);
                })
                .build();
            if let Err(e) = r {
                tracing::warn!("can't export FileManager1: {e}");
            }
        },
        |_, name| tracing::debug!("owning {name}"),
        |_, name| tracing::debug!("not owning {name} (another file manager has it)"),
    );
}

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::HANDLES_OPEN).build();
    let style: Rc<std::cell::OnceCell<Rc<AppStyle>>> = Rc::default();
    {
        let style = style.clone();
        app.connect_startup(move |app| {
            let _ = style.set(AppStyle::install(include_str!("files.css")));
            install_accels(app);
            let nw = gio::SimpleAction::new("new-window", None);
            let a = app.clone();
            nw.connect_activate(move |_, _| {
                let loc = FilesWindow::any()
                    .map(|w| w.location_for_new_window())
                    .unwrap_or_else(|| Location::Folder(gio::File::for_path(glib::home_dir())));
                FilesWindow::open(&a, loc, None);
            });
            app.add_action(&nw);
            own_file_manager1(app);
        });
    }
    app.connect_activate(|app| {
        FilesWindow::open(app, Location::Folder(gio::File::for_path(glib::home_dir())), None);
    });
    app.connect_open(|app, files, _| {
        for f in files {
            show(app, f.clone(), true);
        }
    });
    let r = app.run();
    drop(style);
    r
}
