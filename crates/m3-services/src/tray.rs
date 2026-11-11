//! System tray: `org.kde.StatusNotifierWatcher` + StatusNotifierHost, and
//! `com.canonical.dbusmenu` menus.
//!
//! The tray runs on its own thread with its own session-bus connection (and
//! a short method timeout), so a slow or hung tray application never delays
//! the other services. Only the shell starts it ([`start`]); other m3
//! programs using [`crate::Services`] don't compete for the watcher name.
//!
//! If another StatusNotifierWatcher already owns the name (another desktop's
//! panel, `snixembed`, …), m3 acts only as a host of that watcher.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{StreamExt, stream::BoxStream};
use tracing::{debug, info, warn};
use zbus::{OwnedVariant, Variant, interface, message::Header, object_server::SignalEmitter};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
/// Preferred icon size (pixels) when an item offers several pixmaps.
const ICON_TARGET: i32 = 48;

// ---- public types ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrayStatus {
    /// Not important right now: hidden from the panel.
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl TrayStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "Passive" => TrayStatus::Passive,
            "NeedsAttention" => TrayStatus::NeedsAttention,
            _ => TrayStatus::Active,
        }
    }
}

/// An icon image converted to straight-alpha RGBA bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayPixmap {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrayItem {
    /// Stable identifier: bus name + object path.
    pub key: String,
    pub id: String,
    pub title: String,
    pub status: TrayStatus,
    pub icon_name: String,
    pub icon_theme_path: String,
    pub icon_pixmap: Option<TrayPixmap>,
    pub attention_icon_name: String,
    pub attention_pixmap: Option<TrayPixmap>,
    /// Tooltip text (title and description), markup stripped.
    pub tooltip: String,
    pub has_menu: bool,
    /// The item only provides a menu: a primary click opens it.
    pub item_is_menu: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuToggle {
    #[default]
    None,
    Checkmark,
    Radio,
}

/// One entry of a dbusmenu tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuNode {
    pub id: i32,
    /// Label with mnemonic underscores removed (`__` becomes `_`).
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub separator: bool,
    pub toggle: MenuToggle,
    /// 1 = on, 0 = off, -1 = indeterminate.
    pub toggle_state: i32,
    pub icon_name: String,
    /// PNG data for the item's icon, if the application sent one.
    pub icon_data: Vec<u8>,
    pub children: Vec<MenuNode>,
}

impl MenuNode {
    pub fn is_submenu(&self) -> bool {
        !self.children.is_empty()
    }
}

#[derive(Debug, Clone)]
pub enum TrayEvent {
    /// The current items (Passive ones included; the UI hides them).
    Items(Vec<TrayItem>),
    /// A menu to show (or to refresh, if it is already open) for `key`.
    Menu { key: String, menu: MenuNode },
}

#[derive(Debug, Clone)]
pub enum TrayCommand {
    /// Primary click. Items that are only menus (or don't implement
    /// `Activate`) answer with a [`TrayEvent::Menu`].
    Activate {
        key: String,
    },
    SecondaryActivate {
        key: String,
    },
    /// Vertical scroll by `delta` steps.
    Scroll {
        key: String,
        delta: i32,
    },
    OpenMenu {
        key: String,
    },
    /// A submenu is about to be shown.
    MenuAboutToShow {
        key: String,
        id: i32,
    },
    MenuClicked {
        key: String,
        id: i32,
    },
    MenuClosed {
        key: String,
    },
}

/// Send commands to the tray thread.
#[derive(Clone)]
pub struct TrayHandle {
    tx: async_channel::Sender<TrayCommand>,
}

impl TrayHandle {
    pub fn send(&self, cmd: TrayCommand) {
        if self.tx.try_send(cmd).is_err() {
            warn!("tray thread is gone; command dropped");
        }
    }
}

/// Start the tray (watcher + host) on its own thread.
pub fn start() -> (TrayHandle, async_channel::Receiver<TrayEvent>) {
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<TrayCommand>();
    let (ev_tx, ev_rx) = async_channel::unbounded::<TrayEvent>();
    let spawned = std::thread::Builder::new().name("m3-tray".into()).spawn(move || {
        zbus::block_on(run(cmd_rx, ev_tx));
    });
    if let Err(e) = spawned {
        warn!("cannot start the tray thread: {e}");
    }
    (TrayHandle { tx: cmd_tx }, ev_rx)
}

// ---- pure helpers (unit-tested) --------------------------------------------------

