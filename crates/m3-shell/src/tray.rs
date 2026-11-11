//! The panel's system tray: StatusNotifierItem icons in a pill, with their
//! dbusmenu menus shown as Material 3 menus.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};
use m3_services::tray::{MenuNode, MenuToggle, TrayCommand, TrayHandle, TrayItem, TrayPixmap, TrayStatus};

use crate::icons;

pub struct TrayView {
    root: gtk::Box,
    handle: TrayHandle,
    buttons: RefCell<Vec<(String, gtk::Button)>>,
    items: RefCell<HashMap<String, TrayItem>>,
    /// A menu was requested from this panel for this item.
    pending_menu: RefCell<Option<String>>,
    /// The open menu: (item key, popover, page stack).
    open: RefCell<Option<(String, gtk::Popover, gtk::Stack)>>,
}

fn texture(p: &TrayPixmap) -> Option<gdk::MemoryTexture> {
    let stride = (p.width as usize).checked_mul(4)?;
    if p.rgba.len() != stride * p.height as usize {
        return None;
    }
    let bytes = glib::Bytes::from(&p.rgba);
    Some(gdk::MemoryTexture::new(p.width, p.height, gdk::MemoryFormat::R8g8b8a8, &bytes, stride))
}

/// Make icons from an item's own theme directory findable.
fn add_theme_path(path: &str) {
    if path.is_empty() || !std::path::Path::new(path).is_dir() {
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let theme = gtk::IconTheme::for_display(&display);
    let p = std::path::PathBuf::from(path);
    if !theme.search_path().contains(&p) {
        theme.add_search_path(&p);
    }
}

/// Set `image` from an icon name (theme name or absolute path) or a pixmap.
fn set_icon(image: &gtk::Image, name: &str, pixmap: Option<&TrayPixmap>) -> bool {
    if name.starts_with('/') && std::path::Path::new(name).is_file() {
        image.set_from_file(Some(name));
        return true;
    }
    if !name.is_empty() {
        let has = gdk::Display::default().is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(name));
        if has {
            image.set_icon_name(Some(name));
            return true;
        }
    }
    if let Some(t) = pixmap.and_then(texture) {
        image.set_paintable(Some(&t));
        return true;
    }
    false
}

