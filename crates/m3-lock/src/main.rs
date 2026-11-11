//! m3-lock — the m3 lock screen.
//!
//! Uses ext-session-lock-v1 (via gtk4-session-lock), so the compositor
//! guarantees nothing else is visible or receives input while locked, and
//! the session stays locked if this process dies. Authentication goes
//! through PAM (`/etc/pam.d/m3-lock`).
//!
//! Its look comes from `lock.toml`; `m3-lock --preview` shows it in a
//! normal window without locking or authenticating anything.

mod auth;
mod avatar;
mod battery;
mod look;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, gio, glib, prelude::*};
use gtk4_session_lock::Instance;
use m3_core::config::{self as cfg, LockClockPosition, LockConfig, ThemeConfig};
use m3_theme::Theme;
use tracing::{error, info, warn};
use zeroize::Zeroizing;

const CSS: &str = include_str!("lock.css");

/// The now-playing card of one window.
struct MediaView {
    root: gtk::Box,
    art: gtk::Stack,
    picture: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    play_icon: gtk::Label,
}

struct Lock {
    /// `None` in preview mode: nothing is locked and nothing authenticates.
    instance: Option<Instance>,
    preview: bool,
    user: auth::User,
    entries: RefCell<Vec<gtk::PasswordEntry>>,
    messages: RefCell<Vec<gtk::Label>>,
    caps: RefCell<Vec<gtk::Label>>,
    clocks: RefCell<Vec<(gtk::Label, gtk::Label)>>,
    /// The unlock buttons' content: an arrow, or the expressive loading
    /// indicator while PAM is checking.
    go_stacks: RefCell<Vec<gtk::Stack>>,
    media: RefCell<Vec<MediaView>>,
    /// Player shown in the media card (bus name).
    media_player: RefCell<Option<String>>,
    batteries: RefCell<Vec<(gtk::Box, gtk::Label, gtk::Label)>>,
    busy: Cell<bool>,
    failures: Cell<u32>,
    theme_cfg: ThemeConfig,
    cfg: LockConfig,
}