/// Resolve what an item passed to `RegisterStatusNotifierItem` (or what a
/// watcher reports) into `(bus name, object path)`.
///
/// * `":1.42"` / `"org.kde.StatusNotifierItem-1-1"` → default path
/// * `":1.42/StatusNotifierItem"` → bus + path
/// * `"/org/ayatana/NotificationItem/x"` → the caller's bus + that path
pub fn parse_item_address(service: &str, sender: Option<&str>) -> Option<(String, String)> {
    let service = service.trim();
    if service.is_empty() {
        return None;
    }
    if service.starts_with('/') {
        let sender = sender.filter(|s| !s.is_empty())?;
        return Some((sender.to_owned(), service.to_owned()));
    }
    match service.find('/') {
        Some(i) => {
            let (bus, path) = service.split_at(i);
            (!bus.is_empty()).then(|| (bus.to_owned(), path.to_owned()))
        }
        None => Some((service.to_owned(), DEFAULT_ITEM_PATH.to_owned())),
    }
}

/// Convert ARGB32 in network byte order (as used by StatusNotifierItem) to
/// RGBA. Returns `None` when the data doesn't match the size.
pub fn argb_to_rgba(width: i32, height: i32, data: &[u8]) -> Option<Vec<u8>> {
    if !(1..=1024).contains(&width) || !(1..=1024).contains(&height) {
        return None;
    }
    if data.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    for px in data.chunks_exact(4) {
        out.extend_from_slice(&[px[1], px[2], px[3], px[0]]);
    }
    Some(out)
}

/// Pick the pixmap closest to `target`: the smallest one at least `target`
/// wide, otherwise the largest one. Invalid entries are skipped.
pub fn choose_pixmap(pixmaps: &[(i32, i32, Vec<u8>)], target: i32) -> Option<TrayPixmap> {
    let valid: Vec<&(i32, i32, Vec<u8>)> = pixmaps
        .iter()
        .filter(|(w, h, d)| {
            *w > 0 && *h > 0 && *w <= 1024 && *h <= 1024 && d.len() == (*w as usize) * (*h as usize) * 4
        })
        .collect();
    let best = valid
        .iter()
        .filter(|(w, _, _)| *w >= target)
        .min_by_key(|(w, _, _)| *w)
        .or_else(|| valid.iter().max_by_key(|(w, _, _)| *w))?;
    let (w, h, d) = best;
    Some(TrayPixmap { width: *w, height: *h, rgba: argb_to_rgba(*w, *h, d)? })
}

/// Remove GTK/Qt mnemonic markers: `_File` → `File`, `__` → `_`.
pub fn strip_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                chars.next();
                out.push('_');
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Remove simple markup tags and decode the common entities (tooltips may
/// contain a subset of HTML).
pub fn strip_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A dbusmenu property value, decoded from its D-Bus variant.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Str(String),
    Bool(bool),
    Int(i64),
    Bytes(Vec<u8>),
    Other,
}

/// Build a menu node from decoded properties (defaults per the dbusmenu
/// spec: enabled and visible unless said otherwise).
pub fn build_node(id: i32, props: &HashMap<String, Prop>, children: Vec<MenuNode>) -> MenuNode {
    let s = |k: &str| match props.get(k) {
        Some(Prop::Str(v)) => v.clone(),
        _ => String::new(),
    };
    let b = |k: &str, default: bool| match props.get(k) {
        Some(Prop::Bool(v)) => *v,
        _ => default,
    };
    let toggle = match s("toggle-type").as_str() {
        "checkmark" => MenuToggle::Checkmark,
        "radio" => MenuToggle::Radio,
        _ => MenuToggle::None,
    };
    let toggle_state = match props.get("toggle-state") {
        Some(Prop::Int(v)) => (*v).clamp(-1, 1) as i32,
        _ => 0,
    };
    let icon_data = match props.get("icon-data") {
        Some(Prop::Bytes(v)) => v.clone(),
        _ => Vec::new(),
    };
    let children: Vec<MenuNode> = children.into_iter().filter(|c| c.visible).collect();
    MenuNode {
        id,
        label: strip_mnemonic(&s("label")),
        enabled: b("enabled", true),
        visible: b("visible", true),
        separator: s("type") == "separator",
        toggle,
        toggle_state,
        icon_name: s("icon-name"),
        icon_data,
        children,
    }
}

// ---- variant decoding ------------------------------------------------------------

fn inner<'a, 'b>(v: &'a Variant<'b>) -> &'a Variant<'b> {
    let mut v = v;
    while let Variant::Variant(b) = v {
        v = b;
    }
    v
}