impl TrayView {
    pub fn new(handle: TrayHandle, orientation: gtk::Orientation) -> Rc<Self> {
        let root = gtk::Box::new(orientation, 2);
        root.add_css_class("m3-tray");
        root.set_valign(gtk::Align::Center);
        root.set_halign(gtk::Align::Center);
        root.set_visible(false);
        Rc::new(Self {
            root,
            handle,
            buttons: RefCell::new(Vec::new()),
            items: RefCell::new(HashMap::new()),
            pending_menu: RefCell::new(None),
            open: RefCell::new(None),
        })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Show `items` (Passive ones are hidden), keeping existing buttons so an
    /// open menu stays anchored while icons change.
    pub fn update(self: &Rc<Self>, items: &[TrayItem]) {
        let shown: Vec<&TrayItem> = items.iter().filter(|i| i.status != TrayStatus::Passive).collect();
        *self.items.borrow_mut() = items.iter().map(|i| (i.key.clone(), i.clone())).collect();

        // Drop buttons whose items are gone (closing their menu).
        let keep: Vec<&str> = shown.iter().map(|i| i.key.as_str()).collect();
        let removed: Vec<(String, gtk::Button)> =
            self.buttons.borrow().iter().filter(|(k, _)| !keep.contains(&k.as_str())).cloned().collect();
        for (key, b) in removed {
            if self.open.borrow().as_ref().is_some_and(|(k, _, _)| *k == key) {
                self.close_menu();
            }
            self.root.remove(&b);
        }
        self.buttons.borrow_mut().retain(|(k, _)| keep.contains(&k.as_str()));

        let mut prev: Option<gtk::Widget> = None;
        for item in &shown {
            let existing = self.buttons.borrow().iter().find(|(k, _)| *k == item.key).map(|(_, b)| b.clone());
            let button = match existing {
                Some(b) => b,
                None => {
                    let b = self.make_button(&item.key);
                    self.buttons.borrow_mut().push((item.key.clone(), b.clone()));
                    b
                }
            };
            if button.parent().is_none() {
                self.root.insert_child_after(&button, prev.as_ref());
            } else {
                self.root.reorder_child_after(&button, prev.as_ref());
            }
            self.style_button(&button, item);
            prev = Some(button.upcast());
        }
        self.root.set_visible(!shown.is_empty());
    }

    fn style_button(&self, button: &gtk::Button, item: &TrayItem) {
        add_theme_path(&item.icon_theme_path);
        let Some(image) = button.child().and_downcast::<gtk::Image>() else { return };
        let attention = item.status == TrayStatus::NeedsAttention;
        let ok = attention && set_icon(&image, &item.attention_icon_name, item.attention_pixmap.as_ref())
            || set_icon(&image, &item.icon_name, item.icon_pixmap.as_ref());
        if !ok {
            image.set_icon_name(Some("application-x-executable-symbolic"));
        }
        if attention {
            button.add_css_class("attention");
        } else {
            button.remove_css_class("attention");
        }
        let tip = if item.tooltip.is_empty() { &item.title } else { &item.tooltip };
        button.set_tooltip_text((!tip.is_empty()).then_some(tip.as_str()));
        let name = if item.title.is_empty() { &item.id } else { &item.title };
        button.update_property(&[gtk::accessible::Property::Label(name)]);
    }

    fn make_button(self: &Rc<Self>, key: &str) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("m3-tray-item");
        let image = gtk::Image::new();
        image.set_pixel_size(20);
        button.set_child(Some(&image));

        // Primary click (and keyboard activation).
        let weak = Rc::downgrade(self);
        let k = key.to_owned();
        button.connect_clicked(move |_| {
            let Some(t) = weak.upgrade() else { return };
            // The item may answer with its menu (menu-only items).
            *t.pending_menu.borrow_mut() = Some(k.clone());
            t.handle.send(TrayCommand::Activate { key: k.clone() });
        });
        // Middle and right clicks.
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = Rc::downgrade(self);
        let k = key.to_owned();
        click.connect_released(move |g, _, _, _| {
            let Some(t) = weak.upgrade() else { return };
            match g.current_button() {
                2 => t.handle.send(TrayCommand::SecondaryActivate { key: k.clone() }),
                3 => {
                    *t.pending_menu.borrow_mut() = Some(k.clone());
                    t.handle.send(TrayCommand::OpenMenu { key: k.clone() });
                }
                _ => return,
            }
            g.set_state(gtk::EventSequenceState::Claimed);
        });
        button.add_controller(click);
        // Scrolling (volume applets, …).
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let weak = Rc::downgrade(self);
        let k = key.to_owned();
        scroll.connect_scroll(move |_, _, dy| {
            if let Some(t) = weak.upgrade() {
                let delta = if dy > 0.0 {
                    1
                } else if dy < 0.0 {
                    -1
                } else {
                    0
                };
                if delta != 0 {
                    t.handle.send(TrayCommand::Scroll { key: k.clone(), delta });
                }
            }
            glib::Propagation::Stop
        });
        button.add_controller(scroll);
        button
    }

    /// A menu arrived from the tray service: show it if this panel asked for
    /// it, or refresh it if it is already open here.
    pub fn show_menu(self: &Rc<Self>, key: &str, menu: &MenuNode) {
        let already_open = self.open.borrow().as_ref().is_some_and(|(k, _, _)| k == key);
        if already_open {
            if let Some((_, _, stack)) = self.open.borrow().as_ref() {
                self.fill_stack(stack, key, menu);
            }
            return;
        }
        if self.pending_menu.borrow().as_deref() != Some(key) {
            return;
        }
        *self.pending_menu.borrow_mut() = None;
        let Some(button) = self.buttons.borrow().iter().find(|(k, _)| k == key).map(|(_, b)| b.clone()) else {
            return;
        };
        if menu.children.is_empty() {
            return;
        }
        self.close_menu();

        let popover = gtk::Popover::new();
        popover.set_position(crate::placement::popover_side());
        popover.add_css_class("menu");
        popover.add_css_class("m3-tray-menu");
        popover.set_has_arrow(false);
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
        stack.set_vhomogeneous(false);
        stack.set_hhomogeneous(false);
        stack.set_interpolate_size(true);
        popover.set_child(Some(&stack));
        popover.set_parent(&button);
        self.fill_stack(&stack, key, menu);

        let weak = Rc::downgrade(self);
        let k = key.to_owned();
        popover.connect_closed(move |p| {
            let Some(t) = weak.upgrade() else { return };
            t.handle.send(TrayCommand::MenuClosed { key: k.clone() });
            let p = p.clone();
            // Unparent after the popover finished closing.
            let later = p.clone();
            glib::idle_add_local_once(move || {
                if later.parent().is_some() {
                    later.unparent();
                }
            });
            let mut open = t.open.borrow_mut();
            if open.as_ref().is_some_and(|(_, op, _)| *op == p) {
                open.take();
            }
        });
        *self.open.borrow_mut() = Some((key.to_owned(), popover.clone(), stack));
        popover.popup();
    }