fn main() -> glib::ExitCode {
    {
        use tracing_subscriber::{EnvFilter, fmt};
        let filter = EnvFilter::try_from_env("M3_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
        fmt().with_env_filter(filter).with_target(false).with_writer(std::io::stderr).init();
    }
    // `--preview`: show the lock screen in a normal window, without locking,
    // so its look can be checked from Settings.
    let preview = std::env::args().skip(1).any(|a| a == "--preview");
    let app =
        gtk::Application::builder().application_id("org.m3.Lock").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_activate(move |app| activate(app, preview));
    app.run_with_args::<&str>(&[])
}

fn activate(app: &gtk::Application, preview: bool) {
    if !preview && !gtk4_session_lock::is_supported() {
        error!("the compositor does not support ext-session-lock-v1; refusing to show an insecure lock screen");
        app.quit();
        return;
    }
    let Some(user) = auth::current_user() else {
        error!("cannot determine the current user; not locking");
        app.quit();
        return;
    };
    let dir = cfg::default_dir().ok();
    let theme_cfg = dir.as_ref().map(|d| cfg::load_or(d, ThemeConfig::default()).0).unwrap_or_default();
    // A broken lock.toml never stops the screen from locking: defaults are used.
    let lock_cfg = match dir.as_ref().map(|d| cfg::load_or(d, LockConfig::default())) {
        Some((c, Some(e))) => {
            warn!("{e}");
            c
        }
        Some((c, None)) => c,
        None => LockConfig::default(),
    };
    let seed = (!theme_cfg.wallpaper.path.as_os_str().is_empty())
        .then(|| m3_theme::wallpaper::cached_seed(&theme_cfg.wallpaper.path))
        .flatten();
    // The lock screen is always dark: calmer at night, consistent at a glance.
    let mut dark_cfg = theme_cfg.clone();
    dark_cfg.mode = cfg::ThemeMode::Dark;
    let theme = Theme::resolve(&dark_cfg, seed, m3_core::time::local_minutes());
    let provider = gtk::CssProvider::new();
    let mut css = m3_theme::css::gtk_css(&theme, &theme_cfg.font_family, &theme_cfg.monospace_font_family, false);
    css.push_str(CSS);
    css.push_str(&look::dynamic_css(&lock_cfg));
    provider.load_from_string(&css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let instance = (!preview).then(Instance::new);
    let lock = Rc::new(Lock {
        instance: instance.clone(),
        preview,
        user,
        entries: RefCell::new(Vec::new()),
        messages: RefCell::new(Vec::new()),
        go_stacks: RefCell::new(Vec::new()),
        caps: RefCell::new(Vec::new()),
        clocks: RefCell::new(Vec::new()),
        media: RefCell::new(Vec::new()),
        media_player: RefCell::new(None),
        batteries: RefCell::new(Vec::new()),
        busy: Cell::new(false),
        failures: Cell::new(0),
        theme_cfg,
        cfg: lock_cfg,
    });

    let Some(instance) = instance else {
        // Preview: one fullscreen window; Esc or closing it quits.
        let monitor =
            gdk::Display::default().and_then(|d| d.monitors().item(0)).and_then(|m| m.downcast::<gdk::Monitor>().ok());
        let window = lock.build_window(app, true, monitor.as_ref());
        let keys = gtk::EventControllerKey::new();
        let w = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);
        // Everything else only holds weak references: keep the state alive
        // for as long as the preview window exists.
        let keep = lock.clone();
        window.connect_destroy(move |_| {
            let _ = &keep;
        });
        window.fullscreen();
        window.present();
        lock.start_clock();
        lock.start_extras();
        return;
    };

    let hold = app.hold();
    let hold = Rc::new(RefCell::new(Some(hold)));
    {
        let app = app.clone();
        let hold = hold.clone();
        instance.connect_failed(move |_| {
            error!("could not lock the session (another lock screen is active?)");
            hold.borrow_mut().take();
            app.quit();
        });
    }
    instance.connect_locked(|_| info!("session locked"));
    {
        let app = app.clone();
        instance.connect_unlocked(move |_| {
            info!("session unlocked");
            hold.borrow_mut().take();
            app.quit();
        });
    }
    {
        let lock = lock.clone();
        let app = app.clone();
        let first = Cell::new(true);
        instance.connect_monitor(move |inst, monitor| {
            let primary = first.replace(false);
            let window = lock.build_window(&app, primary, Some(monitor));
            inst.assign_window_to_monitor(&window, monitor);
        });
    }
    if !instance.lock() {
        error!("ext-session-lock is not available");
        app.quit();
        return;
    }
    lock.start_clock();
    lock.start_extras();
}

impl Lock {
    fn build_window(
        self: &Rc<Self>,
        app: &gtk::Application,
        primary: bool,
        monitor: Option<&gdk::Monitor>,
    ) -> gtk::Window {
        let c = &self.cfg;
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.add_css_class("m3");
        window.add_css_class("m3-lock");
        if self.preview {
            window.set_title(Some("Lock screen preview"));
        }

        // Background: wallpaper / image (optionally blurred) or the gradient.
        let overlay = gtk::Overlay::new();
        let connector = monitor.and_then(|m| m.connector()).map(|c| c.to_string());
        match look::background_path(c, &self.theme_cfg, connector.as_deref(), |p| p.is_file()) {
            Some(path) => {
                let pic = gtk::Picture::for_filename(&path);
                pic.set_content_fit(gtk::ContentFit::Cover);
                pic.set_can_shrink(true);
                pic.add_css_class("m3-lock-bg");
                overlay.set_child(Some(&pic));
            }
            None => {
                let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
                bg.add_css_class("m3-lock-fallback");
                overlay.set_child(Some(&bg));
            }
        }
        overlay.set_overflow(gtk::Overflow::Hidden);
        let scrim = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrim.add_css_class("m3-lock-scrim");
        overlay.add_overlay(&scrim);

        // Clock.
        let left = c.clock_position == LockClockPosition::Left;
        let clock = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clock.add_css_class("m3-lock-clock");
        let time = gtk::Label::new(None);
        time.add_css_class("m3-lock-time");
        let date = gtk::Label::new(None);
        date.add_css_class("m3-title-large-emphasized");
        date.add_css_class("m3-lock-date");
        date.set_visible(c.show_date);
        if left {
            time.set_xalign(0.0);
            date.set_xalign(0.0);
        }
        clock.append(&time);
        clock.append(&date);
        self.clocks.borrow_mut().push((time, date));

        // The card, owner message and media (primary output only).
        let column = gtk::Box::new(gtk::Orientation::Vertical, 16);
        column.add_css_class("m3-lock-column");
        if primary {
            column.append(&self.build_card(&window));
            let msg = c.message_text();
            if !msg.is_empty() {
                let l = gtk::Label::new(Some(&msg));
                l.set_use_markup(false);
                l.set_wrap(true);
                l.set_max_width_chars(48);
                l.set_justify(gtk::Justification::Center);
                l.add_css_class("m3-lock-owner-message");
                l.add_css_class("m3-body-large");
                column.append(&l);
            }
            if c.show_media {
                column.append(&self.build_media());
            }
        }

        match c.clock_position {
            LockClockPosition::Center => {
                let all = gtk::Box::new(gtk::Orientation::Vertical, 8);
                all.set_halign(gtk::Align::Center);
                all.set_valign(gtk::Align::Center);
                clock.set_margin_bottom(32);
                all.append(&clock);
                all.append(&column);
                column.set_halign(gtk::Align::Center);
                overlay.add_overlay(&all);
            }
            LockClockPosition::Top => {
                // Clock at the top; the card centred in the space below it.
                let all = gtk::Box::new(gtk::Orientation::Vertical, 0);
                clock.set_halign(gtk::Align::Center);
                clock.set_margin_top(64);
                column.set_halign(gtk::Align::Center);
                column.set_valign(gtk::Align::Center);
                column.set_vexpand(true);
                column.set_margin_bottom(64);
                all.append(&clock);
                all.append(&column);
                overlay.add_overlay(&all);
            }
            LockClockPosition::Left => {
                clock.set_halign(gtk::Align::Start);
                clock.set_valign(gtk::Align::Center);
                clock.set_margin_start(112);
                overlay.add_overlay(&clock);
                column.set_halign(gtk::Align::End);
                column.set_valign(gtk::Align::Center);
                column.set_margin_end(112);
                overlay.add_overlay(&column);
            }
        }

        if c.show_battery {
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chip.add_css_class("m3-lock-battery");
            chip.set_halign(gtk::Align::End);
            chip.set_valign(gtk::Align::Start);
            chip.set_margin_top(24);
            chip.set_margin_end(24);
            let icon = gtk::Label::new(None);
            icon.add_css_class("m3-icon");
            icon.add_css_class("small");
            icon.add_css_class("filled");
            let pct = gtk::Label::new(None);
            pct.add_css_class("m3-label-large-emphasized");
            chip.append(&icon);
            chip.append(&pct);
            chip.set_visible(false);
            overlay.add_overlay(&chip);
            self.batteries.borrow_mut().push((chip, icon, pct));
        }

        if self.preview {
            let hint = gtk::Label::new(Some("Preview — press Esc to close"));
            hint.add_css_class("m3-lock-preview-hint");
            hint.add_css_class("m3-label-large");
            hint.set_halign(gtk::Align::Center);
            hint.set_valign(gtk::Align::End);
            hint.set_margin_bottom(28);
            overlay.add_overlay(&hint);
        }
        window.set_child(Some(&overlay));
        window
    }

    /// Avatar, name, password row, caps-lock hint and status message.
    fn build_card(self: &Rc<Self>, window: &gtk::Window) -> gtk::Box {
        let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
        card.add_css_class("m3-lock-card");
        card.set_halign(gtk::Align::Center);
        if self.cfg.show_user {
            let initial = self.user.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
            let face = std::path::Path::new(&self.user.home).join(".face");
            card.append(&avatar::build(&face, &initial, self.cfg.avatar_shape));
            let name = gtk::Label::new(Some(&self.user.display_name));
            name.add_css_class("m3-headline-small-emphasized");
            card.append(&name);
        }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::PasswordEntry::new();
        entry.set_show_peek_icon(true);
        entry.set_property("placeholder-text", if self.preview { "Password (preview)" } else { "Password" });
        entry.add_css_class("m3-lock-entry");
        entry.set_hexpand(true);
        entry.update_property(&[gtk::accessible::Property::Label("Password")]);
        let go = gtk::Button::new();
        let go_stack = gtk::Stack::new();
        go_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        go_stack.add_named(
            &{
                let l = gtk::Label::new(Some("arrow_forward"));
                l.add_css_class("m3-icon");
                l
            },
            Some("icon"),
        );
        go_stack.add_named(m3_widgets::LoadingIndicator::new(32).widget(), Some("busy"));
        go.set_child(Some(&go_stack));
        self.go_stacks.borrow_mut().push(go_stack);
        go.add_css_class("fab");
        go.add_css_class("m3-lock-go");
        go.set_tooltip_text(Some("Unlock"));
        go.update_property(&[gtk::accessible::Property::Label("Unlock")]);
        row.append(&entry);
        row.append(&go);
        card.append(&row);

        let caps = gtk::Label::new(Some("Caps Lock is on"));
        caps.add_css_class("m3-lock-caps");
        caps.set_visible(false);
        card.append(&caps);
        let message = gtk::Label::new(None);
        message.add_css_class("m3-lock-message");
        message.set_wrap(true);
        message.set_visible(false);
        message.set_accessible_role(gtk::AccessibleRole::Alert);
        card.append(&message);

        if self.preview {
            // Preview never authenticates: nothing to type into.
            entry.set_sensitive(false);
            go.set_sensitive(false);
        } else {
            let me = Rc::downgrade(self);
            entry.connect_activate(move |_| {
                if let Some(l) = me.upgrade() {
                    l.submit();
                }
            });
            let me = Rc::downgrade(self);
            go.connect_clicked(move |_| {
                if let Some(l) = me.upgrade() {
                    l.submit();
                }
            });
        }
        // Caps Lock indicator.
        if let Some(kbd) = gdk::Display::default().and_then(|d| d.default_seat()).and_then(|s| s.keyboard()) {
            caps.set_visible(kbd.is_caps_locked());
            let c = caps.clone();
            kbd.connect_caps_lock_state_notify(move |k| c.set_visible(k.is_caps_locked()));
        }
        self.entries.borrow_mut().push(entry.clone());
        self.messages.borrow_mut().push(message);
        self.caps.borrow_mut().push(caps);
        if !self.preview {
            window.connect_map(move |_| {
                entry.grab_focus();
            });
        }
        card
    }

    /// The now-playing pill (hidden until a player is playing or paused).
    fn build_media(self: &Rc<Self>) -> gtk::Box {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        root.add_css_class("m3-lock-media");
        root.set_halign(gtk::Align::Center);
        root.set_visible(false);
        let art = gtk::Stack::new();
        // A gtk::Image scales its file to a fixed pixel size (a Picture
        // would ask for the cover's full size).
        let picture = gtk::Image::new();
        picture.set_pixel_size(48);
        picture.set_overflow(gtk::Overflow::Hidden);
        picture.add_css_class("m3-lock-media-art");
        art.set_valign(gtk::Align::Center);
        let note = m3_widgets::ShapeIcon::new(m3_widgets::Shape::Cookie(9), "music_note", 48, true);
        note.widget().add_css_class("secondary");
        art.add_named(note.widget(), Some("icon"));
        art.add_named(&picture, Some("art"));
        root.append(&art);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_valign(gtk::Align::Center);
        text.set_hexpand(true);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(28);
        title.add_css_class("m3-title-small-emphasized");
        let artist = gtk::Label::new(None);
        artist.set_xalign(0.0);
        artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        artist.set_max_width_chars(28);
        artist.add_css_class("m3-body-medium");
        artist.add_css_class("m3-secondary-text");
        text.append(&title);
        text.append(&artist);
        root.append(&text);
        let play = gtk::Button::new();
        let play_icon = gtk::Label::new(Some("play_arrow"));
        play_icon.add_css_class("m3-icon");
        play_icon.add_css_class("filled");
        play.set_child(Some(&play_icon));
        play.add_css_class("icon");
        play.add_css_class("m3-lock-media-play");
        play.set_tooltip_text(Some("Play or pause"));
        play.update_property(&[gtk::accessible::Property::Label("Play or pause")]);
        let next = gtk::Button::new();
        let next_icon = gtk::Label::new(Some("skip_next"));
        next_icon.add_css_class("m3-icon");
        next_icon.add_css_class("filled");
        next.set_child(Some(&next_icon));
        next.add_css_class("icon");
        next.set_tooltip_text(Some("Next track"));
        next.update_property(&[gtk::accessible::Property::Label("Next track")]);
        play.set_valign(gtk::Align::Center);
        next.set_valign(gtk::Align::Center);
        root.append(&play);
        root.append(&next);
        for (b, method) in [(&play, "PlayPause"), (&next, "Next")] {
            let me = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                let Some(l) = me.upgrade() else { return };
                let Some(player) = l.media_player.borrow().clone() else { return };
                let me = Rc::downgrade(&l);
                glib::spawn_future_local(async move {
                    if let Ok(conn) = gio::bus_get_future(gio::BusType::Session).await {
                        m3_widgets::mpris::control(&conn, &player, method).await;
                    }
                    glib::timeout_future(Duration::from_millis(300)).await;
                    if let Some(l) = me.upgrade() {
                        l.refresh_media().await;
                    }
                });
            });
        }
        self.media.borrow_mut().push(MediaView { root: root.clone(), art, picture, title, artist, play_icon });
        root
    }

    async fn refresh_media(&self) {
        let np = match gio::bus_get_future(gio::BusType::Session).await {
            Ok(conn) => m3_widgets::mpris::query(&conn).await,
            Err(_) => None,
        };
        *self.media_player.borrow_mut() = np.as_ref().map(|n| n.bus_name.clone());
        for v in self.media.borrow().iter() {
            match &np {
                Some(n) => {
                    v.root.set_visible(true);
                    v.title.set_text(if n.title.is_empty() { "Unknown title" } else { &n.title });
                    v.artist.set_text(&n.artist);
                    v.artist.set_visible(!n.artist.is_empty());
                    v.play_icon.set_text(if n.playing { "pause" } else { "play_arrow" });
                    match &n.art {
                        Some(p) => {
                            v.picture.set_from_file(Some(p));
                            v.art.set_visible_child_name("art");
                        }
                        None => v.art.set_visible_child_name("icon"),
                    }
                }
                None => v.root.set_visible(false),
            }
        }
    }

    fn refresh_battery(&self) {
        let b = battery::read();
        for (chip, icon, pct) in self.batteries.borrow().iter() {
            match b {
                Some(b) => {
                    chip.set_visible(true);
                    icon.set_text(battery::icon(b));
                    pct.set_text(&format!("{}%", b.percent));
                    chip.update_property(&[gtk::accessible::Property::Label(&format!("Battery {}%", b.percent))]);
                }
                None => chip.set_visible(false),
            }
        }
    }

    /// Media and battery refreshes (only for what is enabled).
    fn start_extras(self: &Rc<Self>) {
        if self.cfg.show_battery {
            self.refresh_battery();
            let me = Rc::downgrade(self);
            glib::timeout_add_seconds_local(30, move || match me.upgrade() {
                Some(l) => {
                    l.refresh_battery();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
        }
        if self.cfg.show_media {
            let me = Rc::downgrade(self);
            glib::spawn_future_local(async move {
                loop {
                    let Some(l) = me.upgrade() else { break };
                    l.refresh_media().await;
                    drop(l);
                    glib::timeout_future(Duration::from_secs(2)).await;
                }
            });
        }
    }

    fn start_clock(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        let tick = move || {
            let Some(l) = me.upgrade() else { return glib::ControlFlow::Break };
            let now = chrono::Local::now();
            let (tp, dp) = (l.cfg.time_pattern(), l.cfg.date_format.pattern());
            for (t, d) in l.clocks.borrow().iter() {
                t.set_text(&now.format(tp).to_string());
                d.set_text(&now.format(dp).to_string());
            }
            glib::ControlFlow::Continue
        };
        tick();
        glib::timeout_add_seconds_local(1, tick);
    }

    fn set_busy_ui(&self, busy: bool) {
        for s in self.go_stacks.borrow().iter() {
            s.set_visible_child_name(if busy { "busy" } else { "icon" });
        }
    }

    fn show_message(&self, text: &str, error: bool) {
        for m in self.messages.borrow().iter() {
            m.set_text(text);
            m.set_visible(!text.is_empty());
            if error { m.add_css_class("error") } else { m.remove_css_class("error") }
        }
    }

    fn submit(self: &Rc<Self>) {
        if self.busy.get() || self.preview {
            return;
        }
        let Some(entry) = self.entries.borrow().first().cloned() else { return };
        let password = Zeroizing::new(entry.text().to_string());
        entry.set_text("");
        if password.is_empty() {
            return;
        }
        self.busy.set(true);
        entry.set_sensitive(false);
        entry.remove_css_class("error");
        self.show_message("Checking…", false);
        self.set_busy_ui(true);

        let user = self.user.name.clone();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let r = auth::authenticate(&user, password);
            let _ = tx.send_blocking(r);
        });
        let me = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = rx.recv().await;
            let Some(l) = me.upgrade() else { return };
            l.set_busy_ui(false);
            match result {
                Ok(auth::AuthResult::Success) => {
                    l.show_message("", false);
                    if let Some(i) = &l.instance {
                        i.unlock();
                    }
                }
                Ok(auth::AuthResult::Failure(msg)) => {
                    let n = l.failures.get() + 1;
                    l.failures.set(n);
                    warn!("authentication failed (attempt {n})");
                    l.show_message(&msg, true);
                    entry.add_css_class("error");
                    // Back off after repeated failures (PAM may add its own delay).
                    let delay = if n >= 5 { Duration::from_secs(5) } else { Duration::from_millis(0) };
                    let e2 = entry.clone();
                    let me2 = Rc::downgrade(&l);
                    glib::timeout_add_local_once(delay, move || {
                        e2.set_sensitive(true);
                        e2.grab_focus();
                        if let Some(l) = me2.upgrade() {
                            l.busy.set(false);
                        }
                    });
                }
                Err(_) => {
                    l.show_message("Authentication failed unexpectedly.", true);
                    entry.set_sensitive(true);
                    l.busy.set(false);
                }
            }
        });
    }
}
