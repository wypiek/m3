//! A file manager window: sidebar, toolbar with path bar, grid/list views
//! over a filtered + sorted model, search, clipboard, drag and drop.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::ops::{Job, Operations, Outcome};
use crate::sidebar::{self, Sidebar, Target};
use crate::thumbs::Thumbnailer;
use crate::util::{self, SortEntry, SortKey};
use crate::{dialogs, trash};

/// Attributes every view needs.
pub const ATTRS: &str = "standard::name,standard::display-name,standard::type,standard::size,standard::icon,\
standard::content-type,standard::is-hidden,standard::is-backup,standard::is-symlink,standard::target-uri,\
time::modified,thumbnail::path,thumbnail::failed,id::filesystem,access::can-write,access::can-trash";

thread_local! {
    static WINDOWS: RefCell<Vec<Rc<FilesWindow>>> = const { RefCell::new(Vec::new()) };
}

pub fn icon(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("m3-icon");
    l.add_css_class("small");
    l.set_accessible_role(gtk::AccessibleRole::Img);
    l
}

fn icon_button(name: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&icon(name)));
    b.add_css_class("icon");
    b.set_tooltip_text(Some(tooltip));
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

/// What a window shows.
#[derive(Clone)]
pub enum Location {
    Folder(gio::File),
    Recent,
    Trash,
    Search { base: gio::File, query: String },
}

impl Location {
    pub fn describe(&self) -> String {
        match self {
            Location::Folder(f) => f.parse_name().to_string(),
            Location::Recent => "Recent files".into(),
            Location::Trash => "Trash".into(),
            Location::Search { base, query } => format!("“{query}” in {}", base.parse_name()),
        }
    }

    pub fn same_place(&self, other: &Location) -> bool {
        match (self, other) {
            (Location::Folder(a), Location::Folder(b)) => a.equal(b),
            (Location::Recent, Location::Recent) | (Location::Trash, Location::Trash) => true,
            _ => false,
        }
    }

    fn folder(&self) -> Option<&gio::File> {
        match self {
            Location::Folder(f) => Some(f),
            Location::Search { base, .. } => Some(base),
            _ => None,
        }
    }
}

pub fn file_of(info: &gio::FileInfo) -> Option<gio::File> {
    info.attribute_object("standard::file").and_downcast::<gio::File>()
}

fn entry_of(info: &gio::FileInfo) -> SortEntry {
    SortEntry {
        name: info.display_name().to_string(),
        is_dir: info.file_type() == gio::FileType::Directory,
        size: info.size(),
        modified: info.modification_date_time().map_or(0, |d| d.to_unix()),
        content_type: info.content_type().map(|c| c.to_string()).unwrap_or_default(),
    }
}