    fn close_menu(&self) {
        let open = self.open.borrow_mut().take();
        if let Some((_, p, _)) = open {
            p.popdown();
        }
    }

    /// (Re)build all pages: the root and every submenu.
    fn fill_stack(self: &Rc<Self>, stack: &gtk::Stack, key: &str, menu: &MenuNode) {
        let visible = stack.visible_child_name().map(|n| n.to_string());
        while let Some(c) = stack.first_child() {
            stack.remove(&c);
        }
        self.add_page(stack, key, menu, None);
        if let Some(name) = visible.filter(|n| stack.child_by_name(n).is_some()) {
            stack.set_visible_child_name(&name);
        } else {
            stack.set_visible_child_name("0");
        }
    }

    fn add_page(self: &Rc<Self>, stack: &gtk::Stack, key: &str, node: &MenuNode, parent: Option<&MenuNode>) {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("m3-tray-menu-page");
        if let Some(parent) = parent {
            // Submenu header: back to the parent page.
            let back = gtk::Button::new();
            back.add_css_class("m3-menu-item");
            back.add_css_class("flat");
            back.add_css_class("m3-tray-menu-back");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.append(&icons::symbol_with("arrow_back", &["small"]));
            let l = gtk::Label::new(Some(&node.label));
            l.set_xalign(0.0);
            l.add_css_class("m3-title-small-emphasized");
            row.append(&l);
            back.set_child(Some(&row));
            let s = stack.clone();
            let parent_id = parent.id.to_string();
            back.connect_clicked(move |_| s.set_visible_child_name(&parent_id));
            page.append(&back);
            page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        for child in &node.children {
            if child.separator {
                page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                continue;
            }
            page.append(&self.menu_row(stack, key, child));
            if child.is_submenu() {
                self.add_page(stack, key, child, Some(node));
            }
        }
        stack.add_named(&page, Some(&node.id.to_string()));
    }

    fn menu_row(self: &Rc<Self>, stack: &gtk::Stack, key: &str, item: &MenuNode) -> gtk::Button {
        let b = gtk::Button::new();
        b.add_css_class("m3-menu-item");
        b.add_css_class("flat");
        b.set_sensitive(item.enabled);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let lead = match (item.toggle, item.toggle_state) {
            (MenuToggle::Checkmark, 1) => Some("check_box"),
            (MenuToggle::Checkmark, _) => Some("check_box_outline_blank"),
            (MenuToggle::Radio, 1) => Some("radio_button_checked"),
            (MenuToggle::Radio, _) => Some("radio_button_unchecked"),
            (MenuToggle::None, _) => None,
        };
        if let Some(sym) = lead {
            let classes: &[&str] =
                if item.toggle_state == 1 { &["small", "filled", "m3-tray-toggle"] } else { &["small"] };
            row.append(&icons::symbol_with(sym, classes));
        } else if !item.icon_data.is_empty() {
            let bytes = glib::Bytes::from(&item.icon_data);
            if let Ok(t) = gdk::Texture::from_bytes(&bytes) {
                let img = gtk::Image::from_paintable(Some(&t));
                img.set_pixel_size(18);
                row.append(&img);
            }
        } else if !item.icon_name.is_empty() {
            let img = gtk::Image::from_icon_name(&item.icon_name);
            img.set_pixel_size(18);
            row.append(&img);
        }
        let l = gtk::Label::new(Some(&item.label));
        l.set_xalign(0.0);
        l.set_hexpand(true);
        row.append(&l);
        if item.is_submenu() {
            row.append(&icons::symbol_with("chevron_right", &["small"]));
        }
        b.set_child(Some(&row));
        b.update_property(&[gtk::accessible::Property::Label(&item.label)]);

        let weak = Rc::downgrade(self);
        let k = key.to_owned();
        let id = item.id;
        let submenu = item.is_submenu();
        let s = stack.clone();
        b.connect_clicked(move |_| {
            let Some(t) = weak.upgrade() else { return };
            if submenu {
                t.handle.send(TrayCommand::MenuAboutToShow { key: k.clone(), id });
                s.set_visible_child_name(&id.to_string());
            } else {
                t.handle.send(TrayCommand::MenuClicked { key: k.clone(), id });
                t.close_menu();
            }
        });
        b
    }
}
