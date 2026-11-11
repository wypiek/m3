//! The Music window: library views, the floating now-playing bar and the
//! full now-playing view, wired to the player, the queue and MPRIS.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use m3_widgets::{AppStyle, CircularWavyProgress, Shape, ShapeIcon, SpringSpec, WavyProgress, WavySlider};

use crate::art::{self, ShapedCover, Thumbs};
use crate::library::{self, Album, Artist, ScanEvent, Track, format_time};
use crate::mpris::{self, Command, Meta, Mpris, Update};
use crate::player::{Player, PlayerEvent};
use crate::queue::{Queue, Repeat};
use crate::state::{self, Saved};

const VIEWS: [(&str, &str, &str); 3] =
    [("songs", "Songs", "music_note"), ("albums", "Albums", "album"), ("artists", "Artists", "person")];

fn icon(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("m3-icon");
    l.set_accessible_role(gtk::AccessibleRole::Img);
    l
}

fn icon_button(name: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&icon(name)));
    b.add_css_class("icon");
    b.set_tooltip_text(Some(tip));
    b.update_property(&[gtk::accessible::Property::Label(tip)]);
    b
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A square cover of a fixed size (a `gtk::Image` scales its paintable to
/// `pixel_size`, unlike `gtk::Picture`, whose natural size is the image's).
fn cover_image(size: i32, class: &str) -> gtk::Image {
    let i = gtk::Image::new();
    i.set_pixel_size(size);
    i.set_size_request(size, size);
    i.set_overflow(gtk::Overflow::Hidden);
    i.add_css_class(class);
    i.set_valign(gtk::Align::Center);
    i.set_halign(gtk::Align::Center);
    i
}

fn set_cover(img: &gtk::Image, tex: Option<&gdk::Texture>) {
    match tex {
        Some(t) => {
            img.set_paintable(Some(t));
            img.remove_css_class("no-art");
        }
        None => {
            img.clear();
            img.add_css_class("no-art");
        }
    }
}

/// Show the cover at `path` in `img` once its thumbnail is ready; ignores the
/// result if the image has been rebound to something else meanwhile.
fn show_cover(thumbs: &Thumbs, img: &gtk::Image, path: Option<&PathBuf>, size: i32) {
    let Some(path) = path else {
        img.set_widget_name("");
        set_cover(img, None);
        return;
    };
    let key = format!("{size}:{}", path.display());
    img.set_widget_name(&key);
    let weak = img.downgrade();
    thumbs.request(path, size, move |tex| {
        if let Some(img) = weak.upgrade()
            && img.widget_name() == key
        {
            set_cover(&img, tex);
        }
    });
}

fn index_of(obj: &glib::Object) -> Option<usize> {
    obj.downcast_ref::<glib::BoxedAnyObject>().map(|b| *b.borrow::<usize>())
}

/// A window action handler.
type Handler = Box<dyn Fn(&Rc<Music>)>;

/// Detail page contents (an album or an artist).
enum Detail {
    Album(usize),
    Artist(usize),
}

pub struct Music {
    app: gtk::Application,
    window: gtk::ApplicationWindow,
    style: Rc<AppStyle>,
    art_css: gtk::CssProvider,
    root: PathBuf,
    thumbs: Rc<Thumbs>,
    seeds: RefCell<HashMap<PathBuf, Option<u32>>>,

    tracks: RefCell<Rc<Vec<Track>>>,
    externals: RefCell<Vec<Track>>,
    albums: RefCell<Vec<Album>>,
    artists: RefCell<Vec<Artist>>,
    queue: RefCell<Queue>,
    player: RefCell<Option<Player>>,
    mpris: Mpris,
    loaded: Cell<bool>,
    errors_in_row: Cell<usize>,
    scanning: Cell<bool>,
    updating: Cell<bool>,
    last_position: Cell<i64>,
    /// The track currently marked as playing in the song list.
    marked: Cell<Option<usize>>,
    /// Saved queue waiting for the library to load.
    restore: RefCell<Option<Saved>>,
    /// Position to seek to once a restored track has loaded.
    pending_seek: Cell<Option<u64>>,
    save_timer: Rc<RefCell<Option<glib::SourceId>>>,
    this: RefCell<std::rc::Weak<Music>>,

    // Library
    content: gtk::Stack,
    views: gtk::Stack,
    nav: Vec<gtk::ToggleButton>,
    search: gtk::SearchEntry,
    song_store: gio::ListStore,
    song_filter: gtk::CustomFilter,
    song_model: gtk::FilterListModel,
    album_store: gio::ListStore,
    album_filter: gtk::CustomFilter,
    album_model: gtk::FilterListModel,
    artist_store: gio::ListStore,
    artist_filter: gtk::CustomFilter,
    artist_model: gtk::FilterListModel,
    scan_busy: gtk::Stack,
    scan_label: gtk::Label,
    no_results: gtk::Label,
    empty_label: gtk::Label,
    detail: RefCell<Option<Detail>>,
    detail_tracks: RefCell<Vec<usize>>,
    detail_cover: gtk::Image,
    detail_title: gtk::Label,
    detail_sub: gtk::Label,
    detail_list: gtk::ListBox,
    detail_back_to: RefCell<String>,

    // Now-playing bar
    bar: gtk::Revealer,
    bar_cover: gtk::Image,
    bar_title: gtk::Label,
    bar_artist: gtk::Label,
    bar_play: gtk::Button,
    bar_progress: WavyProgress,

    // Now-playing view
    np: gtk::Revealer,
    np_cover: ShapedCover,
    np_title: gtk::Label,
    np_artist: gtk::Label,
    np_slider: WavySlider,
    np_elapsed: gtk::Label,
    np_remaining: gtk::Label,
    np_play: ShapeIcon,
    np_prev: gtk::Button,
    np_next: gtk::Button,
    np_shuffle: gtk::ToggleButton,
    np_repeat: gtk::Button,
    np_volume: gtk::Scale,

    snackbar: gtk::Revealer,
    snack_label: gtk::Label,
    snack_timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Music {
    pub fn new(app: &gtk::Application, style: Rc<AppStyle>) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Music")
            .default_width(1120)
            .default_height(780)
            .build();
        window.add_css_class("m3");
        window.add_css_class("m3-music-window");

        let art_css = gtk::CssProvider::new();
        if let Some(d) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &art_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        }

        // ---- header: title, navigation (connected button group), search
        let header = gtk::HeaderBar::new();
        let title = label("Music", &["m3-title-large-emphasized", "m3-music-title"]);
        header.pack_start(&title);
        let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        nav_box.add_css_class("m3-button-group");
        let mut nav: Vec<gtk::ToggleButton> = Vec::new();
        for (_, name, ic) in VIEWS {
            let b = gtk::ToggleButton::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let i = icon(ic);
            i.add_css_class("small");
            row.append(&i);
            row.append(&gtk::Label::new(Some(name)));
            b.set_child(Some(&row));
            if let Some(first) = nav.first() {
                b.set_group(Some(first));
            }
            nav_box.append(&b);
            nav.push(b);
        }
        header.set_title_widget(Some(&nav_box));
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search music"));
        search.add_css_class("m3-search");
        search.set_width_chars(22);
        let scan_busy = gtk::Stack::new();
        let rescan = icon_button("refresh", "Rescan the music folder (Ctrl+R)");
        rescan.set_action_name(Some("win.rescan"));
        scan_busy.add_named(&rescan, Some("idle"));
        let busy = CircularWavyProgress::new(28, None);
        busy.widget().set_tooltip_text(Some("Scanning…"));
        busy.widget().set_valign(gtk::Align::Center);
        busy.widget().set_halign(gtk::Align::Center);
        scan_busy.add_named(busy.widget(), Some("busy"));
        header.pack_end(&scan_busy);
        header.pack_end(&search);
        window.set_titlebar(Some(&header));

        // ---- library views
        let song_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let album_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let artist_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        // Filters are re-created with access to `self` below; start permissive.
        let song_filter = gtk::CustomFilter::new(|_| true);
        let album_filter = gtk::CustomFilter::new(|_| true);
        let artist_filter = gtk::CustomFilter::new(|_| true);
        let song_model = gtk::FilterListModel::new(Some(song_store.clone()), Some(song_filter.clone()));
        let album_model = gtk::FilterListModel::new(Some(album_store.clone()), Some(album_filter.clone()));
        let artist_model = gtk::FilterListModel::new(Some(artist_store.clone()), Some(artist_filter.clone()));

        let views = gtk::Stack::new();
        views.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
        views.set_transition_duration(280);
        views.set_vexpand(true);

        let content = gtk::Stack::new();
        content.set_transition_type(gtk::StackTransitionType::Crossfade);
        // "No matches" message over the filtered views.
        let no_results = label("", &["m3-title-medium", "m3-secondary-text", "m3-no-results"]);
        no_results.set_halign(gtk::Align::Center);
        no_results.set_valign(gtk::Align::Start);
        no_results.set_margin_top(64);
        no_results.set_visible(false);
        no_results.set_can_target(false);
        let library_page = gtk::Overlay::new();
        library_page.set_child(Some(&views));
        library_page.add_overlay(&no_results);
        content.add_named(&library_page, Some("library"));

        // Scanning page (first run, no cache).
        let scanning = gtk::Box::new(gtk::Orientation::Vertical, 16);
        scanning.set_valign(gtk::Align::Center);
        scanning.set_halign(gtk::Align::Center);
        let big_busy = CircularWavyProgress::new(72, None);
        big_busy.widget().set_halign(gtk::Align::Center);
        scanning.append(big_busy.widget());
        let scan_label = label("", &["m3-title-medium", "m3-secondary-text"]);
        scan_label.set_xalign(0.5);
        scanning.append(&scan_label);
        content.add_named(&scanning, Some("scanning"));

        // Empty page.
        let root = library::music_dir();
        let empty = gtk::Box::new(gtk::Orientation::Vertical, 16);
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        empty.add_css_class("m3-music-empty");
        let shape = ShapeIcon::new(Shape::Cookie(9), "library_music", 128, true);
        shape.widget().add_css_class("tertiary");
        m3_widgets::morph_on_interaction(
            shape.widget(),
            shape.shape(),
            Shape::Cookie(9),
            Shape::Sunny(8),
            Shape::Clover(4),
        );
        empty.append(shape.widget());
        let et = label("No music yet", &["m3-headline-medium-emphasized"]);
        et.set_xalign(0.5);
        empty.append(&et);
        let empty_label = label("", &["m3-body-large", "m3-secondary-text"]);
        empty_label.set_xalign(0.5);
        empty_label.set_justify(gtk::Justification::Center);
        empty_label.set_wrap(true);
        empty_label.set_max_width_chars(48);
        empty.append(&empty_label);
        let empty_rescan = gtk::Button::with_label("Rescan");
        empty_rescan.add_css_class("filled");
        empty_rescan.set_halign(gtk::Align::Center);
        empty_rescan.set_action_name(Some("win.rescan"));
        empty.append(&empty_rescan);
        content.add_named(&empty, Some("empty"));

        // Detail page (album / artist).
        let detail_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let detail_scroll = gtk::ScrolledWindow::new();
        detail_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        detail_scroll.set_vexpand(true);
        let detail_inner = gtk::Box::new(gtk::Orientation::Vertical, 24);
        detail_inner.add_css_class("m3-music-detail");
        let back = icon_button("arrow_back", "Back");
        back.set_halign(gtk::Align::Start);
        back.add_css_class("tonal");
        detail_inner.append(&back);
        let dhead = gtk::Box::new(gtk::Orientation::Horizontal, 32);
        let detail_cover = cover_image(220, "m3-detail-cover");
        detail_cover.set_valign(gtk::Align::Start);
        dhead.append(&detail_cover);
        let dtext = gtk::Box::new(gtk::Orientation::Vertical, 6);
        dtext.set_valign(gtk::Align::Center);
        let detail_title = label("", &["m3-display-small-emphasized"]);
        detail_title.set_wrap(true);
        let detail_sub = label("", &["m3-title-medium", "m3-secondary-text"]);
        detail_sub.set_wrap(true);
        dtext.append(&detail_title);
        dtext.append(&detail_sub);
        let dbuttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        dbuttons.set_margin_top(16);
        let play_all = gtk::Button::new();
        let pa = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        pa.append(&icon("play_arrow"));
        pa.append(&gtk::Label::new(Some("Play")));
        play_all.set_child(Some(&pa));
        play_all.add_css_class("filled");
        play_all.add_css_class("medium");
        let shuffle_all = gtk::Button::new();
        let sa = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        sa.append(&icon("shuffle"));
        sa.append(&gtk::Label::new(Some("Shuffle")));
        shuffle_all.set_child(Some(&sa));
        shuffle_all.add_css_class("tonal");
        shuffle_all.add_css_class("medium");
        dbuttons.append(&play_all);
        dbuttons.append(&shuffle_all);
        dtext.append(&dbuttons);
        dhead.append(&dtext);
        detail_inner.append(&dhead);
        let detail_list = gtk::ListBox::new();
        detail_list.add_css_class("m3-list");
        detail_list.set_selection_mode(gtk::SelectionMode::None);
        detail_inner.append(&detail_list);
        let clamp = gtk::Box::new(gtk::Orientation::Vertical, 0);
        clamp.append(&detail_inner);
        detail_scroll.set_child(Some(&clamp));
        detail_page.append(&detail_scroll);

        // ---- now-playing bar
        let bar_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bar_box.add_css_class("m3-np-bar");
        bar_box.add_css_class("m3-art-scheme");
        let bar_cover = cover_image(52, "m3-bar-cover");
        let bar_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        bar_text.set_hexpand(true);
        bar_text.set_valign(gtk::Align::Center);
        let bar_title = label("", &["m3-title-small-emphasized"]);
        bar_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let bar_artist = label("", &["m3-body-small", "m3-secondary-text"]);
        bar_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let bar_progress = WavyProgress::new(Some(0.0));
        bar_progress.set_style(m3_widgets::WaveStyle {
            amplitude: 2.0,
            wavelength: 24.0,
            thickness: 3.0,
            gap: 3.0,
            speed: 0.6,
        });
        bar_text.append(&bar_title);
        bar_text.append(&bar_artist);
        bar_text.append(bar_progress.widget());
        let bar_play = icon_button("play_arrow", "Play");
        bar_play.add_css_class("m3-bar-play");
        bar_play.set_valign(gtk::Align::Center);
        let bar_next = icon_button("skip_next", "Next");
        bar_next.set_valign(gtk::Align::Center);
        bar_next.set_action_name(Some("win.next"));
        let bar_open = icon_button("expand_less", "Now playing");
        bar_open.set_valign(gtk::Align::Center);
        bar_open.set_action_name(Some("win.now-playing"));
        bar_box.append(&bar_cover);
        bar_box.append(&bar_text);
        bar_box.append(&bar_play);
        bar_box.append(&bar_next);
        bar_box.append(&bar_open);
        let bar = gtk::Revealer::new();
        bar.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        bar.set_transition_duration(300);
        bar.set_child(Some(&bar_box));
        bar.set_valign(gtk::Align::End);
        bar.set_halign(gtk::Align::Center);
        bar.set_margin_bottom(16);
        // Tapping the text opens the full view.
        let open_click = gtk::GestureClick::new();
        open_click.connect_released(|g, _, _, _| {
            if let Some(w) = g.widget() {
                let _ = w.activate_action("win.now-playing", None);
            }
        });
        bar_text.add_controller(open_click);

        // ---- now-playing view
        let np_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        np_box.add_css_class("m3-now-playing");
        np_box.add_css_class("m3-art-scheme");
        let np_top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        np_top.add_css_class("m3-np-top");
        let np_close = icon_button("expand_more", "Close (Esc)");
        np_close.add_css_class("tonal");
        np_close.set_action_name(Some("win.now-playing"));
        np_top.append(&np_close);
        let np_heading = label("Now playing", &["m3-title-medium-emphasized"]);
        np_heading.set_hexpand(true);
        np_heading.set_xalign(0.5);
        np_top.append(&np_heading);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_size_request(48, -1);
        np_top.append(&spacer);
        np_box.append(&np_top);
        let np_body = gtk::Box::new(gtk::Orientation::Horizontal, 56);
        np_body.set_valign(gtk::Align::Center);
        np_body.set_halign(gtk::Align::Center);
        np_body.set_vexpand(true);
        let np_cover = ShapedCover::new(360, Shape::Cookie(9));
        np_cover.widget().set_valign(gtk::Align::Center);
        np_body.append(np_cover.widget());
        let np_col = gtk::Box::new(gtk::Orientation::Vertical, 10);
        np_col.set_valign(gtk::Align::Center);
        np_col.set_size_request(440, -1);
        let np_title = label("", &["m3-display-small-emphasized", "m3-np-title"]);
        np_title.set_wrap(true);
        np_title.set_lines(2);
        np_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let np_artist = label("", &["m3-title-large", "m3-np-artist"]);
        np_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        np_col.append(&np_title);
        np_col.append(&np_artist);
        let np_slider = WavySlider::new();
        np_slider.widget().set_margin_top(20);
        np_slider.widget().update_property(&[gtk::accessible::Property::Label("Position")]);
        np_col.append(np_slider.widget());
        let times = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let np_elapsed = label("0:00", &["m3-label-large", "m3-np-time"]);
        np_elapsed.set_hexpand(true);
        let np_remaining = label("0:00", &["m3-label-large", "m3-np-time"]);
        times.append(&np_elapsed);
        times.append(&np_remaining);
        np_col.append(&times);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        controls.set_halign(gtk::Align::Center);
        controls.set_margin_top(12);
        let np_shuffle = gtk::ToggleButton::new();
        np_shuffle.set_child(Some(&icon("shuffle")));
        np_shuffle.add_css_class("icon");
        np_shuffle.add_css_class("square");
        np_shuffle.set_tooltip_text(Some("Shuffle"));
        np_shuffle.update_property(&[gtk::accessible::Property::Label("Shuffle")]);
        np_shuffle.set_valign(gtk::Align::Center);
        let np_prev = icon_button("skip_previous", "Previous");
        np_prev.add_css_class("medium");
        np_prev.add_css_class("tonal");
        np_prev.set_valign(gtk::Align::Center);
        np_prev.set_action_name(Some("win.previous"));
        let play_btn = gtk::Button::new();
        play_btn.add_css_class("m3-np-play");
        play_btn.set_tooltip_text(Some("Play/pause (Space)"));
        play_btn.update_property(&[gtk::accessible::Property::Label("Play or pause")]);
        let np_play = ShapeIcon::new(Shape::Circle, "play_arrow", 96, true);
        np_play.label().add_css_class("large");
        play_btn.set_child(Some(np_play.widget()));
        play_btn.set_action_name(Some("win.play-pause"));
        let np_next = icon_button("skip_next", "Next");
        np_next.add_css_class("medium");
        np_next.add_css_class("tonal");
        np_next.set_valign(gtk::Align::Center);
        np_next.set_action_name(Some("win.next"));
        let np_repeat = icon_button("repeat", "Repeat: off");
        np_repeat.add_css_class("square");
        np_repeat.set_valign(gtk::Align::Center);
        controls.append(&np_shuffle);
        controls.append(&np_prev);
        controls.append(&play_btn);
        controls.append(&np_next);
        controls.append(&np_repeat);
        np_col.append(&controls);
        let vol = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        vol.set_margin_top(16);
        let vol_icon = icon("volume_up");
        vol_icon.add_css_class("m3-secondary-text");
        let np_volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.02);
        np_volume.set_draw_value(false);
        np_volume.set_hexpand(true);
        np_volume.set_value(1.0);
        np_volume.update_property(&[gtk::accessible::Property::Label("Volume")]);
        vol.append(&vol_icon);
        vol.append(&np_volume);
        np_col.append(&vol);
        np_body.append(&np_col);
        np_box.append(&np_body);
        let np = gtk::Revealer::new();
        np.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        np.set_transition_duration(380);
        np.set_child(Some(&np_box));
        np.set_can_target(false);

        // ---- snackbar
        let snack = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        snack.add_css_class("m3-snackbar");
        let snack_label = gtk::Label::new(None);
        snack_label.set_wrap(true);
        snack_label.set_max_width_chars(60);
        snack.append(&snack_label);
        let snackbar = gtk::Revealer::new();
        snackbar.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        snackbar.set_child(Some(&snack));
        snackbar.set_halign(gtk::Align::Center);
        snackbar.set_valign(gtk::Align::End);
        snackbar.set_margin_bottom(104);
        snackbar.set_can_target(false);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&content));
        overlay.add_overlay(&bar);
        overlay.add_overlay(&np);
        overlay.add_overlay(&snackbar);
        window.set_child(Some(&overlay));

        let (mpris_handle, mpris_rx) = mpris::start();
        let seed = glib::monotonic_time() as u64;
        let me = Rc::new(Self {
            app: app.clone(),
            window,
            style,
            art_css,
            root,
            thumbs: Thumbs::new(),
            seeds: RefCell::new(HashMap::new()),
            tracks: RefCell::new(Rc::new(Vec::new())),
            externals: RefCell::new(Vec::new()),
            albums: RefCell::new(Vec::new()),
            artists: RefCell::new(Vec::new()),
            queue: RefCell::new(Queue::new(seed)),
            player: RefCell::new(None),
            mpris: mpris_handle,
            loaded: Cell::new(false),
            errors_in_row: Cell::new(0),
            scanning: Cell::new(false),
            updating: Cell::new(false),
            last_position: Cell::new(-1),
            marked: Cell::new(None),
            restore: RefCell::new(None),
            pending_seek: Cell::new(None),
            save_timer: Rc::new(RefCell::new(None)),
            this: RefCell::new(std::rc::Weak::new()),
            content,
            views,
            nav,
            search,
            song_store,
            song_filter,
            song_model,
            album_store,
            album_filter,
            album_model,
            artist_store,
            artist_filter,
            artist_model,
            scan_busy,
            scan_label,
            no_results,
            empty_label,
            detail: RefCell::new(None),
            detail_tracks: RefCell::new(Vec::new()),
            detail_cover,
            detail_title,
            detail_sub,
            detail_list,
            detail_back_to: RefCell::new("albums".into()),
            bar,
            bar_cover,
            bar_title,
            bar_artist,
            bar_play,
            bar_progress,
            np,
            np_cover,
            np_title,
            np_artist,
            np_slider,
            np_elapsed,
            np_remaining,
            np_play,
            np_prev,
            np_next,
            np_shuffle,
            np_repeat,
            np_volume,
            snackbar,
            snack_label,
            snack_timer: Rc::new(RefCell::new(None)),
        });
        me.empty_label.set_text(&format!(
            "Music plays the audio files in {}. Add some there (subfolders are fine), then press Rescan.",
            me.root.display()
        ));
        me.build_views(&detail_page, &back, &play_all, &shuffle_all);
        me.connect(mpris_rx);
        *me.this.borrow_mut() = Rc::downgrade(&me);
        me.init_player();
        me.apply_saved(state::load_from(&state::path()));
        me.load_library();
        me.publish_state();
        me
    }

    pub fn window(&self) -> &gtk::ApplicationWindow {
        &self.window
    }

    // ------------------------------------------------------------ views

    fn build_views(
        self: &Rc<Self>,
        detail_page: &gtk::Box,
        back: &gtk::Button,
        play_all: &gtk::Button,
        shuffle: &gtk::Button,
    ) {
        let weak = Rc::downgrade(self);

        // Filters with access to the library.
        let w = weak.clone();
        self.song_filter.set_filter_func(move |o| {
            let Some(me) = w.upgrade() else { return true };
            let q = me.search.text();
            index_of(o).and_then(|i| me.tracks.borrow().get(i).map(|t| t.matches(&q))).unwrap_or(false)
        });
        let w = weak.clone();
        self.album_filter.set_filter_func(move |o| {
            let Some(me) = w.upgrade() else { return true };
            let q = me.search.text().to_lowercase();
            index_of(o)
                .and_then(|i| {
                    let tracks = me.tracks.borrow();
                    me.albums.borrow().get(i).map(|a| {
                        q.is_empty()
                            || a.title.to_lowercase().contains(&q)
                            || a.artist.to_lowercase().contains(&q)
                            || a.tracks.iter().any(|&t| tracks.get(t).is_some_and(|t| t.matches(&q)))
                    })
                })
                .unwrap_or(false)
        });
        let w = weak.clone();
        self.artist_filter.set_filter_func(move |o| {
            let Some(me) = w.upgrade() else { return true };
            let q = me.search.text().to_lowercase();
            index_of(o)
                .and_then(|i| {
                    let tracks = me.tracks.borrow();
                    me.artists.borrow().get(i).map(|a| {
                        q.is_empty()
                            || a.name.to_lowercase().contains(&q)
                            || a.tracks.iter().any(|&t| tracks.get(t).is_some_and(|t| t.matches(&q)))
                    })
                })
                .unwrap_or(false)
        });

        // Songs.
        let songs =
            gtk::ListView::new(Some(gtk::NoSelection::new(Some(self.song_model.clone()))), Some(self.song_factory()));
        songs.add_css_class("m3-music-list");
        songs.set_single_click_activate(true);
        let w = weak.clone();
        songs.connect_activate(move |_, pos| {
            if let Some(me) = w.upgrade() {
                let ids = me.model_indices(&me.song_model);
                me.play_list(ids, pos as usize, false);
            }
        });
        self.views.add_named(&scrolled(&songs, "m3-music-scroll"), Some("songs"));

        // Albums.
        let albums =
            gtk::GridView::new(Some(gtk::NoSelection::new(Some(self.album_model.clone()))), Some(self.album_factory()));
        albums.add_css_class("m3-album-grid");
        albums.set_max_columns(8);
        albums.set_min_columns(2);
        albums.set_single_click_activate(true);
        let w = weak.clone();
        albums.connect_activate(move |_, pos| {
            if let Some(me) = w.upgrade()
                && let Some(i) = me.album_model.item(pos).as_ref().and_then(index_of)
            {
                me.show_detail(Detail::Album(i), "albums");
            }
        });
        self.views.add_named(&scrolled(&albums, "m3-music-scroll"), Some("albums"));

        // Artists.
        let artists = gtk::ListView::new(
            Some(gtk::NoSelection::new(Some(self.artist_model.clone()))),
            Some(self.artist_factory()),
        );
        artists.add_css_class("m3-music-list");
        artists.set_single_click_activate(true);
        let w = weak.clone();
        artists.connect_activate(move |_, pos| {
            if let Some(me) = w.upgrade()
                && let Some(i) = me.artist_model.item(pos).as_ref().and_then(index_of)
            {
                me.show_detail(Detail::Artist(i), "artists");
            }
        });
        self.views.add_named(&scrolled(&artists, "m3-music-scroll"), Some("artists"));
        self.views.add_named(detail_page, Some("detail"));

        for (i, b) in self.nav.iter().enumerate() {
            let w = weak.clone();
            b.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(me) = w.upgrade()
                {
                    me.views.set_visible_child_name(VIEWS[i].0);
                }
            });
        }
        if let Some(b) = self.nav.first() {
            b.set_active(true);
        }

        let w = weak.clone();
        self.search.connect_search_changed(move |_| {
            if let Some(me) = w.upgrade() {
                me.song_filter.changed(gtk::FilterChange::Different);
                me.album_filter.changed(gtk::FilterChange::Different);
                me.artist_filter.changed(gtk::FilterChange::Different);
                if me.views.visible_child_name().as_deref() == Some("detail") {
                    let to = me.detail_back_to.borrow().clone();
                    me.views.set_visible_child_name(&to);
                }
                me.update_no_results();
            }
        });
        // Esc in the search field clears it.
        self.search.connect_stop_search(|s| s.set_text(""));
        let w = weak.clone();
        self.views.connect_visible_child_name_notify(move |_| {
            if let Some(me) = w.upgrade() {
                me.update_no_results();
            }
        });

        let w = weak.clone();
        back.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                let to = me.detail_back_to.borrow().clone();
                me.views.set_visible_child_name(&to);
            }
        });
        let w = weak.clone();
        play_all.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                let ids = me.detail_tracks.borrow().clone();
                me.play_list(ids, 0, false);
            }
        });
        let w = weak.clone();
        shuffle.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                let ids = me.detail_tracks.borrow().clone();
                me.play_list(ids, 0, true);
            }
        });
        let w = weak;
        self.detail_list.connect_row_activated(move |_, row| {
            if let Some(me) = w.upgrade() {
                let ids = me.detail_tracks.borrow().clone();
                me.play_list(ids, row.index().max(0) as usize, false);
            }
        });
    }

    fn song_factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            row.add_css_class("m3-song-row");
            let cover = cover_image(44, "m3-row-cover");
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            let title = label("", &["m3-body-large", "m3-song-title"]);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let sub = label("", &["m3-body-medium", "m3-secondary-text"]);
            sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&title);
            text.append(&sub);
            let playing = icon("graphic_eq");
            playing.add_css_class("m3-playing-icon");
            let dur = label("", &["m3-label-large", "m3-secondary-text", "m3-np-time"]);
            row.append(&cover);
            row.append(&text);
            row.append(&playing);
            row.append(&dur);
            item.set_child(Some(&row));
        });
        let weak = Rc::downgrade(self);
        f.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(me), Some(i)) = (weak.upgrade(), item.item().as_ref().and_then(index_of)) else { return };
            let Some(row) = item.child().and_downcast::<gtk::Box>() else { return };
            let tracks = me.tracks.borrow().clone();
            let Some(t) = tracks.get(i) else { return };
            let kids = children(&row);
            if let [cover, text, playing, dur] = kids.as_slice() {
                if let Some(p) = cover.downcast_ref::<gtk::Image>() {
                    show_cover(&me.thumbs, p, t.cover.as_ref(), 88);
                }
                let tk = children(text);
                if let (Some(title), Some(sub)) = (
                    tk.first().and_then(|w| w.downcast_ref::<gtk::Label>()),
                    tk.get(1).and_then(|w| w.downcast_ref::<gtk::Label>()),
                ) {
                    title.set_text(&t.display_title());
                    sub.set_text(&format!("{} · {}", t.display_artist(), t.display_album()));
                }
                let current = me.loaded.get() && me.queue.borrow().current() == Some(i);
                playing.set_visible(current);
                if current {
                    row.add_css_class("current")
                } else {
                    row.remove_css_class("current")
                }
                if let Some(d) = dur.downcast_ref::<gtk::Label>() {
                    d.set_text(&if t.duration_ms > 0 { format_time(t.duration_ms) } else { String::new() });
                }
            }
        });
        f
    }

    fn album_factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.add_css_class("m3-album-tile");
            let cover = cover_image(176, "m3-album-cover");
            let title = label("", &["m3-title-small-emphasized"]);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_margin_top(8);
            let artist = label("", &["m3-body-medium", "m3-secondary-text"]);
            artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
            b.append(&cover);
            b.append(&title);
            b.append(&artist);
            item.set_child(Some(&b));
        });
        let weak = Rc::downgrade(self);
        f.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(me), Some(i)) = (weak.upgrade(), item.item().as_ref().and_then(index_of)) else { return };
            let Some(b) = item.child() else { return };
            let albums = me.albums.borrow();
            let Some(a) = albums.get(i) else { return };
            let kids = children(&b);
            if let [cover, title, artist] = kids.as_slice() {
                if let Some(p) = cover.downcast_ref::<gtk::Image>() {
                    show_cover(&me.thumbs, p, a.cover.as_ref(), 352);
                }
                if let Some(l) = title.downcast_ref::<gtk::Label>() {
                    l.set_text(&a.title);
                }
                if let Some(l) = artist.downcast_ref::<gtk::Label>() {
                    l.set_text(&a.artist);
                }
            }
        });
        f
    }

    fn artist_factory(self: &Rc<Self>) -> gtk::SignalListItemFactory {
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            row.add_css_class("m3-song-row");
            let avatar = gtk::Label::new(None);
            avatar.add_css_class("m3-artist-avatar");
            avatar.set_valign(gtk::Align::Center);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_valign(gtk::Align::Center);
            text.set_hexpand(true);
            let name = label("", &["m3-body-large"]);
            let sub = label("", &["m3-body-medium", "m3-secondary-text"]);
            text.append(&name);
            text.append(&sub);
            row.append(&avatar);
            row.append(&text);
            row.append(&icon("chevron_right"));
            item.set_child(Some(&row));
        });
        let weak = Rc::downgrade(self);
        f.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(me), Some(i)) = (weak.upgrade(), item.item().as_ref().and_then(index_of)) else { return };
            let Some(row) = item.child() else { return };
            let artists = me.artists.borrow();
            let Some(a) = artists.get(i) else { return };
            let kids = children(&row);
            if let [avatar, text, _] = kids.as_slice() {
                if let Some(l) = avatar.downcast_ref::<gtk::Label>() {
                    l.set_text(&a.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
                }
                let tk = children(text);
                if let (Some(n), Some(s)) = (
                    tk.first().and_then(|w| w.downcast_ref::<gtk::Label>()),
                    tk.get(1).and_then(|w| w.downcast_ref::<gtk::Label>()),
                ) {
                    n.set_text(&a.name);
                    let songs =
                        if a.tracks.len() == 1 { "1 song".to_owned() } else { format!("{} songs", a.tracks.len()) };
                    let albums = if a.albums == 1 { "1 album".to_owned() } else { format!("{} albums", a.albums) };
                    s.set_text(&format!("{songs} · {albums}"));
                }
            }
        });
        f
    }

    fn update_no_results(&self) {
        let q = self.search.text();
        let model = match self.views.visible_child_name().as_deref() {
            Some("songs") => Some(&self.song_model),
            Some("albums") => Some(&self.album_model),
            Some("artists") => Some(&self.artist_model),
            _ => None,
        };
        let empty = !q.trim().is_empty() && model.is_some_and(|m| m.n_items() == 0);
        if empty {
            self.no_results.set_text(&format!("Nothing matches “{}”", q.trim()));
        }
        self.no_results.set_visible(empty);
    }

    fn model_indices(&self, model: &gtk::FilterListModel) -> Vec<usize> {
        (0..model.n_items()).filter_map(|i| model.item(i).as_ref().and_then(index_of)).collect()
    }

    fn show_detail(self: &Rc<Self>, d: Detail, from: &str) {
        let tracks = self.tracks.borrow().clone();
        let (title, sub, ids, cover) = match &d {
            Detail::Album(i) => {
                let albums = self.albums.borrow();
                let Some(a) = albums.get(*i) else { return };
                let total: u64 = a.tracks.iter().filter_map(|&t| tracks.get(t)).map(|t| t.duration_ms).sum();
                let n = a.tracks.len();
                let sub = format!(
                    "{} · {} · {}",
                    a.artist,
                    if n == 1 { "1 song".into() } else { format!("{n} songs") },
                    format_time(total)
                );
                (a.title.clone(), sub, a.tracks.clone(), a.cover.clone())
            }
            Detail::Artist(i) => {
                let artists = self.artists.borrow();
                let Some(a) = artists.get(*i) else { return };
                let cover = a.tracks.iter().filter_map(|&t| tracks.get(t)).find_map(|t| t.cover.clone());
                let n = a.tracks.len();
                (a.name.clone(), if n == 1 { "1 song".into() } else { format!("{n} songs") }, a.tracks.clone(), cover)
            }
        };
        self.detail_title.set_text(&title);
        self.detail_sub.set_text(&sub);
        show_cover(&self.thumbs, &self.detail_cover, cover.as_ref(), 440);
        while let Some(c) = self.detail_list.first_child() {
            self.detail_list.remove(&c);
        }
        let album_view = matches!(d, Detail::Album(_));
        for (n, &i) in ids.iter().enumerate() {
            let Some(t) = tracks.get(i) else { continue };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            row.add_css_class("m3-detail-row");
            let num = label(
                &if album_view {
                    t.track_no.map(|n| n.to_string()).unwrap_or_else(|| (n + 1).to_string())
                } else {
                    (n + 1).to_string()
                },
                &["m3-label-large", "m3-secondary-text", "m3-track-no"],
            );
            num.set_width_chars(3);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
            text.set_hexpand(true);
            text.set_valign(gtk::Align::Center);
            let tt = label(&t.display_title(), &["m3-body-large"]);
            tt.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&tt);
            if !album_view {
                let s = label(t.display_album(), &["m3-body-medium", "m3-secondary-text"]);
                s.set_ellipsize(gtk::pango::EllipsizeMode::End);
                text.append(&s);
            }
            let dur = label(&format_time(t.duration_ms), &["m3-label-large", "m3-secondary-text", "m3-np-time"]);
            row.append(&num);
            row.append(&text);
            row.append(&dur);
            let lr = gtk::ListBoxRow::new();
            lr.set_child(Some(&row));
            self.detail_list.append(&lr);
        }
        *self.detail_tracks.borrow_mut() = ids;
        *self.detail.borrow_mut() = Some(d);
        *self.detail_back_to.borrow_mut() = from.to_owned();
        self.views.set_visible_child_name("detail");
        m3_widgets::animate(&self.detail_cover, "m3-enter");
    }

    // ------------------------------------------------------------ library

    fn set_tracks(self: &Rc<Self>, mut list: Vec<Track>) {
        // Keep files opened from outside the library.
        for e in self.externals.borrow().iter() {
            if !list.iter().any(|t| t.path == e.path) {
                list.push(e.clone());
            }
        }
        // Remap the queue by path.
        let old = self.tracks.borrow().clone();
        let (q_paths, cur_path) = {
            let q = self.queue.borrow();
            let mut items: Vec<usize> = q.current().into_iter().collect();
            items.extend(q.upcoming());
            (
                items.iter().filter_map(|&i| old.get(i).map(|t| t.path.clone())).collect::<Vec<_>>(),
                q.current().and_then(|i| old.get(i).map(|t| t.path.clone())),
            )
        };
        let index: HashMap<&std::path::Path, usize> =
            list.iter().enumerate().map(|(i, t)| (t.path.as_path(), i)).collect();
        let new_items: Vec<usize> = q_paths.iter().filter_map(|p| index.get(p.as_path()).copied()).collect();
        let still_current = cur_path.as_ref().is_some_and(|p| index.contains_key(p.as_path()));
        drop(index);

        *self.albums.borrow_mut() = library::albums(&list);
        *self.artists.borrow_mut() = library::artists(&list);
        *self.tracks.borrow_mut() = Rc::new(list);
        if !q_paths.is_empty() {
            let shuffle = self.queue.borrow().shuffle();
            let mut q = self.queue.borrow_mut();
            q.set_shuffle(false);
            q.set(new_items, 0);
            q.set_shuffle(shuffle);
            if !still_current && self.loaded.get() {
                drop(q);
                self.stop();
            }
        }
        let n = self.tracks.borrow().len();
        fill(&self.song_store, n);
        self.marked.set(if self.loaded.get() { self.queue.borrow().current() } else { None });
        fill(&self.album_store, self.albums.borrow().len());
        fill(&self.artist_store, self.artists.borrow().len());
        if self.views.visible_child_name().as_deref() == Some("detail") {
            let to = self.detail_back_to.borrow().clone();
            self.views.set_visible_child_name(&to);
        }
        self.update_content_page();
        self.publish_state();
        if self.restore.borrow().is_some() && !self.tracks.borrow().is_empty() {
            self.try_restore();
        }
    }

    fn update_content_page(&self) {
        let page = if !self.tracks.borrow().is_empty() {
            "library"
        } else if self.scanning.get() {
            "scanning"
        } else {
            "empty"
        };
        self.content.set_visible_child_name(page);
    }

    fn load_library(self: &Rc<Self>) {
        let t0 = std::time::Instant::now();
        if let Some(cached) = library::load_cache(&self.root) {
            let n = cached.len();
            self.set_tracks(cached);
            tracing::info!("library: {n} tracks from the cache in {} ms", t0.elapsed().as_millis());
        }
        self.rescan();
    }

    fn rescan(self: &Rc<Self>) {
        if self.scanning.get() {
            return;
        }
        self.scanning.set(true);
        self.scan_busy.set_visible_child_name("busy");
        self.scan_label.set_text(&format!("Looking for music in {}…", self.root.display()));
        self.update_content_page();
        let (tx, rx) = async_channel::unbounded::<ScanEvent>();
        let root = self.root.clone();
        let externals: Vec<PathBuf> = self.externals.borrow().iter().map(|t| t.path.clone()).collect();
        let cached: Vec<Track> =
            self.tracks.borrow().iter().filter(|t| !externals.contains(&t.path)).cloned().collect();
        let spawned = std::thread::Builder::new().name("m3-music-scan".into()).spawn(move || {
            library::scan(&root, &cached, &|e| {
                let _ = tx.send_blocking(e);
            });
        });
        if let Err(e) = spawned {
            self.scanning.set(false);
            self.snack(&format!("Cannot scan the library: {e}"));
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(e) = rx.recv().await {
                let Some(me) = weak.upgrade() else { break };
                match e {
                    ScanEvent::Progress { done, total } => {
                        me.scan_label.set_text(&format!("Reading {done} of {total} files in {}…", me.root.display()));
                    }
                    ScanEvent::Done(tracks) => {
                        me.scanning.set(false);
                        me.scan_busy.set_visible_child_name("idle");
                        me.set_tracks(tracks);
                    }
                    ScanEvent::Failed(msg) => {
                        me.scanning.set(false);
                        me.scan_busy.set_visible_child_name("idle");
                        me.update_content_page();
                        me.snack(&msg);
                    }
                }
            }
        });
    }

    /// Play files given on the command line / via MPRIS OpenUri.
    pub fn open_files(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
        if paths.is_empty() {
            self.snack("Nothing to play: the file could not be found.");
            return;
        }
        let (tx, rx) = async_channel::bounded::<Vec<Track>>(1);
        let spawned = std::thread::Builder::new().name("m3-music-open".into()).spawn(move || {
            let tracks = match gstreamer_pbutils::Discoverer::new(gstreamer::ClockTime::from_seconds(8)) {
                Ok(d) => paths
                    .iter()
                    .map(|p| {
                        let mtime = std::fs::metadata(p)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        library::read_track(&d, p, mtime)
                    })
                    .collect(),
                Err(_) => paths.iter().map(|p| Track { path: p.clone(), ..Default::default() }).collect(),
            };
            let _ = tx.send_blocking(tracks);
        });
        if spawned.is_err() {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(opened) = rx.recv().await else { return };
            let Some(me) = weak.upgrade() else { return };
            let paths: Vec<PathBuf> = opened.iter().map(|t| t.path.clone()).collect();
            {
                let mut ext = me.externals.borrow_mut();
                for t in opened {
                    if !ext.iter().any(|e| e.path == t.path) {
                        ext.push(t);
                    }
                }
            }
            let list: Vec<Track> = me.tracks.borrow().iter().filter(|t| !paths.contains(&t.path)).cloned().collect();
            me.set_tracks(list);
            let tracks = me.tracks.borrow().clone();
            let ids: Vec<usize> = paths.iter().filter_map(|p| tracks.iter().position(|t| &t.path == p)).collect();
            me.play_list(ids, 0, false);
        });
    }

    // ------------------------------------------------------------ playback

    fn init_player(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        match Player::new(move |e| {
            if let Some(me) = weak.upgrade() {
                me.on_player_event(e);
            }
        }) {
            Ok(p) => {
                self.updating.set(true);
                self.np_volume.set_value(p.volume());
                self.updating.set(false);
                self.mpris.update(Update::Volume(p.volume()));
                *self.player.borrow_mut() = Some(p);
            }
            Err(e) => self.snack(&e),
        }
    }

    fn with_player<T>(&self, f: impl FnOnce(&Player) -> T) -> Option<T> {
        self.player.borrow().as_ref().map(f)
    }

    fn play_list(self: &Rc<Self>, ids: Vec<usize>, start: usize, shuffle: bool) {
        if ids.is_empty() {
            return;
        }
        {
            let mut q = self.queue.borrow_mut();
            if shuffle {
                q.set_shuffle(true);
                let start = (glib::random_int() as usize) % ids.len();
                q.set(ids, start);
            } else {
                q.set(ids, start);
            }
        }
        if shuffle {
            self.sync_toggles();
        }
        self.errors_in_row.set(0);
        if let Some(i) = self.queue.borrow().current() {
            self.play_index(i);
        }
    }

    fn play_index(self: &Rc<Self>, i: usize) {
        self.load_index(i, true);
    }

    /// Load track `i`, playing or paused (restored sessions start paused).
    fn load_index(self: &Rc<Self>, i: usize, play: bool) {
        let tracks = self.tracks.borrow().clone();
        let Some(t) = tracks.get(i) else { return };
        if play {
            self.pending_seek.set(None);
        }
        let res = self.with_player(|p| p.load(&t.path, play));
        match res {
            Some(Ok(())) => self.loaded.set(true),
            Some(Err(e)) => {
                self.snack(&e);
                return;
            }
            None => return,
        }
        self.last_position.set(-1);
        self.show_track(t);
        self.mpris.update(Update::Track(Some(meta(i, t))));
        self.publish_state();
        self.refresh_lists();
        self.schedule_save();
    }

    fn show_track(&self, t: &Track) {
        let title = t.display_title();
        self.bar_title.set_text(&title);
        self.bar_artist.set_text(t.display_artist());
        self.np_title.set_text(&title);
        self.np_artist.set_text(&format!("{} — {}", t.display_artist(), t.display_album()));
        show_cover(&self.thumbs, &self.bar_cover, t.cover.as_ref(), 104);
        self.np_cover.set_cover(t.cover.as_deref());
        self.apply_art_scheme(t.cover.as_ref());
        self.bar.set_reveal_child(true);
        self.window.set_title(Some(&format!("{title} — Music")));
    }

    /// Colours from the cover for the now-playing surfaces.
    fn apply_art_scheme(&self, cover: Option<&PathBuf>) {
        let seed = cover.and_then(|c| {
            if let Some(s) = self.seeds.borrow().get(c) {
                return *s;
            }
            let s = art::seed(c);
            self.seeds.borrow_mut().insert(c.clone(), s);
            s
        });
        match seed {
            Some(s) => self.art_css.load_from_string(&art::scheme_css(&self.style.config(), s)),
            None => self.art_css.load_from_string(""),
        }
    }

    /// Rebind the rows of the previous and the new current track (GTK only
    /// rebinds rows whose item object changes).
    fn refresh_lists(&self) {
        let cur = if self.loaded.get() { self.queue.borrow().current() } else { None };
        let prev = self.marked.replace(cur);
        for i in [prev, cur].into_iter().flatten() {
            if (i as u32) < self.song_store.n_items() {
                self.song_store.splice(i as u32, 1, &[glib::BoxedAnyObject::new(i)]);
            }
        }
    }

    fn toggle_play(self: &Rc<Self>) {
        if !self.loaded.get() {
            let cur = self.queue.borrow().current();
            match cur {
                Some(i) => self.play_index(i),
                None => {
                    let ids = self.model_indices(&self.song_model);
                    self.play_list(ids, 0, false);
                }
            }
            return;
        }
        let playing = self.with_player(Player::is_playing).unwrap_or(false);
        self.with_player(|p| if playing { p.pause() } else { p.play() });
    }

    fn play(self: &Rc<Self>) {
        if self.loaded.get() {
            self.with_player(Player::play);
        } else {
            self.toggle_play();
        }
    }

    fn pause(&self) {
        self.with_player(Player::pause);
    }

    fn stop(&self) {
        self.with_player(Player::stop);
        self.loaded.set(false);
        self.set_playing_ui(false);
        self.np_slider.set_value(0.0);
        self.bar_progress.set_fraction(Some(0.0));
        self.np_elapsed.set_text("0:00");
        self.mpris.update(Update::Status("Stopped"));
        self.mpris.update(Update::Position(0));
        self.refresh_lists();
    }

    fn next(self: &Rc<Self>, auto: bool) {
        let n = self.queue.borrow_mut().next(auto);
        match n {
            Some(i) => self.play_index(i),
            None if auto => self.stop(),
            None => {}
        }
        self.publish_state();
    }

    fn previous(self: &Rc<Self>) {
        // Restart the track if we're more than 3 s in.
        if self.with_player(Player::position_ms).flatten().unwrap_or(0) > 3000 {
            self.seek_to(0);
            return;
        }
        if let Some(i) = self.queue.borrow_mut().previous() {
            self.play_index(i);
        }
        self.publish_state();
    }

    fn duration_ms(&self) -> u64 {
        self.with_player(Player::duration_ms)
            .flatten()
            .filter(|d| *d > 0)
            .or_else(|| {
                let i = self.queue.borrow().current()?;
                self.tracks.borrow().get(i).map(|t| t.duration_ms)
            })
            .unwrap_or(0)
    }

    fn seek_to(&self, ms: u64) {
        if !self.loaded.get() {
            return;
        }
        let d = self.duration_ms();
        let ms = if d > 0 { ms.min(d) } else { ms };
        self.with_player(|p| p.seek_ms(ms));
        self.mpris.update(Update::Seeked(ms as i64 * 1000));
        self.tick();
    }

    fn on_player_event(self: &Rc<Self>, e: PlayerEvent) {
        match e {
            PlayerEvent::Eos => {
                self.errors_in_row.set(0);
                self.next(true);
            }
            PlayerEvent::Error(msg) => {
                let title = self
                    .queue
                    .borrow()
                    .current()
                    .and_then(|i| self.tracks.borrow().get(i).map(|t| t.display_title()))
                    .unwrap_or_default();
                self.snack(&format!("Cannot play “{title}”: {msg}"));
                let n = self.errors_in_row.get() + 1;
                self.errors_in_row.set(n);
                // Skip broken files, but never loop over a queue of them.
                if n < self.queue.borrow().len() && self.queue.borrow().has_next() {
                    self.next(false);
                } else {
                    self.stop();
                }
            }
            PlayerEvent::State(playing) => {
                self.set_playing_ui(playing);
                if playing {
                    self.errors_in_row.set(0);
                }
                let status = if playing {
                    "Playing"
                } else if self.loaded.get() {
                    "Paused"
                } else {
                    "Stopped"
                };
                self.mpris.update(Update::Status(status));
                if !playing {
                    self.schedule_save();
                }
            }
            PlayerEvent::Duration => {
                if let Some(ms) = self.pending_seek.take() {
                    self.with_player(|p| p.seek_ms(ms));
                }
                // Files without a duration tag: publish it once known.
                let d = self.duration_ms();
                if let Some(i) = self.queue.borrow().current()
                    && let Some(t) = self.tracks.borrow().get(i)
                    && t.duration_ms == 0
                    && d > 0
                {
                    let mut m = meta(i, t);
                    m.length_us = d as i64 * 1000;
                    self.mpris.update(Update::Track(Some(m)));
                }
                self.tick();
            }
        }
    }

    fn set_playing_ui(&self, playing: bool) {
        let icon_name = if playing { "pause" } else { "play_arrow" };
        self.np_play.set_icon(icon_name);
        // Expressive: round when paused, a soft square while playing.
        self.np_play.shape().morph_to(if playing { Shape::Square } else { Shape::Circle }, 0.0, SpringSpec::FAST);
        if let Some(l) = self.bar_play.child().and_downcast::<gtk::Label>() {
            l.set_text(icon_name);
        }
        self.bar_play.set_tooltip_text(Some(if playing { "Pause" } else { "Play" }));
        if playing {
            self.bar_play.add_css_class("playing")
        } else {
            self.bar_play.remove_css_class("playing")
        }
        self.np_slider.set_waving(playing);
        self.bar_progress.set_waving(playing);
        self.np_cover.set_spinning(playing);
    }

    /// Periodic position update.
    fn tick(&self) {
        if !self.loaded.get() {
            return;
        }
        let pos = self.with_player(Player::position_ms).flatten().unwrap_or(0);
        let dur = self.duration_ms();
        let f = if dur > 0 { pos as f64 / dur as f64 } else { 0.0 };
        self.np_slider.set_value(f);
        self.bar_progress.set_fraction(Some(f));
        self.np_elapsed.set_text(&format_time(pos));
        self.np_remaining.set_text(&if dur > 0 {
            format!("−{}", format_time(dur.saturating_sub(pos)))
        } else {
            String::new()
        });
        let us = pos as i64 * 1000;
        if us != self.last_position.get() {
            self.last_position.set(us);
            self.mpris.update(Update::Position(us));
        }
    }

    fn publish_state(&self) {
        let q = self.queue.borrow();
        let has_tracks = !self.tracks.borrow().is_empty();
        self.np_prev.set_sensitive(self.loaded.get());
        self.np_next.set_sensitive(q.has_next());
        self.mpris.update(Update::Can {
            next: q.has_next(),
            previous: q.has_previous() || self.loaded.get(),
            play: has_tracks || self.loaded.get(),
        });
        self.mpris.update(Update::Shuffle(q.shuffle()));
        self.mpris.update(Update::Loop(q.repeat.mpris()));
    }

    fn sync_toggles(&self) {
        self.updating.set(true);
        let (shuffle, repeat) = {
            let q = self.queue.borrow();
            (q.shuffle(), q.repeat)
        };
        self.np_shuffle.set_active(shuffle);
        let (ic, tip) = match repeat {
            Repeat::Off => ("repeat", "Repeat: off"),
            Repeat::All => ("repeat", "Repeat: all"),
            Repeat::One => ("repeat_one", "Repeat: this song"),
        };
        if let Some(l) = self.np_repeat.child().and_downcast::<gtk::Label>() {
            l.set_text(ic);
        }
        self.np_repeat.set_tooltip_text(Some(tip));
        self.np_repeat.update_property(&[gtk::accessible::Property::Label(tip)]);
        if repeat == Repeat::Off {
            self.np_repeat.remove_css_class("active")
        } else {
            self.np_repeat.add_css_class("active")
        }
        self.updating.set(false);
        self.publish_state();
    }

    fn set_shuffle(&self, on: bool) {
        self.queue.borrow_mut().set_shuffle(on);
        self.sync_toggles();
        self.schedule_save();
    }

    fn set_repeat(&self, r: Repeat) {
        self.queue.borrow_mut().repeat = r;
        self.sync_toggles();
        self.schedule_save();
    }

    fn set_volume(&self, v: f64) {
        let v = v.clamp(0.0, 1.0);
        self.with_player(|p| p.set_volume(v));
        if (self.np_volume.value() - v).abs() > 0.001 {
            self.updating.set(true);
            self.np_volume.set_value(v);
            self.updating.set(false);
        }
        self.mpris.update(Update::Volume(v));
        self.schedule_save();
    }

    // ------------------------------------------------------------ saved state

    fn apply_saved(&self, saved: Option<Saved>) {
        let Some(saved) = saved else { return };
        self.set_volume(saved.volume);
        {
            let mut q = self.queue.borrow_mut();
            q.repeat = saved.repeat();
            q.set_shuffle(saved.shuffle);
        }
        self.sync_toggles();
        if saved.current.is_some() && !saved.queue.is_empty() {
            *self.restore.borrow_mut() = Some(saved);
        }
    }

    /// Re-create the saved queue (paused at the saved position) once the
    /// library knows the tracks.
    fn try_restore(self: &Rc<Self>) {
        if self.loaded.get() {
            self.restore.borrow_mut().take();
            return;
        }
        let Some(saved) = self.restore.borrow_mut().take() else { return };
        let tracks = self.tracks.borrow().clone();
        let index: HashMap<&std::path::Path, usize> =
            tracks.iter().enumerate().map(|(i, t)| (t.path.as_path(), i)).collect();
        let cur_path = saved.current.and_then(|c| saved.queue.get(c));
        let mut items = Vec::new();
        let mut start = None;
        for p in &saved.queue {
            if let Some(&i) = index.get(p.as_path()) {
                if Some(p) == cur_path {
                    start = Some(items.len());
                }
                items.push(i);
            }
        }
        let Some(start) = start else { return };
        drop(index);
        {
            let mut q = self.queue.borrow_mut();
            let shuffle = q.shuffle();
            q.set_shuffle(false);
            q.set(items, start);
            q.set_shuffle(shuffle);
        }
        if let Some(i) = self.queue.borrow().current() {
            self.pending_seek.set((saved.position_ms > 0).then_some(saved.position_ms));
            self.load_index(i, false);
        }
    }

    fn snapshot(&self) -> Saved {
        let q = self.queue.borrow();
        let tracks = self.tracks.borrow();
        let queue: Vec<PathBuf> = q.items().iter().filter_map(|&i| tracks.get(i).map(|t| t.path.clone())).collect();
        let current = if queue.len() == q.items().len() { q.current_item() } else { None };
        let position_ms = self
            .pending_seek
            .get()
            .or_else(|| if self.loaded.get() { self.with_player(Player::position_ms).flatten() } else { None })
            .unwrap_or(0);
        Saved {
            volume: self.np_volume.value(),
            shuffle: q.shuffle(),
            repeat: q.repeat.mpris().into(),
            queue,
            current,
            position_ms,
            ..Default::default()
        }
    }

    /// Write the state file now.
    pub fn save_state(&self) {
        if let Some(t) = self.save_timer.borrow_mut().take() {
            t.remove();
        }
        // Don't overwrite a saved queue that hasn't been restored yet.
        let mut s = self.snapshot();
        if let Some(r) = self.restore.borrow().as_ref() {
            s.queue = r.queue.clone();
            s.current = r.current;
            s.position_ms = r.position_ms;
        }
        if let Err(e) = state::save_to(&state::path(), &s) {
            tracing::warn!("cannot save the music state: {e}");
        }
    }

    /// Save shortly (coalesces bursts like dragging the volume slider).
    fn schedule_save(&self) {
        if self.save_timer.borrow().is_some() {
            return;
        }
        let timer = self.save_timer.clone();
        let weak = self.this.borrow().clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(400), move || {
            timer.borrow_mut().take();
            if let Some(me) = weak.upgrade() {
                me.save_state();
            }
        });
        *self.save_timer.borrow_mut() = Some(id);
    }

    fn toggle_now_playing(&self) {
        let open = !self.np.reveals_child();
        if open && !self.loaded.get() && self.queue.borrow().current().is_none() {
            return;
        }
        self.np.set_reveal_child(open);
        self.np.set_can_target(open);
        if open && let Some(child) = self.np.child() {
            m3_widgets::animate(&child, "m3-enter-up");
        }
        self.bar.set_reveal_child(!open && (self.loaded.get() || self.queue.borrow().current().is_some()));
    }

    pub fn snack(&self, text: &str) {
        self.snack_label.set_text(text);
        self.snackbar.set_reveal_child(true);
        if let Some(t) = self.snack_timer.borrow_mut().take() {
            t.remove();
        }
        let sb = self.snackbar.clone();
        let timer = self.snack_timer.clone();
        let id = glib::timeout_add_local_once(Duration::from_secs(5), move || {
            // This source is finished; forget it so it is never removed twice.
            timer.borrow_mut().take();
            sb.set_reveal_child(false);
        });
        *self.snack_timer.borrow_mut() = Some(id);
    }

    // ------------------------------------------------------------ wiring

    fn connect(self: &Rc<Self>, mpris_rx: async_channel::Receiver<Command>) {
        let weak = Rc::downgrade(self);
        let action = |name: &str, f: Handler| {
            let a = gio::SimpleAction::new(name, None);
            let w = weak.clone();
            a.connect_activate(move |_, _| {
                if let Some(me) = w.upgrade() {
                    f(&me);
                }
            });
            self.window.add_action(&a);
        };
        action("play-pause", Box::new(|m| m.toggle_play()));
        action("next", Box::new(|m| m.next(false)));
        action("previous", Box::new(|m| m.previous()));
        action("rescan", Box::new(|m| m.rescan()));
        action("now-playing", Box::new(|m| m.toggle_now_playing()));
        action(
            "search",
            Box::new(|m| {
                m.search.grab_focus();
            }),
        );
        self.app.set_accels_for_action("win.rescan", &["<Control>r"]);
        self.app.set_accels_for_action("win.search", &["<Control>f"]);
        self.app.set_accels_for_action("win.next", &["<Control>Right"]);
        self.app.set_accels_for_action("win.previous", &["<Control>Left"]);

        // Space toggles playback (unless typing); Esc closes now playing.
        let keys = gtk::EventControllerKey::new();
        let w = weak.clone();
        keys.connect_key_pressed(move |c, key, _, mods| {
            let Some(me) = w.upgrade() else { return glib::Propagation::Proceed };
            let typing = c
                .widget()
                .and_then(|win| win.root())
                .and_then(|r| r.focus())
                .is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::SearchEntry>());
            match key {
                gdk::Key::space if !typing && mods.is_empty() => {
                    me.toggle_play();
                    glib::Propagation::Stop
                }
                gdk::Key::Escape if me.np.reveals_child() => {
                    me.toggle_now_playing();
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        self.window.add_controller(keys);

        let w = weak.clone();
        self.bar_play.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                me.toggle_play();
            }
        });
        let w = weak.clone();
        self.np_slider.connect_seek(move |f| {
            if let Some(me) = w.upgrade() {
                let d = me.duration_ms();
                me.seek_to((d as f64 * f) as u64);
            }
        });
        let w = weak.clone();
        self.np_shuffle.connect_toggled(move |b| {
            if let Some(me) = w.upgrade()
                && !me.updating.get()
            {
                me.set_shuffle(b.is_active());
            }
        });
        let w = weak.clone();
        self.np_repeat.connect_clicked(move |_| {
            if let Some(me) = w.upgrade() {
                let r = me.queue.borrow().repeat.cycle();
                me.set_repeat(r);
            }
        });
        let w = weak.clone();
        self.np_volume.connect_value_changed(move |s| {
            if let Some(me) = w.upgrade()
                && !me.updating.get()
            {
                me.set_volume(s.value());
            }
        });

        // Theme changes: regenerate the album-art scheme with the new mode.
        let w = weak.clone();
        self.style.connect_changed(move |_, _| {
            if let Some(me) = w.upgrade() {
                let cover =
                    me.queue.borrow().current().and_then(|i| me.tracks.borrow().get(i).and_then(|t| t.cover.clone()));
                me.apply_art_scheme(cover.as_ref());
            }
        });

        // Save the position every few seconds while playing.
        let w = weak.clone();
        glib::timeout_add_seconds_local(5, move || match w.upgrade() {
            Some(me) => {
                if me.loaded.get() && me.with_player(Player::is_playing).unwrap_or(false) {
                    me.save_state();
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });

        // Debug aid: report main-loop stalls (the UI must never block).
        let last = Rc::new(Cell::new(std::time::Instant::now()));
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let gap = last.get().elapsed();
            if gap > Duration::from_millis(250) {
                tracing::debug!("main loop stalled for {} ms", gap.as_millis());
            }
            last.set(std::time::Instant::now());
            glib::ControlFlow::Continue
        });

        // Position updates.
        let w = weak.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || match w.upgrade() {
            Some(me) => {
                me.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });

        // MPRIS commands.
        let w = weak;
        glib::spawn_future_local(async move {
            while let Ok(c) = mpris_rx.recv().await {
                let Some(me) = w.upgrade() else { break };
                me.on_mpris(c);
            }
        });
    }

    fn on_mpris(self: &Rc<Self>, c: Command) {
        match c {
            Command::Play => self.play(),
            Command::Pause => self.pause(),
            Command::PlayPause => self.toggle_play(),
            Command::Stop => self.stop(),
            Command::Next => self.next(false),
            Command::Previous => self.previous(),
            Command::Seek(off) => {
                if !self.loaded.get() {
                    return;
                }
                let pos = self.with_player(Player::position_ms).flatten().unwrap_or(0) as i64;
                let target = pos + off / 1000;
                let d = self.duration_ms() as i64;
                if d > 0 && target >= d {
                    self.next(false);
                } else {
                    self.seek_to(target.max(0) as u64);
                }
            }
            Command::SetPosition(id, pos) => {
                let cur = self.queue.borrow().current();
                if cur.is_some_and(|i| mpris::track_path(i) == id) && pos >= 0 {
                    let d = self.duration_ms() as i64 * 1000;
                    if d == 0 || pos <= d {
                        self.seek_to((pos / 1000) as u64);
                    }
                }
            }
            Command::SetVolume(v) => self.set_volume(v),
            Command::SetShuffle(on) => self.set_shuffle(on),
            Command::SetLoop(s) => {
                if let Some(r) = Repeat::from_mpris(&s) {
                    self.set_repeat(r);
                }
            }
            Command::OpenUri(uri) => {
                if let Some(p) = gio::File::for_uri(&uri).path() {
                    self.open_files(vec![p]);
                }
            }
            Command::Raise => self.window.present(),
            Command::Quit => self.app.quit(),
        }
    }
}

fn meta(i: usize, t: &Track) -> Meta {
    Meta {
        id: i,
        title: t.display_title(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        album_artist: t.album_artist.clone(),
        length_us: t.duration_ms as i64 * 1000,
        track_no: t.track_no,
        art_url: t.cover.as_ref().map(|c| gio::File::for_path(c).uri().to_string()),
        url: gio::File::for_path(&t.path).uri().to_string(),
    }
}

fn fill(store: &gio::ListStore, n: usize) {
    let items: Vec<glib::BoxedAnyObject> = (0..n).map(glib::BoxedAnyObject::new).collect();
    store.splice(0, store.n_items(), &items);
}

fn children(w: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut v = Vec::new();
    let mut c = w.first_child();
    while let Some(x) = c {
        c = x.next_sibling();
        v.push(x);
    }
    v
}

fn scrolled(child: &impl IsA<gtk::Widget>, class: &str) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    s.set_child(Some(child));
    s.add_css_class(class);
    s.set_vexpand(true);
    s
}