/// Query `file` for the views (blocking; local files are fast).
fn info_for(file: &gio::File) -> Option<gio::FileInfo> {
    let i = file.query_info(ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok()?;
    i.set_attribute_object("standard::file", file);
    Some(i)
}

#[derive(Clone, Copy)]
struct SortState {
    key: SortKey,
    desc: bool,
    folders_first: bool,
}

struct SearchJob {
    stop: Arc<AtomicBool>,
    found: Arc<Mutex<Vec<PathBuf>>>,
    done: Arc<AtomicBool>,
}

pub struct FilesWindow {
    pub window: gtk::ApplicationWindow,
    location: RefCell<Location>,
    back: RefCell<Vec<Location>>,
    forward: RefCell<Vec<Location>>,
    dir_list: gtk::DirectoryList,
    store: gio::ListStore,
    filter_model: gtk::FilterListModel,
    filter: gtk::CustomFilter,
    sorter: gtk::CustomSorter,
    sort_model: gtk::SortListModel,
    selection: gtk::MultiSelection,
    sort: Rc<Cell<SortState>>,
    show_hidden: Rc<Cell<bool>>,
    grid: gtk::GridView,
    list: gtk::ColumnView,
    views: gtk::Stack,
    status: gtk::Box,
    status_icon: m3_widgets::ShapeIcon,
    status_title: gtk::Label,
    status_body: gtk::Label,
    loading: gtk::Widget,
    crumbs: gtk::Box,
    path_stack: gtk::Stack,
    location_entry: gtk::Entry,
    search_bar: gtk::Revealer,
    search_entry: gtk::SearchEntry,
    search: RefCell<Option<SearchJob>>,
    search_gen: Cell<u64>,
    empty_trash: gtk::Button,
    sidebar: Rc<Sidebar>,
    ops: Rc<Operations>,
    thumbs: Rc<Thumbnailer>,
    menu: gtk::PopoverMenu,
    overlay: gtk::Overlay,
    snackbar: gtk::Revealer,
    snack_label: gtk::Label,
    snack_action: gtk::Button,
    snack_cb: RefCell<Option<Box<dyn Fn()>>>,
    snack_timer: Rc<RefCell<Option<glib::SourceId>>>,
    pending_select: RefCell<Vec<String>>,
    typeahead: RefCell<(String, Option<glib::SourceId>)>,
}

impl FilesWindow {
    pub fn open(app: &gtk::Application, loc: Location, select: Option<gio::File>) -> Rc<Self> {
        let w = Self::build(app);
        if let Some(f) = select {
            w.pending_select.borrow_mut().push(f.uri().to_string());
        }
        w.navigate(loc, true);
        w.window.present();
        w.grid.grab_focus();
        WINDOWS.with(|ws| ws.borrow_mut().push(w.clone()));
        w
    }

    pub fn location_for_new_window(&self) -> Location {
        match &*self.location.borrow() {
            Location::Search { base, .. } => Location::Folder(base.clone()),
            other => other.clone(),
        }
    }

    pub fn any() -> Option<Rc<Self>> {
        WINDOWS.with(|ws| ws.borrow().last().cloned())
    }

    fn build(app: &gtk::Application) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Files")
            .default_width(1100)
            .default_height(720)
            .build();
        window.add_css_class("m3");
        window.add_css_class("m3-files-window");

        // ---- model: source → hidden filter → sorter → selection
        let dir_list = gtk::DirectoryList::new(Some(ATTRS), gio::File::NONE);
        dir_list.set_monitored(true);
        let store = gio::ListStore::new::<gio::FileInfo>();
        let show_hidden = Rc::new(Cell::new(false));
        let sh = show_hidden.clone();
        let filter = gtk::CustomFilter::new(move |o| {
            let Some(i) = o.downcast_ref::<gio::FileInfo>() else { return false };
            sh.get() || !(i.is_hidden() || i.is_backup())
        });
        let filter_model =
            gtk::FilterListModel::new(Some(dir_list.clone().upcast::<gio::ListModel>()), Some(filter.clone()));
        let sort = Rc::new(Cell::new(SortState { key: SortKey::Name, desc: false, folders_first: true }));
        let st = sort.clone();
        let sorter = gtk::CustomSorter::new(move |a, b| {
            let (Some(a), Some(b)) = (a.downcast_ref::<gio::FileInfo>(), b.downcast_ref::<gio::FileInfo>()) else {
                return gtk::Ordering::Equal;
            };
            let s = st.get();
            util::compare(&entry_of(a), &entry_of(b), s.key, s.desc, s.folders_first).into()
        });
        let sort_model = gtk::SortListModel::new(Some(filter_model.clone()), Some(sorter.clone()));
        let selection = gtk::MultiSelection::new(Some(sort_model.clone()));

        let thumbs: Rc<Thumbnailer> = Rc::default();
        let grid = gtk::GridView::new(Some(selection.clone()), None::<gtk::ListItemFactory>);
        grid.set_enable_rubberband(false);
        grid.set_max_columns(16);
        grid.set_min_columns(2);
        grid.add_css_class("m3-files-grid");
        let list = gtk::ColumnView::new(Some(selection.clone()));
        list.set_enable_rubberband(false);
        list.set_show_row_separators(false);
        list.add_css_class("m3-files-list");

        let views = gtk::Stack::new();
        views.set_transition_type(gtk::StackTransitionType::Crossfade);
        let gs = gtk::ScrolledWindow::new();
        gs.set_child(Some(&grid));
        gs.set_vexpand(true);
        let ls = gtk::ScrolledWindow::new();
        ls.set_child(Some(&list));
        views.add_named(&gs, Some("grid"));
        views.add_named(&ls, Some("list"));

        // ---- empty / error state
        let status = gtk::Box::new(gtk::Orientation::Vertical, 10);
        status.add_css_class("m3-files-status");
        status.set_halign(gtk::Align::Center);
        status.set_valign(gtk::Align::Center);
        status.set_can_target(false);
        let status_icon = m3_widgets::ShapeIcon::new(m3_widgets::Shape::Cookie(9), "folder_open", 96, true);
        status_icon.widget().add_css_class("secondary");
        let status_title = gtk::Label::new(None);
        status_title.add_css_class("m3-headline-small-emphasized");
        let status_body = gtk::Label::new(None);
        status_body.add_css_class("m3-secondary-text");
        status_body.set_wrap(true);
        status_body.set_justify(gtk::Justification::Center);
        status_body.set_max_width_chars(48);
        status.append(status_icon.widget());
        status.append(&status_title);
        status.append(&status_body);
        status.set_visible(false);
        let loading = m3_widgets::LoadingIndicator::new(48).widget().clone().upcast::<gtk::Widget>();
        loading.set_halign(gtk::Align::Center);
        loading.set_valign(gtk::Align::Center);
        loading.set_can_target(false);
        loading.set_visible(false);

        // ---- header: nav group, path bar, actions
        let header = gtk::HeaderBar::new();
        header.add_css_class("m3-files-header");
        let nav = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        nav.add_css_class("m3-button-group");
        let back_b = icon_button("arrow_back", "Back (Alt+←)");
        back_b.set_action_name(Some("win.back"));
        let fwd_b = icon_button("arrow_forward", "Forward (Alt+→)");
        fwd_b.set_action_name(Some("win.forward"));
        let up_b = icon_button("arrow_upward", "Parent folder (Alt+↑)");
        up_b.set_action_name(Some("win.up"));
        nav.append(&back_b);
        nav.append(&fwd_b);
        nav.append(&up_b);
        header.pack_start(&nav);

        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        crumbs.add_css_class("m3-files-crumbs");
        let crumb_scroll = gtk::ScrolledWindow::new();
        crumb_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        crumb_scroll.set_child(Some(&crumbs));
        crumb_scroll.set_hexpand(true);
        let location_entry = gtk::Entry::new();
        location_entry.add_css_class("m3-files-location");
        location_entry.set_placeholder_text(Some("Type a path, ~ for home"));
        let path_stack = gtk::Stack::new();
        path_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        path_stack.add_named(&crumb_scroll, Some("crumbs"));
        path_stack.add_named(&location_entry, Some("entry"));
        path_stack.set_hexpand(true);
        path_stack.add_css_class("m3-files-pathbar");
        header.set_title_widget(Some(&path_stack));

        let ops_placeholder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let search_toggle = gtk::ToggleButton::new();
        search_toggle.set_child(Some(&icon("search")));
        search_toggle.add_css_class("icon");
        search_toggle.set_tooltip_text(Some("Search (Ctrl+F)"));
        let view_toggle = icon_button("view_list", "List view");
        view_toggle.set_action_name(Some("win.toggle-view"));
        let menu_b = gtk::MenuButton::new();
        menu_b.set_child(Some(&icon("tune")));
        menu_b.add_css_class("icon");
        menu_b.set_tooltip_text(Some("View options"));
        menu_b.set_menu_model(Some(&view_menu()));
        let empty_trash = gtk::Button::with_label("Empty trash");
        empty_trash.add_css_class("danger");
        empty_trash.set_action_name(Some("win.empty-trash"));
        empty_trash.set_visible(false);
        header.pack_end(&menu_b);
        header.pack_end(&view_toggle);
        header.pack_end(&search_toggle);
        header.pack_end(&ops_placeholder);
        header.pack_end(&empty_trash);
        window.set_titlebar(Some(&header));

        let search_entry = gtk::SearchEntry::new();
        search_entry.add_css_class("m3-search");
        search_entry.set_placeholder_text(Some("Search in this folder and below"));
        search_entry.set_hexpand(true);
        let sbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sbox.add_css_class("m3-files-searchbar");
        sbox.append(&search_entry);
        let search_bar = gtk::Revealer::new();
        search_bar.set_child(Some(&sbox));
        search_bar.set_transition_type(gtk::RevealerTransitionType::SlideDown);

        // ---- body
        let content_overlay = gtk::Overlay::new();
        content_overlay.set_child(Some(&views));
        content_overlay.add_overlay(&status);
        content_overlay.add_overlay(&loading);
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.add_css_class("m3-files-pane");
        pane.set_overflow(gtk::Overflow::Hidden);
        pane.set_hexpand(true);
        pane.append(&search_bar);
        pane.append(&content_overlay);

        let snack_label = gtk::Label::new(None);
        snack_label.set_wrap(true);
        snack_label.set_xalign(0.0);
        snack_label.set_hexpand(true);
        let snack_action = gtk::Button::with_label("Undo");
        snack_action.add_css_class("text");
        let sb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        sb.add_css_class("m3-snackbar");
        sb.append(&snack_label);
        sb.append(&snack_action);
        let snackbar = gtk::Revealer::new();
        snackbar.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        snackbar.set_child(Some(&sb));
        snackbar.set_halign(gtk::Align::Center);
        snackbar.set_valign(gtk::Align::End);
        snackbar.set_margin_bottom(20);

        let menu = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);

        let overlay = gtk::Overlay::new();
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        overlay.set_child(Some(&body));
        overlay.add_overlay(&snackbar);
        menu.set_parent(&overlay);
        window.set_child(Some(&overlay));

        let ops = Operations::new(window.upcast_ref());
        ops_placeholder.append(&ops.button);

        let me: Rc<FilesWindow> = Rc::new_cyclic(|weak: &Weak<FilesWindow>| {
            let wn = weak.clone();
            let wn2 = weak.clone();
            let sidebar = Sidebar::new(
                Rc::new(move |t| {
                    if let Some(w) = wn.upgrade() {
                        w.go_target(t);
                    }
                }),
                Rc::new(move |msg| {
                    if let Some(w) = wn2.upgrade() {
                        w.toast(msg, None);
                    }
                }),
            );
            body.append(&sidebar.widget);
            body.append(&pane);
            FilesWindow {
                window: window.clone(),
                location: RefCell::new(Location::Recent),
                back: RefCell::default(),
                forward: RefCell::default(),
                dir_list,
                store,
                filter_model,
                filter,
                sorter,
                sort_model,
                selection,
                sort,
                show_hidden,
                grid,
                list,
                views,
                status,
                status_icon,
                status_title,
                status_body,
                loading,
                crumbs,
                path_stack,
                location_entry,
                search_bar,
                search_entry,
                search: RefCell::new(None),
                search_gen: Cell::new(0),
                empty_trash,
                sidebar,
                ops,
                thumbs,
                menu,
                overlay,
                snackbar,
                snack_label,
                snack_action,
                snack_cb: RefCell::new(None),
                snack_timer: Rc::default(),
                pending_select: RefCell::default(),
                typeahead: RefCell::default(),
            }
        });
        me.setup_factories();
        me.setup_actions(&view_toggle);
        me.setup_input(&gs, &ls);
        me.setup_signals(&search_toggle, &crumb_scroll);
        {
            let weak = Rc::downgrade(&me);
            me.snack_action.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.snackbar.set_reveal_child(false);
                    if let Some(cb) = w.snack_cb.borrow_mut().take() {
                        cb();
                    }
                }
            });
        }
        let weak = Rc::downgrade(&me);
        me.window.connect_destroy(move |_| {
            if let Some(w) = weak.upgrade() {
                w.stop_search();
                WINDOWS.with(|ws| ws.borrow_mut().retain(|x| !Rc::ptr_eq(x, &w)));
            }
        });
        me
    }

    // ---- views -------------------------------------------------------------

    fn setup_factories(self: &Rc<Self>) {
        // Grid tiles.
        let f = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        f.connect_setup(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
            b.add_css_class("m3-files-tile");
            let img = gtk::Image::new();
            img.set_pixel_size(64);
            img.add_css_class("m3-files-thumb");
            let l = gtk::Label::new(None);
            l.set_wrap(true);
            l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            l.set_lines(2);
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            l.set_justify(gtk::Justification::Center);
            l.set_max_width_chars(14);
            l.set_width_chars(14);
            l.add_css_class("m3-body-medium");
            b.append(&img);
            b.append(&l);
            li.set_child(Some(&b));
            if let Some(w) = weak.upgrade() {
                w.item_controllers(&b, li);
            }
        });
        let weak = Rc::downgrade(self);
        f.connect_bind(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(info), Some(b)) = (li.item().and_downcast::<gio::FileInfo>(), li.child()) else { return };
            let Some(img) = b.first_child().and_downcast::<gtk::Image>() else { return };
            let Some(l) = img.next_sibling().and_downcast::<gtk::Label>() else { return };
            l.set_text(&display_name(&info));
            img.set_from_gicon(&info.icon().unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast()));
            b.set_tooltip_text(Some(&info.display_name()));
            if let (Some(w), Some(file)) = (weak.upgrade(), file_of(&info)) {
                let li_w = li.downgrade();
                let info2 = info.clone();
                let img2 = img.clone();
                w.thumbs.request(&info, &file, move |t| {
                    let still = li_w
                        .upgrade()
                        .and_then(|li| li.item())
                        .is_some_and(|i| i == info2.clone().upcast::<glib::Object>());
                    if still {
                        img2.set_paintable(Some(t));
                    }
                });
            }
        });
        self.grid.set_factory(Some(&f));

        // List columns.
        let name_f = gtk::SignalListItemFactory::new();
        let weak = Rc::downgrade(self);
        name_f.connect_setup(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.add_css_class("m3-files-row");
            let img = gtk::Image::new();
            img.set_pixel_size(24);
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            b.append(&img);
            b.append(&l);
            li.set_child(Some(&b));
            if let Some(w) = weak.upgrade() {
                w.item_controllers(&b, li);
            }
        });
        name_f.connect_bind(|_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let (Some(info), Some(b)) = (li.item().and_downcast::<gio::FileInfo>(), li.child()) else { return };
            if let (Some(img), Some(l)) = (
                b.first_child().and_downcast::<gtk::Image>(),
                b.first_child().and_then(|c| c.next_sibling()).and_downcast::<gtk::Label>(),
            ) {
                img.set_from_gicon(&info.icon().unwrap_or_else(|| gio::ThemedIcon::new("text-x-generic").upcast()));
                l.set_text(&display_name(&info));
            }
        });
        let name_col = gtk::ColumnViewColumn::new(Some("Name"), Some(name_f));
        name_col.set_expand(true);
        self.list.append_column(&name_col);
        for (title, kind) in [("Size", 0), ("Modified", 1), ("Type", 2)] {
            let f = gtk::SignalListItemFactory::new();
            f.connect_setup(|_, li| {
                if let Some(li) = li.downcast_ref::<gtk::ListItem>() {
                    let l = gtk::Label::new(None);
                    l.set_xalign(0.0);
                    l.add_css_class("m3-secondary-text");
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    li.set_child(Some(&l));
                }
            });
            f.connect_bind(move |_, li| {
                let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
                let (Some(info), Some(l)) =
                    (li.item().and_downcast::<gio::FileInfo>(), li.child().and_downcast::<gtk::Label>())
                else {
                    return;
                };
                l.set_text(&match kind {
                    0 if info.file_type() == gio::FileType::Directory => "—".into(),
                    0 => util::format_size(info.size().max(0) as u64),
                    1 => info
                        .modification_date_time()
                        .and_then(|d| d.to_local().ok())
                        .and_then(|d| d.format("%-d %b %Y %H:%M").ok())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    _ => info
                        .content_type()
                        .map(|c| gio::content_type_get_description(&c).to_string())
                        .unwrap_or_default(),
                });
            });
            let col = gtk::ColumnViewColumn::new(Some(title), Some(f));
            col.set_fixed_width(if kind == 1 { 170 } else { 130 });
            self.list.append_column(&col);
        }
    }

    /// Right-click menu, drag source and (for folders) drop target on an item.
    fn item_controllers(self: &Rc<Self>, w: &gtk::Box, li: &gtk::ListItem) {
        let click = gtk::GestureClick::new();
        click.set_button(3);
        let weak = Rc::downgrade(self);
        let li_w = li.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let (Some(win), Some(li)) = (weak.upgrade(), li_w.upgrade()) else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            let pos = li.position();
            if !win.selection.is_selected(pos) {
                win.selection.select_item(pos, true);
            }
            if let Some(widget) = g.widget() {
                win.popup_menu(&widget, x, y, true);
            }
        });
        w.add_controller(click);
        // Middle click opens folders in a new window.
        let mid = gtk::GestureClick::new();
        mid.set_button(2);
        let weak = Rc::downgrade(self);
        let li_w = li.downgrade();
        mid.connect_pressed(move |_, _, _, _| {
            let (Some(win), Some(li)) = (weak.upgrade(), li_w.upgrade()) else { return };
            if let Some(info) = li.item().and_downcast::<gio::FileInfo>() {
                if let (gio::FileType::Directory, Some(f)) = (info.file_type(), file_of(&info)) {
                    if let Some(app) = win.window.application() {
                        FilesWindow::open(&app, Location::Folder(f), None);
                    }
                }
            }
        });
        w.add_controller(mid);

        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        // Capture phase: dragging an item moves files; the view's rubber
        // band (bubble phase) then only starts on empty space.
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        let li_w = li.downgrade();
        drag.connect_prepare(move |d, _, _| {
            let (win, li) = (weak.upgrade()?, li_w.upgrade()?);
            let pos = li.position();
            if !win.selection.is_selected(pos) {
                win.selection.select_item(pos, true);
            }
            let files = win.selected_files();
            if files.is_empty() {
                return None;
            }
            if let Some(w) = d.widget() {
                d.set_icon(Some(&gtk::WidgetPaintable::new(Some(&w))), 0, 0);
            }
            Some(files_content(&files, false))
        });
        w.add_controller(drag);

        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        let li_w = li.downgrade();
        drop.connect_accept(move |_, _| {
            li_w.upgrade()
                .and_then(|li| li.item().and_downcast::<gio::FileInfo>())
                .is_some_and(|i| i.file_type() == gio::FileType::Directory)
                && weak.upgrade().is_some()
        });
        let weak = Rc::downgrade(self);
        let li_w = li.downgrade();
        drop.connect_drop(move |t, value, _, _| {
            let (Some(win), Some(li)) = (weak.upgrade(), li_w.upgrade()) else { return false };
            let Some(dest) = li.item().and_downcast::<gio::FileInfo>().and_then(|i| file_of(&i)) else { return false };
            win.handle_drop(t, value, &dest)
        });
        w.add_controller(drop);
    }

    fn handle_drop(self: &Rc<Self>, t: &gtk::DropTarget, value: &glib::Value, dest: &gio::File) -> bool {
        let Ok(list) = value.get::<gdk::FileList>() else { return false };
        let files: Vec<gio::File> = list.files().into_iter().filter(|f| !f.equal(dest)).collect();
        if files.is_empty() {
            return false;
        }
        // Dropped where they already are: nothing to do.
        if files.iter().all(|f| f.parent().is_some_and(|p| p.equal(dest))) {
            return false;
        }
        let state = t.current_event_state();
        let mv = if state.contains(gdk::ModifierType::CONTROL_MASK) {
            false
        } else if state.contains(gdk::ModifierType::SHIFT_MASK) {
            true
        } else {
            same_filesystem(&files[0], dest)
        };
        self.run(if mv {
            Job::Move { sources: files, dest: dest.clone() }
        } else {
            Job::Copy { sources: files, dest: dest.clone() }
        });
        true
    }

    // ---- navigation ------------------------------------------------------------

    fn go_target(self: &Rc<Self>, t: Target) {
        match t {
            Target::Go(loc) => self.navigate(loc, true),
            Target::Mount(v) => {
                let op = gtk::MountOperation::new(Some(&self.window));
                let weak = Rc::downgrade(self);
                glib::spawn_future_local(async move {
                    let r = v.mount_future(gio::MountMountFlags::NONE, Some(&op)).await;
                    let Some(w) = weak.upgrade() else { return };
                    match r {
                        Ok(()) => {
                            if let Some(root) = v.get_mount().map(|m| m.root()) {
                                w.navigate(Location::Folder(root), true);
                            }
                        }
                        Err(e) => w.toast(&format!("Can't mount “{}”: {}", v.name(), e.message()), None),
                    }
                });
            }
        }
    }

    /// Show `loc`. `record` pushes the current location to the back stack.
    pub fn navigate(self: &Rc<Self>, loc: Location, record: bool) {
        if record {
            let cur = self.location.borrow().clone();
            if !cur.same_place(&loc) || matches!(cur, Location::Search { .. }) {
                self.back.borrow_mut().push(cur);
                self.forward.borrow_mut().clear();
            }
        }
        if !matches!(loc, Location::Search { .. }) {
            self.stop_search();
            // Going somewhere else ends the search (clearing the query keeps
            // the bar open: that navigation isn't recorded).
            if record && self.search_bar.reveals_child() {
                self.search_bar.set_reveal_child(false);
            }
        }
        *self.location.borrow_mut() = loc.clone();
        self.selection.unselect_all();
        match &loc {
            Location::Folder(f) => {
                self.dir_list.set_file(Some(f));
                self.filter_model.set_model(Some(&self.dir_list));
            }
            Location::Recent => {
                self.dir_list.set_file(gio::File::NONE);
                self.fill_recent();
                self.filter_model.set_model(Some(&self.store));
            }
            Location::Trash => {
                self.dir_list.set_file(gio::File::NONE);
                self.fill_trash();
                self.filter_model.set_model(Some(&self.store));
            }
            Location::Search { base, query } => {
                self.dir_list.set_file(gio::File::NONE);
                self.store.remove_all();
                self.filter_model.set_model(Some(&self.store));
                self.start_search(base.clone(), query.clone());
            }
        }
        self.empty_trash.set_visible(matches!(loc, Location::Trash));
        self.update_crumbs();
        self.sidebar.select(&loc);
        self.update_nav_actions();
        self.update_status();
        self.window.set_title(Some(&match &loc {
            Location::Folder(f) => f.basename().map(|b| b.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()),
            other => other.describe(),
        }));
    }

    fn update_nav_actions(&self) {
        let set = |name: &str, on: bool| {
            if let Some(a) = self.window.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                a.set_enabled(on);
            }
        };
        set("back", !self.back.borrow().is_empty());
        set("forward", !self.forward.borrow().is_empty());
        set("up", self.location.borrow().folder().and_then(|f| f.parent()).is_some());
    }

    fn go_back(self: &Rc<Self>) {
        let prev = self.back.borrow_mut().pop();
        if let Some(prev) = prev {
            let cur = self.location.borrow().clone();
            self.forward.borrow_mut().push(cur);
            self.navigate(prev, false);
        }
    }

    fn go_forward(self: &Rc<Self>) {
        let next = self.forward.borrow_mut().pop();
        if let Some(next) = next {
            let cur = self.location.borrow().clone();
            self.back.borrow_mut().push(cur);
            self.navigate(next, false);
        }
    }

    fn go_up(self: &Rc<Self>) {
        let cur = self.location.borrow().clone();
        if let Some(parent) = cur.folder().and_then(|f| f.parent()) {
            if let Some(f) = cur.folder() {
                self.pending_select.borrow_mut().push(f.uri().to_string());
            }
            self.navigate(Location::Folder(parent), true);
        }
    }

    fn update_crumbs(self: &Rc<Self>) {
        while let Some(c) = self.crumbs.first_child() {
            self.crumbs.remove(&c);
        }
        let loc = self.location.borrow().clone();
        let segs: Vec<(String, Option<gio::File>)> = match &loc {
            Location::Folder(f) | Location::Search { base: f, .. } => match f.path() {
                Some(p) => util::path_segments(&p, &glib::home_dir())
                    .into_iter()
                    .map(|(l, p)| (l, Some(gio::File::for_path(p))))
                    .collect(),
                None => vec![(f.parse_name().to_string(), Some(f.clone()))],
            },
            Location::Recent => vec![("Recent".into(), None)],
            Location::Trash => vec![("Trash".into(), None)],
        };
        let n = segs.len();
        for (i, (label, file)) in segs.into_iter().enumerate() {
            if i > 0 {
                let sep = icon("chevron_right");
                sep.add_css_class("m3-files-crumb-sep");
                self.crumbs.append(&sep);
            }
            let b = gtk::Button::with_label(&label);
            b.add_css_class("m3-files-crumb");
            if i == n - 1 {
                b.add_css_class("current");
            }
            if let Some(f) = file {
                let weak = Rc::downgrade(self);
                b.connect_clicked(move |_| {
                    if let Some(w) = weak.upgrade() {
                        w.navigate(Location::Folder(f.clone()), true);
                    }
                });
            }
            self.crumbs.append(&b);
        }
        if let Location::Search { query, .. } = &loc {
            let chip = gtk::Label::new(Some(&format!("Search: {query}")));
            chip.add_css_class("m3-files-search-chip");
            self.crumbs.append(&chip);
        }
        self.path_stack.set_visible_child_name("crumbs");
    }

    fn show_location_entry(&self) {
        let text = match self.location.borrow().folder() {
            Some(f) => f
                .path()
                .map(|p| {
                    let home = glib::home_dir();
                    match p.strip_prefix(&home) {
                        Ok(rest) if rest.as_os_str().is_empty() => "~/".to_owned(),
                        Ok(rest) => format!("~/{}/", rest.display()),
                        Err(_) => format!("{}/", p.display()).replace("//", "/"),
                    }
                })
                .unwrap_or_else(|| f.uri().to_string()),
            None => "~/".into(),
        };
        self.location_entry.set_text(&text);
        self.path_stack.set_visible_child_name("entry");
        self.location_entry.grab_focus();
        self.location_entry.set_position(-1);
    }

    fn go_typed_location(self: &Rc<Self>) {
        let text = self.location_entry.text().to_string();
        let f = if text.contains("://") {
            gio::File::for_uri(&text)
        } else {
            gio::File::for_path(util::expand_tilde(&text, &glib::home_dir()))
        };
        match f.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
            gio::FileType::Directory => self.navigate(Location::Folder(f), true),
            gio::FileType::Unknown => {
                self.toast(&format!("“{text}” doesn't exist."), None);
                return;
            }
            _ => {
                if let Some(p) = f.parent() {
                    self.pending_select.borrow_mut().push(f.uri().to_string());
                    self.navigate(Location::Folder(p), true);
                }
            }
        }
        self.path_stack.set_visible_child_name("crumbs");
        self.grid.grab_focus();
    }

    // ---- sources ---------------------------------------------------------------

    fn fill_recent(&self) {
        self.store.remove_all();
        let items: Vec<gio::FileInfo> = gtk::RecentManager::default()
            .items()
            .into_iter()
            .filter(|i| i.exists())
            .take(200)
            .filter_map(|i| info_for(&gio::File::for_uri(&i.uri())))
            .collect();
        self.store.extend_from_slice(&items);
    }

    fn fill_trash(&self) {
        self.store.remove_all();
        let items: Vec<gio::FileInfo> = trash::list()
            .into_iter()
            .filter_map(|t| {
                let i = info_for(&gio::File::for_path(&t.path))?;
                // Show the original name and remember where it came from.
                if let Some(n) = t.info.path.file_name() {
                    i.set_display_name(&n.to_string_lossy());
                }
                i.set_attribute_string("m3::orig-path", &t.info.path.to_string_lossy());
                Some(i)
            })
            .collect();
        self.store.extend_from_slice(&items);
    }

    fn reload(self: &Rc<Self>) {
        let loc = self.location.borrow().clone();
        match loc {
            Location::Folder(_) => {
                // Re-reading the folder: reset the directory list.
                let f = self.dir_list.file();
                self.dir_list.set_file(gio::File::NONE);
                self.dir_list.set_file(f.as_ref());
            }
            Location::Recent => self.fill_recent(),
            Location::Trash => self.fill_trash(),
            Location::Search { .. } => {}
        }
        self.sidebar.rebuild();
        self.update_status();
    }

    // ---- search ----------------------------------------------------------------

    fn stop_search(&self) {
        if let Some(s) = self.search.borrow_mut().take() {
            s.stop.store(true, Ordering::Relaxed);
        }
    }

    fn start_search(self: &Rc<Self>, base: gio::File, query: String) {
        self.stop_search();
        let Some(root) = base.path() else {
            self.toast("Search works in local folders.", None);
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let found: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let hidden = self.show_hidden.get();
        {
            let (stop, found, done, q) = (stop.clone(), found.clone(), done.clone(), query.clone());
            std::thread::spawn(move || {
                search_walk(&root, &q, hidden, &stop, &found);
                done.store(true, Ordering::Relaxed);
            });
        }
        *self.search.borrow_mut() = Some(SearchJob { stop, found, done });
        let generation = self.search_gen.get() + 1;
        self.search_gen.set(generation);
        self.loading.set_visible(true);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(120), move || {
            let Some(w) = weak.upgrade() else { return glib::ControlFlow::Break };
            if w.search_gen.get() != generation {
                return glib::ControlFlow::Break;
            }
            let (batch, finished) = match &*w.search.borrow() {
                Some(s) => (
                    s.found.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default(),
                    s.done.load(Ordering::Relaxed),
                ),
                None => (Vec::new(), true),
            };
            let infos: Vec<gio::FileInfo> = batch.iter().filter_map(|p| info_for(&gio::File::for_path(p))).collect();
            if !infos.is_empty() {
                w.store.extend_from_slice(&infos);
            }
            if finished {
                w.loading.set_visible(false);
                w.update_status();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    // ---- status ----------------------------------------------------------------

    fn update_status(&self) {
        let empty = self.sort_model.n_items() == 0;
        let loading = matches!(*self.location.borrow(), Location::Folder(_)) && self.dir_list.is_loading();
        self.loading
            .set_visible(loading || self.search.borrow().as_ref().is_some_and(|s| !s.done.load(Ordering::Relaxed)));
        if let Some(e) = self.dir_list.error().filter(|_| matches!(*self.location.borrow(), Location::Folder(_))) {
            let (title, body, sym) = if e.matches(gio::IOErrorEnum::PermissionDenied) {
                ("No access", "You don't have permission to view this folder.", "lock")
            } else if e.matches(gio::IOErrorEnum::NotFound) {
                ("Folder not found", "This folder doesn't exist anymore.", "folder_off")
            } else {
                ("Can't show this folder", e.message(), "error")
            };
            self.set_status(sym, title, body);
            return;
        }
        if !empty || loading {
            self.status.set_visible(false);
            return;
        }
        let (sym, title, body) = match &*self.location.borrow() {
            Location::Folder(_) if !self.show_hidden.get() && self.dir_list.n_items() > 0 => {
                ("visibility_off", "Only hidden files", "Press Ctrl+H to show hidden files.")
            }
            Location::Folder(_) => ("folder_open", "This folder is empty", "Drop files here or paste with Ctrl+V."),
            Location::Recent => ("schedule", "No recent files", "Files you open in apps show up here."),
            Location::Trash => {
                ("delete_outline", "The trash is empty", "Deleted files appear here until you empty it.")
            }
            Location::Search { .. } => {
                ("search_off", "No results", "Try a different name, or search a broader folder.")
            }
        };
        self.set_status(sym, title, body);
    }

    fn set_status(&self, sym: &str, title: &str, body: &str) {
        self.status_icon.set_icon(sym);
        self.status_title.set_text(title);
        self.status_body.set_text(body);
        if !self.status.is_visible() {
            m3_widgets::animate(&self.status, "m3-enter-up");
        }
        self.status.set_visible(true);
    }

    // ---- selection & operations ---------------------------------------------------

    pub fn selected_infos(&self) -> Vec<gio::FileInfo> {
        let set = self.selection.selection();
        let mut out = Vec::new();
        let n = set.size();
        for k in 0..n {
            let pos = set.nth(k as u32);
            if let Some(i) = self.selection.item(pos).and_downcast::<gio::FileInfo>() {
                out.push(i);
            }
        }
        out
    }

    pub fn selected_files(&self) -> Vec<gio::File> {
        self.selected_infos().iter().filter_map(file_of).collect()
    }

    fn select_uris(&self, uris: &[String]) -> bool {
        let mut any = false;
        let mut first = None;
        for pos in 0..self.selection.n_items() {
            if let Some(f) = self.selection.item(pos).and_downcast::<gio::FileInfo>().and_then(|i| file_of(&i)) {
                if uris.iter().any(|u| *u == f.uri().as_str()) {
                    self.selection.select_item(pos, !any);
                    first.get_or_insert(pos);
                    any = true;
                }
            }
        }
        if let Some(p) = first {
            if self.views.visible_child_name().as_deref() == Some("list") {
                self.list.scroll_to(p, None::<&gtk::ColumnViewColumn>, gtk::ListScrollFlags::FOCUS, None);
            } else {
                self.grid.scroll_to(p, gtk::ListScrollFlags::FOCUS, None);
            }
        }
        any
    }

    fn run(self: &Rc<Self>, job: Job) {
        let weak = Rc::downgrade(self);
        let kind_trash = matches!(job, Job::Trash(_));
        self.ops.start(job, move |out: &Outcome| {
            let Some(w) = weak.upgrade() else { return };
            if let Some(e) = out.errors.first() {
                let more =
                    if out.errors.len() > 1 { format!(" (and {} more)", out.errors.len() - 1) } else { String::new() };
                w.toast(&format!("{e}{more}"), None);
            } else if kind_trash && !out.trashed.is_empty() {
                let n = out.trashed.len();
                let msg = if n == 1 {
                    format!(
                        "“{}” moved to the trash",
                        out.trashed[0].file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                    )
                } else {
                    format!("{n} items moved to the trash")
                };
                let paths = out.trashed.clone();
                let w2 = Rc::downgrade(&w);
                w.toast(
                    &msg,
                    Some(Box::new(move || {
                        let items: Vec<trash::TrashItem> = paths.iter().filter_map(|p| trash::find_latest(p)).collect();
                        if let Some(w) = w2.upgrade() {
                            w.run(Job::Restore(items));
                        }
                    })),
                );
            } else if out.cancelled {
                w.toast("Cancelled.", None);
            }
            if !out.created.is_empty() {
                let uris: Vec<String> = out.created.iter().map(|f| f.uri().to_string()).collect();
                if !w.select_uris(&uris) {
                    *w.pending_select.borrow_mut() = uris;
                }
            }
            if matches!(*w.location.borrow(), Location::Trash | Location::Recent) {
                w.reload();
            } else {
                w.sidebar.rebuild();
            }
        });
    }

    pub fn toast(&self, msg: &str, undo: Option<Box<dyn Fn()>>) {
        self.snack_label.set_text(msg);
        self.snack_action.set_visible(undo.is_some());
        *self.snack_cb.borrow_mut() = undo;
        self.snackbar.set_reveal_child(true);
        if let Some(t) = self.snack_timer.borrow_mut().take() {
            t.remove();
        }
        let sb = self.snackbar.clone();
        let slot = self.snack_timer.clone();
        let timer = glib::timeout_add_local_once(Duration::from_secs(6), move || {
            // This source is finished; forget it so it's never removed twice.
            slot.borrow_mut().take();
            sb.set_reveal_child(false);
        });
        *self.snack_timer.borrow_mut() = Some(timer);
    }

    fn open_selected(self: &Rc<Self>) {
        let infos = self.selected_infos();
        if infos.len() == 1 && infos[0].file_type() == gio::FileType::Directory {
            if let Some(f) = file_of(&infos[0]) {
                if matches!(*self.location.borrow(), Location::Trash) {
                    return; // trashed folders can't be browsed in place
                }
                self.navigate(Location::Folder(f), true);
            }
            return;
        }
        for i in infos {
            let Some(f) = file_of(&i) else { continue };
            if i.file_type() == gio::FileType::Directory {
                if let Some(app) = self.window.application() {
                    FilesWindow::open(&app, Location::Folder(f), None);
                }
                continue;
            }
            let launcher = gtk::FileLauncher::new(Some(&f));
            let weak = Rc::downgrade(self);
            let name = i.display_name().to_string();
            launcher.launch(Some(&self.window), gio::Cancellable::NONE, move |r| {
                if let (Err(e), Some(w)) = (r, weak.upgrade()) {
                    if !e.matches(gtk::DialogError::Dismissed) {
                        w.toast(&format!("Can't open “{name}”: {}", e.message()), None);
                    }
                }
            });
        }
    }

    fn open_with(self: &Rc<Self>) {
        let infos = self.selected_infos();
        let Some(first) = infos.first() else { return };
        let ct = first.content_type().map(|c| c.to_string()).unwrap_or_default();
        let files: Vec<gio::File> = infos.iter().filter_map(file_of).collect();
        let (w, content, buttons) = dialogs::dialog(self.window.upcast_ref(), "Open with", None);
        let list = gtk::ListBox::new();
        list.add_css_class("m3-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let default = gio::AppInfo::default_for_type(&ct, false);
        let mut apps = gio::AppInfo::all_for_type(&ct);
        if apps.is_empty() {
            apps = gio::AppInfo::all().into_iter().filter(|a| a.should_show()).collect();
        }
        for app in apps {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let img = app.icon().map(|i| gtk::Image::from_gicon(&i)).unwrap_or_else(gtk::Image::new);
            img.set_pixel_size(32);
            row.append(&img);
            let l = gtk::Label::new(Some(&app.display_name()));
            l.set_xalign(0.0);
            l.set_hexpand(true);
            row.append(&l);
            if default.as_ref().is_some_and(|d| d.equal(&app)) {
                let tag = gtk::Label::new(Some("Default"));
                tag.add_css_class("m3-files-count");
                row.append(&tag);
            }
            let b = gtk::Button::new();
            b.set_child(Some(&row));
            b.add_css_class("flat");
            b.add_css_class("m3-files-app");
            let (files, ww, weak) = (files.clone(), w.clone(), Rc::downgrade(self));
            b.connect_clicked(move |b| {
                ww.close();
                let ctx = b.display().app_launch_context();
                if let Err(e) = app.launch(&files, Some(&ctx)) {
                    if let Some(win) = weak.upgrade() {
                        win.toast(&format!("Can't open: {}", e.message()), None);
                    }
                }
            });
            list.append(&b);
        }
        let sc = gtk::ScrolledWindow::new();
        sc.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sc.set_max_content_height(360);
        sc.set_propagate_natural_height(true);
        sc.set_child(Some(&list));
        content.append(&sc);
        let cancel = dialogs::button("Cancel", "text");
        let ww = w.clone();
        cancel.connect_clicked(move |_| ww.close());
        buttons.append(&cancel);
        w.present();
    }

    fn current_folder(&self) -> Option<gio::File> {
        match &*self.location.borrow() {
            Location::Folder(f) => Some(f.clone()),
            _ => None,
        }
    }

    fn new_folder(self: &Rc<Self>) {
        let Some(dir) = self.current_folder() else { return };
        let d2 = dir.clone();
        let weak = Rc::downgrade(self);
        let start = util::keep_both_name("New Folder", |c| dir.child(c).query_exists(gio::Cancellable::NONE));
        let start =
            if dir.child("New Folder").query_exists(gio::Cancellable::NONE) { start } else { "New Folder".into() };
        dialogs::ask_name(
            self.window.upcast_ref(),
            "New folder",
            "Create",
            &start,
            false,
            true,
            move |n| d2.child(n).query_exists(gio::Cancellable::NONE),
            move |name| {
                let Some(w) = weak.upgrade() else { return };
                let f = dir.child(&name);
                let weak = Rc::downgrade(&w);
                glib::spawn_future_local(async move {
                    let r = f.make_directory_future(glib::Priority::DEFAULT).await;
                    let Some(w) = weak.upgrade() else { return };
                    match r {
                        Ok(()) => {
                            let uri = f.uri().to_string();
                            if !w.select_uris(std::slice::from_ref(&uri)) {
                                w.pending_select.borrow_mut().push(uri);
                            }
                        }
                        Err(e) => w.toast(&format!("Can't create “{name}”: {}", e.message()), None),
                    }
                });
            },
        );
    }

    fn rename(self: &Rc<Self>) {
        let infos = self.selected_infos();
        let [info] = infos.as_slice() else { return };
        let Some(file) = file_of(info) else { return };
        let Some(parent) = file.parent() else { return };
        let name = info.display_name().to_string();
        let is_dir = info.file_type() == gio::FileType::Directory;
        let weak = Rc::downgrade(self);
        dialogs::ask_name(
            self.window.upcast_ref(),
            if is_dir { "Rename folder" } else { "Rename file" },
            "Rename",
            &name,
            !is_dir,
            false,
            move |n| parent.child(n).query_exists(gio::Cancellable::NONE),
            move |new| {
                let file = file.clone();
                let weak = weak.clone();
                glib::spawn_future_local(async move {
                    let r = file.set_display_name_future(&new, glib::Priority::DEFAULT).await;
                    let Some(w) = weak.upgrade() else { return };
                    match r {
                        Ok(f) => {
                            let uri = f.uri().to_string();
                            if !w.select_uris(std::slice::from_ref(&uri)) {
                                w.pending_select.borrow_mut().push(uri);
                            }
                        }
                        Err(e) => w.toast(&format!("Can't rename: {}", e.message()), None),
                    }
                });
            },
        );
    }

    fn trash_selected(self: &Rc<Self>) {
        if matches!(*self.location.borrow(), Location::Trash) {
            self.delete_selected();
            return;
        }
        let files = self.selected_files();
        if !files.is_empty() {
            self.run(Job::Trash(files));
        }
    }

    fn delete_selected(self: &Rc<Self>) {
        let infos = self.selected_infos();
        if infos.is_empty() {
            return;
        }
        let files: Vec<gio::File> = infos.iter().filter_map(file_of).collect();
        let title = if infos.len() == 1 {
            format!("Delete “{}” permanently?", infos[0].display_name())
        } else {
            format!("Delete {} items permanently?", infos.len())
        };
        let weak = Rc::downgrade(self);
        dialogs::confirm(self.window.upcast_ref(), &title, "This can't be undone.", "Delete", move || {
            if let Some(w) = weak.upgrade() {
                w.run(Job::Delete(files.clone()));
            }
        });
    }

    fn restore_selected(self: &Rc<Self>) {
        let paths: Vec<PathBuf> = self.selected_files().iter().filter_map(|f| f.path()).collect();
        let items: Vec<trash::TrashItem> = trash::list().into_iter().filter(|t| paths.contains(&t.path)).collect();
        if !items.is_empty() {
            self.run(Job::Restore(items));
        }
    }

    fn empty_trash(self: &Rc<Self>) {
        let n = trash::count();
        if n == 0 {
            return;
        }
        let weak = Rc::downgrade(self);
        dialogs::confirm(
            self.window.upcast_ref(),
            "Empty the trash?",
            &format!("{n} item{} will be deleted permanently.", if n == 1 { "" } else { "s" }),
            "Empty trash",
            move || {
                if let Some(w) = weak.upgrade() {
                    w.run(Job::EmptyTrash);
                }
            },
        );
    }

    // ---- clipboard ------------------------------------------------------------------

    fn copy_to_clipboard(&self, cut: bool) {
        let files = self.selected_files();
        if files.is_empty() {
            return;
        }
        if let Err(e) = self.window.clipboard().set_content(Some(&files_content(&files, cut))) {
            self.toast(&format!("Can't use the clipboard: {e}"), None);
            return;
        }
        let n = files.len();
        self.toast(
            &format!(
                "{} {} — paste with Ctrl+V",
                if n == 1 { "1 item".to_owned() } else { format!("{n} items") },
                if cut { "cut" } else { "copied" }
            ),
            None,
        );
    }

    fn paste(self: &Rc<Self>) {
        let Some(dest) = self.current_folder() else { return };
        let clip = self.window.clipboard();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Some((cut, files)) = read_clipboard_files(&clip).await else {
                if let Some(w) = weak.upgrade() {
                    w.toast("There are no files on the clipboard.", None);
                }
                return;
            };
            let Some(w) = weak.upgrade() else { return };
            if files.is_empty() {
                return;
            }
            w.run(if cut { Job::Move { sources: files, dest } } else { Job::Copy { sources: files, dest } });
            if cut {
                // The files moved; the clipboard no longer points at anything.
                let _ = clip.set_content(None::<&gdk::ContentProvider>);
            }
        });
    }

    fn bookmark_current(&self) {
        let target = self
            .selected_infos()
            .into_iter()
            .find(|i| i.file_type() == gio::FileType::Directory)
            .and_then(|i| file_of(&i))
            .or_else(|| self.current_folder());
        let Some(f) = target else { return };
        let uri = f.uri().to_string();
        let mut marks = sidebar::load_bookmarks();
        if marks.iter().any(|m| m.uri == uri) {
            self.toast("Already bookmarked.", None);
            return;
        }
        marks.push(util::Bookmark { uri, label: None });
        match sidebar::save_bookmarks(&marks) {
            Ok(()) => {
                self.sidebar.rebuild();
                self.toast(
                    &format!(
                        "Bookmarked “{}”",
                        f.basename().map(|b| b.to_string_lossy().into_owned()).unwrap_or_default()
                    ),
                    None,
                );
            }
            Err(e) => self.toast(&format!("Can't save bookmarks: {e}"), None),
        }
    }

    // ---- menus -------------------------------------------------------------------------

    fn popup_menu(&self, from: &gtk::Widget, x: f64, y: f64, on_item: bool) {
        let loc = self.location.borrow().clone();
        let m = gio::Menu::new();
        if on_item {
            let infos = self.selected_infos();
            let single_dir = infos.len() == 1 && infos[0].file_type() == gio::FileType::Directory;
            if let Location::Trash = loc {
                m.append(Some("Restore"), Some("win.restore"));
                m.append(Some("Delete permanently"), Some("win.delete"));
            } else {
                let open = gio::Menu::new();
                open.append(Some("Open"), Some("win.open"));
                if !single_dir {
                    open.append(Some("Open with…"), Some("win.open-with"));
                }
                if matches!(loc, Location::Search { .. } | Location::Recent) {
                    open.append(Some("Show in folder"), Some("win.show-in-folder"));
                }
                m.append_section(None, &open);
                let edit = gio::Menu::new();
                edit.append(Some("Cut"), Some("win.cut"));
                edit.append(Some("Copy"), Some("win.copy"));
                edit.append(Some("Rename…"), Some("win.rename"));
                m.append_section(None, &edit);
                let del = gio::Menu::new();
                del.append(Some("Move to trash"), Some("win.trash"));
                del.append(Some("Delete permanently…"), Some("win.delete"));
                m.append_section(None, &del);
                if single_dir {
                    m.append(Some("Add bookmark"), Some("win.bookmark"));
                }
            }
            m.append(Some("Properties"), Some("win.properties"));
        } else {
            match loc {
                Location::Trash => m.append(Some("Empty trash…"), Some("win.empty-trash")),
                Location::Folder(_) => {
                    m.append(Some("New folder…"), Some("win.new-folder"));
                    m.append(Some("Paste"), Some("win.paste"));
                    m.append(Some("Select all"), Some("win.select-all"));
                    let s = gio::Menu::new();
                    s.append(Some("Show hidden files"), Some("win.show-hidden"));
                    s.append(Some("Add bookmark"), Some("win.bookmark"));
                    s.append(Some("Properties"), Some("win.folder-properties"));
                    m.append_section(None, &s);
                }
                _ => m.append(Some("Select all"), Some("win.select-all")),
            }
        }
        self.menu.set_menu_model(Some(&m));
        let p = from.compute_point(&self.overlay, &gtk::graphene::Point::new(x as f32, y as f32));
        let (px, py) = p.map_or((x as i32, y as i32), |p| (p.x() as i32, p.y() as i32));
        self.menu.set_pointing_to(Some(&gdk::Rectangle::new(px, py, 1, 1)));
        self.menu.popup();
    }

    // ---- setup -------------------------------------------------------------------------

    fn setup_actions(self: &Rc<Self>, view_toggle: &gtk::Button) {
        let add = |name: &str, f: WinAction| {
            let a = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            a.connect_activate(move |_, _| {
                if let Some(w) = weak.upgrade() {
                    f(&w);
                }
            });
            self.window.add_action(&a);
        };
        add("back", Box::new(|w| w.go_back()));
        add("forward", Box::new(|w| w.go_forward()));
        add("up", Box::new(|w| w.go_up()));
        add("home", Box::new(|w| w.navigate(Location::Folder(gio::File::for_path(glib::home_dir())), true)));
        add("location", Box::new(|w| w.show_location_entry()));
        add(
            "search",
            Box::new(|w| {
                w.search_bar.set_reveal_child(true);
                w.search_entry.grab_focus();
            }),
        );
        add("open", Box::new(|w| w.open_selected()));
        add("open-with", Box::new(|w| w.open_with()));
        add(
            "show-in-folder",
            Box::new(|w| {
                if let Some(f) = w.selected_files().into_iter().next() {
                    if let Some(p) = f.parent() {
                        w.pending_select.borrow_mut().push(f.uri().to_string());
                        w.navigate(Location::Folder(p), true);
                    }
                }
            }),
        );
        add("new-folder", Box::new(|w| w.new_folder()));
        add("rename", Box::new(|w| w.rename()));
        add("copy", Box::new(|w| w.copy_to_clipboard(false)));
        add("cut", Box::new(|w| w.copy_to_clipboard(true)));
        add("paste", Box::new(|w| w.paste()));
        add("trash", Box::new(|w| w.trash_selected()));
        add("delete", Box::new(|w| w.delete_selected()));
        add("restore", Box::new(|w| w.restore_selected()));
        add("empty-trash", Box::new(|w| w.empty_trash()));
        add(
            "properties",
            Box::new(|w| {
                if let Some(f) = w.selected_files().into_iter().next() {
                    dialogs::properties(w.window.upcast_ref(), &f);
                }
            }),
        );
        add(
            "folder-properties",
            Box::new(|w| {
                if let Some(f) = w.current_folder() {
                    dialogs::properties(w.window.upcast_ref(), &f);
                }
            }),
        );
        add(
            "select-all",
            Box::new(|w| {
                w.selection.select_all();
            }),
        );
        add("refresh", Box::new(|w| w.reload()));
        add("bookmark", Box::new(|w| w.bookmark_current()));
        add("close", Box::new(|w| w.window.close()));
        let vt = view_toggle.clone();
        add(
            "toggle-view",
            Box::new(move |w| {
                let list = w.views.visible_child_name().as_deref() != Some("list");
                w.views.set_visible_child_name(if list { "list" } else { "grid" });
                if let Some(l) = vt.child().and_downcast::<gtk::Label>() {
                    l.set_text(if list { "grid_view" } else { "view_list" });
                }
                vt.set_tooltip_text(Some(if list { "Grid view" } else { "List view" }));
            }),
        );

        // Stateful view options.
        let hidden = gio::SimpleAction::new_stateful("show-hidden", None, &false.to_variant());
        let weak = Rc::downgrade(self);
        hidden.connect_change_state(move |a, v| {
            let Some(on) = v.and_then(|v| v.get::<bool>()) else { return };
            a.set_state(&on.to_variant());
            if let Some(w) = weak.upgrade() {
                w.show_hidden.set(on);
                w.filter.changed(gtk::FilterChange::Different);
                w.update_status();
            }
        });
        self.window.add_action(&hidden);
        let key = gio::SimpleAction::new_stateful("sort", Some(glib::VariantTy::STRING), &"name".to_variant());
        let weak = Rc::downgrade(self);
        key.connect_change_state(move |a, v| {
            let Some(k) = v.and_then(|v| v.get::<String>()).and_then(|s| SortKey::from_id(&s)) else { return };
            a.set_state(&k.id().to_variant());
            if let Some(w) = weak.upgrade() {
                let mut s = w.sort.get();
                s.key = k;
                w.sort.set(s);
                w.sorter.changed(gtk::SorterChange::Different);
            }
        });
        self.window.add_action(&key);
        for (name, flag) in [("sort-desc", 0), ("folders-first", 1)] {
            let init = flag != 0;
            let a = gio::SimpleAction::new_stateful(name, None, &init.to_variant());
            let weak = Rc::downgrade(self);
            a.connect_change_state(move |a, v| {
                let Some(on) = v.and_then(|v| v.get::<bool>()) else { return };
                a.set_state(&on.to_variant());
                if let Some(w) = weak.upgrade() {
                    let mut s = w.sort.get();
                    if flag == 0 {
                        s.desc = on;
                    } else {
                        s.folders_first = on;
                    }
                    w.sort.set(s);
                    w.sorter.changed(gtk::SorterChange::Different);
                }
            });
            self.window.add_action(&a);
        }
    }

    fn setup_input(self: &Rc<Self>, gs: &gtk::ScrolledWindow, ls: &gtk::ScrolledWindow) {
        // File shortcuts on the window, in the bubble phase: text fields
        // (location, search, dialogs) handle their own keys first.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(w) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let alt = state.contains(gdk::ModifierType::ALT_MASK);
            let act = |name: &str| {
                WidgetExt::activate_action(&w.window, name, None).ok();
                glib::Propagation::Stop
            };
            match key {
                gdk::Key::Delete | gdk::Key::KP_Delete if shift => act("win.delete"),
                gdk::Key::Delete | gdk::Key::KP_Delete => act("win.trash"),
                gdk::Key::F2 => act("win.rename"),
                gdk::Key::BackSpace if !ctrl => act("win.up"),
                gdk::Key::c if ctrl => act("win.copy"),
                gdk::Key::x if ctrl => act("win.cut"),
                gdk::Key::v if ctrl => act("win.paste"),
                gdk::Key::a if ctrl => act("win.select-all"),
                gdk::Key::Return | gdk::Key::KP_Enter if alt => act("win.properties"),
                k if !ctrl && !alt => match k.to_unicode() {
                    Some(c) if !c.is_control() && !(c == ' ' && w.typeahead.borrow().0.is_empty()) => {
                        w.type_ahead(c);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                },
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(keys);
        for scroll in [gs, ls] {
            // Background: right click, drop target, click on empty space.
            let click = gtk::GestureClick::new();
            click.set_button(3);
            let weak = Rc::downgrade(self);
            click.connect_pressed(move |g, _, x, y| {
                let Some(w) = weak.upgrade() else { return };
                w.selection.unselect_all();
                if let Some(widget) = g.widget() {
                    w.popup_menu(&widget, x, y, false);
                }
            });
            scroll.add_controller(click);
            let drop =
                gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY | gdk::DragAction::MOVE);
            let weak = Rc::downgrade(self);
            drop.connect_drop(move |t, value, _, _| {
                let Some(w) = weak.upgrade() else { return false };
                let Some(dest) = w.current_folder() else { return false };
                w.handle_drop(t, value, &dest)
            });
            scroll.add_controller(drop);
        }
        // Rubber-band selection only from empty space: a press on an item
        // starts a file drag instead (GTK would otherwise rubber-band from
        // anywhere and items could never be dragged).
        for view in [self.grid.clone().upcast::<gtk::Widget>(), self.list.clone().upcast()] {
            let press = gtk::GestureClick::new();
            press.set_propagation_phase(gtk::PropagationPhase::Capture);
            press.connect_pressed(|g, _, x, y| {
                let Some(v) = g.widget() else { return };
                let on_item = v.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| {
                    let mut cur = Some(w);
                    while let Some(c) = cur {
                        if c.has_css_class("m3-files-tile") || c.has_css_class("m3-files-row") {
                            return true;
                        }
                        cur = c.parent();
                    }
                    false
                });
                if let Some(grid) = v.downcast_ref::<gtk::GridView>() {
                    grid.set_enable_rubberband(!on_item);
                } else if let Some(list) = v.downcast_ref::<gtk::ColumnView>() {
                    list.set_enable_rubberband(!on_item);
                }
            });
            view.add_controller(press);
        }
        let weak = Rc::downgrade(self);
        self.grid.connect_activate(move |_, pos| {
            if let Some(w) = weak.upgrade() {
                w.selection.select_item(pos, true);
                w.open_selected();
            }
        });
        let weak = Rc::downgrade(self);
        self.list.connect_activate(move |_, pos| {
            if let Some(w) = weak.upgrade() {
                w.selection.select_item(pos, true);
                w.open_selected();
            }
        });
        // Mouse back/forward buttons.
        let nav = gtk::GestureClick::new();
        nav.set_button(0);
        let weak = Rc::downgrade(self);
        nav.connect_pressed(move |g, _, _, _| {
            let Some(w) = weak.upgrade() else { return };
            match g.current_button() {
                8 => w.go_back(),
                9 => w.go_forward(),
                _ => {}
            }
        });
        self.window.add_controller(nav);
    }

    fn type_ahead(self: &Rc<Self>, c: char) {
        let prefix = {
            let mut t = self.typeahead.borrow_mut();
            t.0.push(c);
            if let Some(id) = t.1.take() {
                id.remove();
            }
            let weak = Rc::downgrade(self);
            t.1 = Some(glib::timeout_add_local_once(Duration::from_millis(900), move || {
                if let Some(w) = weak.upgrade() {
                    let mut t = w.typeahead.borrow_mut();
                    t.0.clear();
                    t.1 = None;
                }
            }));
            t.0.to_lowercase()
        };
        for pos in 0..self.sort_model.n_items() {
            if let Some(i) = self.sort_model.item(pos).and_downcast::<gio::FileInfo>() {
                if i.display_name().to_lowercase().starts_with(&prefix) {
                    self.selection.select_item(pos, true);
                    if self.views.visible_child_name().as_deref() == Some("list") {
                        self.list.scroll_to(pos, None::<&gtk::ColumnViewColumn>, gtk::ListScrollFlags::FOCUS, None);
                    } else {
                        self.grid.scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
                    }
                    return;
                }
            }
        }
    }

    fn setup_signals(self: &Rc<Self>, search_toggle: &gtk::ToggleButton, crumb_scroll: &gtk::ScrolledWindow) {
        // Status and pending selections follow the model.
        let weak = Rc::downgrade(self);
        self.sort_model.connect_items_changed(move |_, _, _, _| {
            let Some(w) = weak.upgrade() else { return };
            let pending = w.pending_select.borrow().clone();
            if !pending.is_empty() && w.select_uris(&pending) {
                w.pending_select.borrow_mut().clear();
            }
            w.update_status();
        });
        let weak = Rc::downgrade(self);
        self.dir_list.connect_loading_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_status();
            }
        });
        let weak = Rc::downgrade(self);
        self.dir_list.connect_error_notify(move |_| {
            if let Some(w) = weak.upgrade() {
                w.update_status();
            }
        });

        // Path bar: click on empty space (or Ctrl+L) edits the location.
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |g, _, x, y| {
            let Some(w) = weak.upgrade() else { return };
            let on_button = g
                .widget()
                .and_then(|wd| wd.pick(x, y, gtk::PickFlags::DEFAULT))
                .is_some_and(|p| p.ancestor(gtk::Button::static_type()).is_some());
            if !on_button && w.location.borrow().folder().is_some() {
                w.show_location_entry();
            }
        });
        crumb_scroll.add_controller(click);
        let weak = Rc::downgrade(self);
        self.location_entry.connect_activate(move |_| {
            if let Some(w) = weak.upgrade() {
                w.go_typed_location();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Escape {
                if let Some(w) = weak.upgrade() {
                    w.path_stack.set_visible_child_name("crumbs");
                    w.grid.grab_focus();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.location_entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(w) = weak.upgrade() {
                w.path_stack.set_visible_child_name("crumbs");
            }
        });
        self.location_entry.add_controller(focus);

        // Search.
        let weak = Rc::downgrade(self);
        search_toggle.connect_toggled(move |t| {
            let Some(w) = weak.upgrade() else { return };
            w.search_bar.set_reveal_child(t.is_active());
            if t.is_active() {
                w.search_entry.grab_focus();
            } else {
                w.search_entry.set_text("");
            }
        });
        let st = search_toggle.clone();
        let weak = Rc::downgrade(self);
        self.search_bar.connect_reveal_child_notify(move |r| {
            st.set_active(r.reveals_child());
            if let Some(w) = weak.upgrade() {
                if !r.reveals_child() {
                    w.search_entry.set_text("");
                }
            }
        });
        self.search_entry.set_search_delay(250);
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |e| {
            let Some(w) = weak.upgrade() else { return };
            let q = e.text().trim().to_owned();
            let cur = w.location.borrow().clone();
            match (q.is_empty(), cur) {
                (true, Location::Search { base, .. }) => w.navigate(Location::Folder(base), false),
                (true, _) => {}
                (false, Location::Search { base, .. }) => w.navigate(Location::Search { base, query: q }, false),
                (false, Location::Folder(base)) => w.navigate(Location::Search { base, query: q }, true),
                (false, _) => {
                    let home = gio::File::for_path(glib::home_dir());
                    w.navigate(Location::Search { base: home, query: q }, true);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_stop_search(move |_| {
            if let Some(w) = weak.upgrade() {
                w.search_bar.set_reveal_child(false);
                w.grid.grab_focus();
            }
        });
    }
}

type WinAction = Box<dyn Fn(&Rc<FilesWindow>)>;

fn display_name(info: &gio::FileInfo) -> String {
    info.display_name().to_string()
}

fn same_filesystem(a: &gio::File, b: &gio::File) -> bool {
    let fs = |f: &gio::File| {
        f.query_info("id::filesystem", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            .ok()
            .and_then(|i| i.attribute_string("id::filesystem").map(|s| s.to_string()))
    };
    matches!((fs(a), fs(b)), (Some(x), Some(y)) if x == y)
}

/// Clipboard / drag content for files: GTK file list, `text/uri-list` and
/// GNOME's copied-files format (which carries cut vs copy).
fn files_content(files: &[gio::File], cut: bool) -> gdk::ContentProvider {
    let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
    let list = gdk::FileList::from_array(files);
    gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_bytes(
            "x-special/gnome-copied-files",
            &glib::Bytes::from_owned(util::encode_gnome_copied(cut, &uris).into_bytes()),
        ),
        gdk::ContentProvider::for_value(&list.to_value()),
        gdk::ContentProvider::for_bytes(
            "text/uri-list",
            &glib::Bytes::from_owned(util::encode_uri_list(&uris).into_bytes()),
        ),
    ])
}

async fn read_stream(stream: gio::InputStream) -> Option<String> {
    let mut buf = Vec::new();
    loop {
        let bytes = stream.read_bytes_future(64 * 1024, glib::Priority::DEFAULT).await.ok()?;
        if bytes.is_empty() {
            break;
        }
        buf.extend_from_slice(&bytes);
        if buf.len() > 16 * 1024 * 1024 {
            return None;
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Files on the clipboard and whether they were cut.
async fn read_clipboard_files(clip: &gdk::Clipboard) -> Option<(bool, Vec<gio::File>)> {
    let formats = clip.formats();
    if formats.contain_mime_type("x-special/gnome-copied-files") {
        if let Ok((s, _)) = clip.read_future(&["x-special/gnome-copied-files"], glib::Priority::DEFAULT).await {
            if let Some((cut, uris)) = read_stream(s).await.as_deref().and_then(util::decode_gnome_copied) {
                return Some((cut, uris.iter().map(|u| gio::File::for_uri(u)).collect()));
            }
        }
    }
    if formats.contain_mime_type("text/uri-list") {
        if let Ok((s, _)) = clip.read_future(&["text/uri-list"], glib::Priority::DEFAULT).await {
            if let Some(t) = read_stream(s).await {
                return Some((false, util::decode_uri_list(&t).iter().map(|u| gio::File::for_uri(u)).collect()));
            }
        }
    }
    if formats.contains_type(gdk::FileList::static_type()) {
        if let Ok(v) = clip.read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT).await {
            if let Ok(l) = v.get::<gdk::FileList>() {
                return Some((false, l.files()));
            }
        }
    }
    None
}

/// Recursive name search below `root` (worker thread).
fn search_walk(root: &Path, query: &str, hidden: bool, stop: &AtomicBool, found: &Mutex<Vec<PathBuf>>) {
    const LIMIT: usize = 5000;
    let mut stack = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(dir) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if util::search_matches(&name, query) {
                if let Ok(mut v) = found.lock() {
                    v.push(e.path());
                }
                count += 1;
                if count >= LIMIT {
                    return;
                }
            }
            if is_dir {
                stack.push(e.path());
            }
        }
    }
}

fn view_menu() -> gio::Menu {
    let m = gio::Menu::new();
    let sort = gio::Menu::new();
    for (label, key) in [("Name", "name"), ("Size", "size"), ("Modified", "modified"), ("Type", "type")] {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("win.sort"), Some(&key.to_variant()));
        sort.append_item(&item);
    }
    m.append_section(Some("Sort by"), &sort);
    let opts = gio::Menu::new();
    opts.append(Some("Descending"), Some("win.sort-desc"));
    opts.append(Some("Folders first"), Some("win.folders-first"));
    opts.append(Some("Show hidden files"), Some("win.show-hidden"));
    m.append_section(None, &opts);
    let more = gio::Menu::new();
    more.append(Some("New folder…"), Some("win.new-folder"));
    more.append(Some("Bookmark this folder"), Some("win.bookmark"));
    more.append(Some("New window"), Some("app.new-window"));
    m.append_section(None, &more);
    m
}