fn v_str(v: &Variant<'_>) -> Option<String> {
    match inner(v) {
        Variant::Str(s) => Some(s.as_str().to_owned()),
        Variant::ObjectPath(p) => Some(p.as_str().to_owned()),
        _ => None,
    }
}

fn v_bool(v: &Variant<'_>) -> Option<bool> {
    match inner(v) {
        Variant::Bool(b) => Some(*b),
        _ => None,
    }
}

fn v_int(v: &Variant<'_>) -> Option<i64> {
    Some(match inner(v) {
        Variant::I32(n) => i64::from(*n),
        Variant::U32(n) => i64::from(*n),
        Variant::I16(n) => i64::from(*n),
        Variant::U16(n) => i64::from(*n),
        Variant::U8(n) => i64::from(*n),
        Variant::I64(n) => *n,
        Variant::U64(n) => i64::try_from(*n).ok()?,
        _ => return None,
    })
}

fn v_bytes(v: &Variant<'_>) -> Option<Vec<u8>> {
    match inner(v) {
        Variant::Array(a) => a
            .inner()
            .iter()
            .map(|e| match e {
                Variant::U8(b) => Some(*b),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// `a(iiay)` icon pixmaps.
fn v_pixmaps(v: &Variant<'_>) -> Vec<(i32, i32, Vec<u8>)> {
    let Variant::Array(a) = inner(v) else { return Vec::new() };
    a.inner()
        .iter()
        .filter_map(|e| {
            let Variant::Structure(s) = inner(e) else { return None };
            let f = s.fields();
            let w = i32::try_from(v_int(f.first()?)?).ok()?;
            let h = i32::try_from(v_int(f.get(1)?)?).ok()?;
            Some((w, h, v_bytes(f.get(2)?)?))
        })
        .collect()
}

fn prop_of(v: &Variant<'_>) -> Prop {
    let v = inner(v);
    if let Some(s) = v_str(v) {
        return Prop::Str(s);
    }
    if let Some(b) = v_bool(v) {
        return Prop::Bool(b);
    }
    if let Some(n) = v_int(v) {
        return Prop::Int(n);
    }
    if let Some(b) = v_bytes(v) {
        return Prop::Bytes(b);
    }
    Prop::Other
}

/// Decode a `(ia{sv}av)` layout node.
fn node_from_variant(v: &Variant<'_>, depth: usize) -> Option<MenuNode> {
    if depth > 16 {
        return None;
    }
    let Variant::Structure(s) = inner(v) else { return None };
    let f = s.fields();
    let id = i32::try_from(v_int(f.first()?)?).ok()?;
    let mut props = HashMap::new();
    if let Variant::Dict(d) = inner(f.get(1)?) {
        for (k, val) in d.iter() {
            if let Some(k) = v_str(k) {
                props.insert(k, prop_of(val));
            }
        }
    }
    let children = match inner(f.get(2)?) {
        Variant::Array(a) => a.inner().iter().filter_map(|c| node_from_variant(c, depth + 1)).collect(),
        _ => Vec::new(),
    };
    Some(build_node(id, &props, children))
}

fn layout_root(id: i32, props: &HashMap<String, OwnedVariant>, children: &[OwnedVariant]) -> MenuNode {
    let props: HashMap<String, Prop> = props.iter().map(|(k, v)| (k.clone(), prop_of(v))).collect();
    let children = children.iter().filter_map(|c| node_from_variant(c, 1)).collect();
    build_node(id, &props, children)
}

fn item_from_props(key: &str, p: &HashMap<String, OwnedVariant>) -> TrayItem {
    let s = |k: &str| p.get(k).and_then(|v| v_str(v)).unwrap_or_default();
    let pix = |k: &str| p.get(k).map(|v| v_pixmaps(v)).and_then(|l| choose_pixmap(&l, ICON_TARGET));
    let mut tooltip = String::new();
    if let Some(Variant::Structure(t)) = p.get("ToolTip").map(|v| inner(v)) {
        let f = t.fields();
        let title = f.get(2).and_then(v_str).unwrap_or_default();
        let descr = f.get(3).and_then(v_str).unwrap_or_default();
        tooltip =
            strip_markup(&[title, descr].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join("\n"));
    }
    let title = s("Title");
    if tooltip.trim().is_empty() {
        tooltip = if title.is_empty() { s("Id") } else { title.clone() };
    }
    let menu = s("Menu");
    TrayItem {
        key: key.to_owned(),
        id: s("Id"),
        title,
        status: TrayStatus::parse(&s("Status")),
        icon_name: s("IconName"),
        icon_theme_path: s("IconThemePath"),
        icon_pixmap: pix("IconPixmap"),
        attention_icon_name: s("AttentionIconName"),
        attention_pixmap: pix("AttentionIconPixmap"),
        tooltip,
        has_menu: !menu.is_empty() && menu != "/",
        item_is_menu: p.get("ItemIsMenu").and_then(|v| v_bool(v)).unwrap_or(false),
    }
}

// ---- the watcher -----------------------------------------------------------------

enum WatcherMsg {
    Registered { bus: String, path: String },
}

struct Watcher {
    /// `bus + path` strings of the registered items.
    items: Arc<Mutex<Vec<String>>>,
    tx: async_channel::Sender<WatcherMsg>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let sender = hdr.sender().map(|s| s.to_string());
        let Some((bus, path)) = parse_item_address(service, sender.as_deref()) else {
            warn!("ignoring tray item with an invalid address `{service}`");
            return;
        };
        let full = format!("{bus}{path}");
        let added = match self.items.lock() {
            Ok(mut items) if !items.contains(&full) => {
                items.push(full.clone());
                true
            }
            _ => false,
        };
        let _ = self.tx.try_send(WatcherMsg::Registered { bus, path });
        if added {
            let _ = Self::status_notifier_item_registered(&emitter, &full).await;
            let _ = self.registered_status_notifier_items_changed(&emitter).await;
        }
    }

    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().map(|i| i.clone()).unwrap_or_default()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

// ---- the host loop ---------------------------------------------------------------

struct ItemState {
    item: TrayItem,
    bus: String,
    /// Unique name of the item's connection (signals come from it).
    owner: String,
    path: String,
    menu_path: Option<String>,
}

enum Wake {
    Command(TrayCommand),
    Watcher(WatcherMsg),
    /// Any StatusNotifierItem signal from (sender, path).
    ItemSignal(String, String),
    /// LayoutUpdated / ItemsPropertiesUpdated from (sender, path).
    MenuSignal(String, String),
    /// NameOwnerChanged(name, new owner).
    NameOwner(String, String),
    /// Host mode: the external watcher (un)registered an item.
    External {
        registered: bool,
        service: String,
    },
}

struct Host {
    conn: zbus::Connection,
    ev: async_channel::Sender<TrayEvent>,
    items: Vec<ItemState>,
    /// The item whose menu is shown.
    open_menu: Option<String>,
    /// We own the watcher (else another watcher does).
    own_watcher: bool,
    registry: Arc<Mutex<Vec<String>>>,
}

async fn signal_messages(
    conn: &zbus::Connection,
    rule: Result<zbus::MatchRule<'static>, zbus::Error>,
) -> Option<zbus::MessageStream> {
    let rule = rule.ok()?;
    zbus::MessageStream::for_match_rule(rule, conn, std::num::NonZeroUsize::new(512)).await.ok()
}

fn sender_path(msg: &zbus::Message) -> Option<(String, String)> {
    let h = msg.header();
    Some((h.sender()?.to_string(), h.path()?.to_string()))
}

async fn run(cmds: async_channel::Receiver<TrayCommand>, ev: async_channel::Sender<TrayEvent>) {
    let conn = match zbus::connection::Builder::session().method_timeout(Duration::from_secs(3)).build().await {
        Ok(c) => c,
        Err(e) => {
            warn!("no session bus ({e}); the system tray is unavailable");
            return;
        }
    };
    let registry = Arc::new(Mutex::new(Vec::new()));
    let (wtx, wrx) = async_channel::unbounded::<WatcherMsg>();
    let watcher = Watcher { items: registry.clone(), tx: wtx };

    let mut own_watcher = false;
    if let Err(e) = conn.object_server().at(WATCHER_PATH, watcher).await {
        warn!("cannot export the tray watcher: {e}");
    } else {
        use zbus::fdo::{RequestNameFlags, RequestNameReply};
        match conn.request_name_with_flags(WATCHER, RequestNameFlags::DO_NOT_QUEUE).await {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                own_watcher = true;
                info!("serving {WATCHER}");
            }
            Ok(_) | Err(_) => {
                info!("another {WATCHER} is running; m3 acts as a tray host only");
                let _ = conn.object_server().remove::<Watcher, _>(WATCHER_PATH).await;
            }
        }
    }
    let host_name = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    if let Err(e) = conn.request_name(host_name.as_str()).await {
        debug!("cannot own {host_name}: {e}");
    }

    let mut streams: Vec<BoxStream<'static, Wake>> =
        vec![cmds.map(Wake::Command).boxed(), wrx.map(Wake::Watcher).boxed()];
    let item_rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(ITEM_IFACE).build();
    if let Some(s) = signal_messages(&conn, item_rule).await {
        streams.push(
            s.filter_map(|m| async move { m.ok().as_ref().and_then(sender_path).map(|(a, b)| Wake::ItemSignal(a, b)) })
                .boxed(),
        );
    }
    let menu_rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(MENU_IFACE).build();
    if let Some(s) = signal_messages(&conn, menu_rule).await {
        streams.push(
            s.filter_map(|m| async move {
                let m = m.ok()?;
                let member = m.header().member()?.to_string();
                (member == "LayoutUpdated" || member == "ItemsPropertiesUpdated")
                    .then(|| sender_path(&m).map(|(a, b)| Wake::MenuSignal(a, b)))
                    .flatten()
            })
            .boxed(),
        );
    }
    let owner_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .interface("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .build();
    if let Some(s) = signal_messages(&conn, owner_rule).await {
        streams.push(
            s.filter_map(|m| async move {
                let (name, _old, new): (String, String, String) = m.ok()?.body().deserialize().ok()?;
                Some(Wake::NameOwner(name, new))
            })
            .boxed(),
        );
    }

    let mut host = Host { conn: conn.clone(), ev, items: Vec::new(), open_menu: None, own_watcher, registry };

    if own_watcher {
        if let Ok(iface) = conn.object_server().interface::<_, Watcher>(WATCHER_PATH).await {
            let _ = Watcher::status_notifier_host_registered(iface.signal_emitter()).await;
        }
    } else {
        // Host of an external watcher: follow its signals and current items.
        let ext_rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(WATCHER).build();
        if let Some(s) = signal_messages(&conn, ext_rule).await {
            streams.push(
                s.filter_map(|m| async move {
                    let m = m.ok()?;
                    let member = m.header().member()?.to_string();
                    let registered = match member.as_str() {
                        "StatusNotifierItemRegistered" => true,
                        "StatusNotifierItemUnregistered" => false,
                        _ => return None,
                    };
                    let service: String = m.body().deserialize().ok()?;
                    Some(Wake::External { registered, service })
                })
                .boxed(),
            );
        }
        if let Ok(w) = zbus::Proxy::new(&conn, WATCHER, WATCHER_PATH, WATCHER).await {
            let _ = w.call_method("RegisterStatusNotifierHost", &(host_name.as_str(),)).await;
            if let Ok(list) = w.get_property::<Vec<String>>("RegisteredStatusNotifierItems").await {
                for service in list {
                    if let Some((bus, path)) = parse_item_address(&service, None) {
                        host.add(&bus, &path).await;
                    }
                }
            }
        }
    }
    host.publish();

    let mut all = futures_util::stream::select_all(streams);
    while let Some(w) = all.next().await {
        match w {
            Wake::Command(c) => host.command(c).await,
            Wake::Watcher(WatcherMsg::Registered { bus, path }) => {
                host.add(&bus, &path).await;
                host.publish();
            }
            Wake::External { registered: true, service } => {
                if let Some((bus, path)) = parse_item_address(&service, None) {
                    host.add(&bus, &path).await;
                    host.publish();
                }
            }
            Wake::External { registered: false, service } => {
                if let Some((bus, path)) = parse_item_address(&service, None) {
                    host.remove(|i| i.bus == bus && i.path == path).await;
                }
            }
            Wake::ItemSignal(sender, path) => host.refresh(&sender, &path).await,
            Wake::MenuSignal(sender, path) => host.menu_changed(&sender, &path).await,
            Wake::NameOwner(name, new_owner) => {
                if new_owner.is_empty() {
                    host.remove(|i| i.bus == name || i.owner == name).await;
                }
            }
        }
    }
}

impl Host {
    fn publish(&self) {
        let items = self.items.iter().map(|i| i.item.clone()).collect();
        let _ = self.ev.try_send(TrayEvent::Items(items));
    }

    async fn fetch(&self, owner: &str, path: &str) -> Option<HashMap<String, OwnedVariant>> {
        let props = zbus::Proxy::new(&self.conn, owner, path, "org.freedesktop.DBus.Properties").await.ok()?;
        match props.call::<_, _, HashMap<String, OwnedVariant>>("GetAll", &(ITEM_IFACE,)).await {
            Ok(p) => Some(p),
            Err(e) => {
                debug!("tray item {owner}{path}: {e}");
                None
            }
        }
    }

    async fn add(&mut self, bus: &str, path: &str) {
        let owner = if bus.starts_with(':') {
            bus.to_owned()
        } else {
            match zbus::fdo::DBusProxy::new(&self.conn).await {
                Ok(dbus) => match bus.try_into() {
                    Ok(name) => match dbus.get_name_owner(name).await {
                        Ok(o) => o.to_string(),
                        Err(e) => {
                            debug!("tray item {bus} has no owner: {e}");
                            return;
                        }
                    },
                    Err(_) => return,
                },
                Err(_) => return,
            }
        };
        if self.items.iter().any(|i| (i.bus == bus || i.owner == owner) && i.path == path) {
            return;
        }
        let Some(props) = self.fetch(&owner, path).await else { return };
        let key = format!("{bus}{path}");
        let item = item_from_props(&key, &props);
        let menu_path = props.get("Menu").and_then(|v| v_str(v)).filter(|m| !m.is_empty() && m != "/");
        info!(id = %item.id, %key, "tray item added");
        self.items.push(ItemState { item, bus: bus.to_owned(), owner, path: path.to_owned(), menu_path });
    }

    async fn remove(&mut self, pred: impl Fn(&ItemState) -> bool) {
        let gone: Vec<String> = self.items.iter().filter(|i| pred(i)).map(|i| format!("{}{}", i.bus, i.path)).collect();
        if gone.is_empty() {
            return;
        }
        self.items.retain(|i| !pred(i));
        if self.own_watcher {
            if let Ok(mut reg) = self.registry.lock() {
                reg.retain(|r| !gone.contains(r));
            }
            if let Ok(iface) = self.conn.object_server().interface::<_, Watcher>(WATCHER_PATH).await {
                for g in &gone {
                    let _ = Watcher::status_notifier_item_unregistered(iface.signal_emitter(), g).await;
                }
                let _ = iface.get().await.registered_status_notifier_items_changed(iface.signal_emitter()).await;
            }
        }
        info!(?gone, "tray items removed");
        self.publish();
    }

    async fn refresh(&mut self, sender: &str, path: &str) {
        let Some(idx) = self.items.iter().position(|i| i.owner == sender && i.path == path) else { return };
        let (owner, key) = (self.items[idx].owner.clone(), self.items[idx].item.key.clone());
        let Some(props) = self.fetch(&owner, path).await else { return };
        let item = item_from_props(&key, &props);
        let menu_path = props.get("Menu").and_then(|v| v_str(v)).filter(|m| !m.is_empty() && m != "/");
        if let Some(st) = self.items.get_mut(idx) {
            if st.item != item || st.menu_path != menu_path {
                st.item = item;
                st.menu_path = menu_path;
                self.publish();
            }
        }
    }

    fn find(&self, key: &str) -> Option<&ItemState> {
        self.items.iter().find(|i| i.item.key == key)
    }

    async fn item_call<B>(&self, key: &str, method: &str, body: &B) -> Result<(), String>
    where
        B: serde::Serialize + zbus::DynamicType,
    {
        let st = self.find(key).ok_or_else(|| "no such tray item".to_owned())?;
        let p = zbus::Proxy::new(&self.conn, st.owner.as_str(), st.path.as_str(), ITEM_IFACE)
            .await
            .map_err(|e| e.to_string())?;
        p.call_method(method, body).await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn menu_proxy(&self, key: &str) -> Option<zbus::Proxy<'static>> {
        let st = self.find(key)?;
        let path = st.menu_path.clone()?;
        zbus::Proxy::new(&self.conn, st.owner.clone(), path, MENU_IFACE).await.ok()
    }

    /// Fetch the whole menu tree and send it to the UI.
    async fn send_menu(&mut self, key: &str, about_to_show: bool) {
        let Some(menu) = self.menu_proxy(key).await else {
            debug!("tray item {key} has no menu");
            return;
        };
        if about_to_show {
            // Lets applications fill the menu lazily; failures are harmless.
            let _ = menu.call_method("AboutToShow", &(0i32,)).await;
        }
        let layout = menu
            .call::<_, _, (u32, (i32, HashMap<String, OwnedVariant>, Vec<OwnedVariant>))>(
                "GetLayout",
                &(0i32, -1i32, Vec::<String>::new()),
            )
            .await;
        match layout {
            Ok((_rev, (id, props, children))) => {
                let root = layout_root(id, &props, &children);
                if about_to_show {
                    let _ = menu.call_method("Event", &(0i32, "opened", Variant::from(0i32), event_time())).await;
                }
                self.open_menu = Some(key.to_owned());
                let _ = self.ev.try_send(TrayEvent::Menu { key: key.to_owned(), menu: root });
            }
            Err(e) => warn!("cannot read the menu of tray item {key}: {e}"),
        }
    }

    async fn menu_changed(&mut self, sender: &str, path: &str) {
        let Some(key) = self.open_menu.clone() else { return };
        let matches = self.find(&key).is_some_and(|i| i.owner == sender && i.menu_path.as_deref() == Some(path));
        if matches {
            self.send_menu(&key, false).await;
        }
    }

    async fn command(&mut self, c: TrayCommand) {
        match c {
            TrayCommand::Activate { key } => {
                let Some(st) = self.find(&key) else { return };
                if st.item.item_is_menu && st.menu_path.is_some() {
                    self.send_menu(&key, true).await;
                } else if let Err(e) = self.item_call(&key, "Activate", &(0i32, 0i32)).await {
                    // Many apps only provide a menu (libappindicator).
                    debug!("Activate on {key} failed ({e}); showing its menu");
                    self.send_menu(&key, true).await;
                }
            }
            TrayCommand::SecondaryActivate { key } => {
                if let Err(e) = self.item_call(&key, "SecondaryActivate", &(0i32, 0i32)).await {
                    debug!("SecondaryActivate on {key}: {e}");
                }
            }
            TrayCommand::Scroll { key, delta } => {
                if let Err(e) = self.item_call(&key, "Scroll", &(delta, "vertical")).await {
                    debug!("Scroll on {key}: {e}");
                }
            }
            TrayCommand::OpenMenu { key } => {
                let has_menu = self.find(&key).is_some_and(|i| i.menu_path.is_some());
                if has_menu {
                    self.send_menu(&key, true).await;
                } else if let Err(e) = self.item_call(&key, "ContextMenu", &(0i32, 0i32)).await {
                    debug!("ContextMenu on {key}: {e}");
                }
            }
            TrayCommand::MenuAboutToShow { key, id } => {
                let Some(menu) = self.menu_proxy(&key).await else { return };
                let need_update = menu.call::<_, _, bool>("AboutToShow", &(id,)).await.unwrap_or(false);
                let _ = menu.call_method("Event", &(id, "opened", Variant::from(0i32), event_time())).await;
                if need_update {
                    self.send_menu(&key, false).await;
                }
            }
            TrayCommand::MenuClicked { key, id } => {
                let Some(menu) = self.menu_proxy(&key).await else { return };
                if let Err(e) = menu.call_method("Event", &(id, "clicked", Variant::from(0i32), event_time())).await {
                    warn!("menu item {id} of {key}: {e}");
                }
            }
            TrayCommand::MenuClosed { key } => {
                if self.open_menu.as_deref() == Some(key.as_str()) {
                    self.open_menu = None;
                }
                if let Some(menu) = self.menu_proxy(&key).await {
                    let _ = menu.call_method("Event", &(0i32, "closed", Variant::from(0i32), event_time())).await;
                }
            }
        }
    }
}

/// dbusmenu timestamps: milliseconds, wrapping (only compared by apps).
fn event_time() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u32).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_addresses() {
        assert_eq!(parse_item_address(":1.42", None), Some((":1.42".into(), "/StatusNotifierItem".into())));
        assert_eq!(
            parse_item_address(":1.42/StatusNotifierItem", None),
            Some((":1.42".into(), "/StatusNotifierItem".into()))
        );
        assert_eq!(
            parse_item_address("org.kde.StatusNotifierItem-1234-1", Some(":1.9")),
            Some(("org.kde.StatusNotifierItem-1234-1".into(), "/StatusNotifierItem".into()))
        );
        assert_eq!(
            parse_item_address("/org/ayatana/NotificationItem/discord", Some(":1.77")),
            Some((":1.77".into(), "/org/ayatana/NotificationItem/discord".into()))
        );
        assert_eq!(parse_item_address("/org/ayatana/NotificationItem/x", None), None);
        assert_eq!(parse_item_address("", Some(":1.1")), None);
        assert_eq!(parse_item_address("/x", Some("")), None);
    }

    #[test]
    fn argb_conversion() {
        // One opaque red pixel and one half-transparent blue one.
        let argb = [255, 255, 0, 0, 128, 0, 0, 255];
        assert_eq!(argb_to_rgba(2, 1, &argb), Some(vec![255, 0, 0, 255, 0, 0, 255, 128]));
        assert_eq!(argb_to_rgba(2, 2, &argb), None, "size mismatch");
        assert_eq!(argb_to_rgba(0, 1, &[]), None);
    }

    #[test]
    fn pixmap_choice() {
        let px = |s: i32| (s, s, vec![0u8; (s * s * 4) as usize]);
        let list = vec![px(16), px(64), px(48), px(22), (32, 32, vec![0u8; 3])];
        assert_eq!(choose_pixmap(&list, 48).map(|p| p.width), Some(48));
        assert_eq!(choose_pixmap(&list, 40).map(|p| p.width), Some(48));
        assert_eq!(choose_pixmap(&list, 100).map(|p| p.width), Some(64), "largest when none is big enough");
        assert_eq!(choose_pixmap(&[(32, 32, vec![1, 2, 3])], 32), None, "invalid data is skipped");
        assert_eq!(choose_pixmap(&[], 32), None);
    }

    #[test]
    fn mnemonics() {
        assert_eq!(strip_mnemonic("_File"), "File");
        assert_eq!(strip_mnemonic("Save _As…"), "Save As…");
        assert_eq!(strip_mnemonic("snake__case"), "snake_case");
        assert_eq!(strip_mnemonic("plain"), "plain");
        assert_eq!(strip_mnemonic("trailing_"), "trailing");
    }

    #[test]
    fn markup() {
        assert_eq!(strip_markup("<b>Discord</b> &amp; friends"), "Discord & friends");
        assert_eq!(strip_markup("a &lt;tag&gt;"), "a <tag>");
    }

    fn props(list: &[(&str, Prop)]) -> HashMap<String, Prop> {
        list.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect()
    }

    #[test]
    fn menu_tree() {
        let quit = build_node(3, &props(&[("label", Prop::Str("_Quit".into()))]), vec![]);
        let hidden =
            build_node(4, &props(&[("label", Prop::Str("Hidden".into())), ("visible", Prop::Bool(false))]), vec![]);
        let sep = build_node(2, &props(&[("type", Prop::Str("separator".into()))]), vec![]);
        let check = build_node(
            5,
            &props(&[
                ("label", Prop::Str("Mute".into())),
                ("toggle-type", Prop::Str("checkmark".into())),
                ("toggle-state", Prop::Int(1)),
                ("enabled", Prop::Bool(false)),
            ]),
            vec![],
        );
        let sub = build_node(
            6,
            &props(&[("label", Prop::Str("Status".into())), ("children-display", Prop::Str("submenu".into()))]),
            vec![build_node(
                7,
                &props(&[("toggle-type", Prop::Str("radio".into())), ("label", Prop::Str("Online".into()))]),
                vec![],
            )],
        );
        let root = build_node(0, &HashMap::new(), vec![check, sep, sub, hidden, quit]);
        assert_eq!(root.children.len(), 4, "invisible items are dropped");
        assert_eq!(root.children[0].toggle, MenuToggle::Checkmark);
        assert_eq!(root.children[0].toggle_state, 1);
        assert!(!root.children[0].enabled);
        assert!(root.children[1].separator);
        assert!(root.children[2].is_submenu());
        assert_eq!(root.children[2].children[0].toggle, MenuToggle::Radio);
        assert_eq!(root.children[3].label, "Quit");
        assert!(root.children[3].enabled && root.children[3].visible, "defaults per spec");
    }

    #[test]
    fn layout_variants_decode() {
        use zbus::wire::StructureBuilder;
        use zbus::{Dict, Structure};
        let leaf_props: HashMap<String, Variant<'_>> =
            [("label".to_owned(), Variant::from("_Open")), ("enabled".to_owned(), Variant::from(true))]
                .into_iter()
                .collect();
        let leaf: Structure<'_> = StructureBuilder::new()
            .add_field(7i32)
            .append_field(Variant::Dict(Dict::from(leaf_props)))
            .add_field(Vec::<Variant<'_>>::new())
            .build()
            .expect("structure");
        let node = node_from_variant(&Variant::Variant(Box::new(Variant::Structure(leaf))), 1).expect("node");
        assert_eq!(node.id, 7);
        assert_eq!(node.label, "Open");
        assert!(node.enabled && node.children.is_empty());
    }
}
