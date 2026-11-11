//! Internal pages: HTML error pages (shown in place of the failed page) and
//! the native GTK new-tab page.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use m3_theme::Theme;
use m3_widgets::{Shape, ShapeIcon};

use crate::address::html_escape;
use crate::store::Bookmark;

/// What the error page offers.
pub enum ErrorKind {
    /// Network or loading failure: retry and back.
    Load,
    /// Certificate problem: back only — no way to bypass.
    Tls,
}

/// An error page in the theme's colours. All dynamic text is escaped.
pub fn error_page(theme: &Theme, kind: ErrorKind, headline: &str, detail: &str, url: &str) -> String {
    let c = &theme.colors;
    let (glyph_color, container) = match kind {
        ErrorKind::Load => (c.on_secondary_container.hex(), c.secondary_container.hex()),
        ErrorKind::Tls => (c.on_error_container.hex(), c.error_container.hex()),
    };
    let retry = match kind {
        ErrorKind::Load => r#"<button class="filled" onclick="location.reload()">Try again</button>"#,
        ErrorKind::Tls => "",
    };
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>{title}</title>
<meta name="color-scheme" content="{scheme}">
<style>
html,body{{margin:0;height:100%;background:{bg};color:{fg};font-family:"Google Sans Flex","Roboto Flex",Inter,sans-serif}}
main{{max-width:560px;margin:0 auto;padding:12vh 32px;}}
.shape{{width:88px;height:88px;background:{container};color:{glyph};display:grid;place-items:center;font-size:44px;font-weight:700;
clip-path:polygon(50% 0%,61% 9%,75% 6%,80% 20%,94% 25%,91% 39%,100% 50%,91% 61%,94% 75%,80% 80%,75% 94%,61% 91%,50% 100%,39% 91%,25% 94%,20% 80%,6% 75%,9% 61%,0% 50%,9% 39%,6% 25%,20% 20%,25% 6%,39% 9%)}}
h1{{font-size:32px;font-weight:620;margin:28px 0 12px}}
p{{font-size:16px;line-height:1.5;color:{muted}}}
.url{{font-family:monospace;word-break:break-all;color:{fg}}}
button{{font:inherit;font-weight:600;font-size:15px;border:none;border-radius:999px;padding:12px 24px;margin:16px 8px 0 0;cursor:pointer;
transition:border-radius .35s cubic-bezier(.42,1.67,.21,.9)}}
button:active{{border-radius:12px}}
.filled{{background:{primary};color:{on_primary}}}
.tonal{{background:{sec};color:{on_sec}}}
</style></head><body><main>
<div class="shape">!</div>
<h1>{title}</h1>
<p>{detail}</p>
<p class="url">{url}</p>
{retry}<button class="tonal" onclick="history.back()">Go back</button>
</main></body></html>"#,
        title = html_escape(headline),
        detail = html_escape(detail),
        url = html_escape(url),
        scheme = if theme.dark { "dark" } else { "light" },
        bg = c.surface.hex(),
        fg = c.on_surface.hex(),
        muted = c.on_surface_variant.hex(),
        container = container,
        glyph = glyph_color,
        primary = c.primary.hex(),
        on_primary = c.on_primary.hex(),
        sec = c.secondary_container.hex(),
        on_sec = c.on_secondary_container.hex(),
    )
}

/// Explain TLS certificate problems in plain words.
pub fn tls_reason(flags: gtk::gio::TlsCertificateFlags) -> String {
    use gtk::gio::TlsCertificateFlags as F;
    let mut reasons = Vec::new();
    if flags.contains(F::UNKNOWN_CA) {
        reasons.push("it is signed by an authority this computer does not trust");
    }
    if flags.contains(F::BAD_IDENTITY) {
        reasons.push("it belongs to a different site");
    }
    if flags.contains(F::EXPIRED) {
        reasons.push("it has expired");
    }
    if flags.contains(F::NOT_ACTIVATED) {
        reasons.push("it is not valid yet");
    }
    if flags.contains(F::REVOKED) {
        reasons.push("it has been revoked");
    }
    if flags.contains(F::INSECURE) {
        reasons.push("it uses insecure cryptography");
    }
    if reasons.is_empty() {
        reasons.push("it could not be verified");
    }
    format!(
        "The site's security certificate can't be trusted: {}. Someone may be trying to impersonate the site, so m3 Browser did not open it.",
        reasons.join(", ")
    )
}

fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Hello, night owl",
    }
}

const TILE_SHAPES: [Shape; 6] =
    [Shape::Cookie(9), Shape::Clover(4), Shape::Sunny(8), Shape::Cookie(4), Shape::Flower(8), Shape::Pentagon];
const TILE_COLORS: [&str; 3] = ["primary-tile", "secondary", "tertiary"];

/// The native new-tab page.
pub struct NewTabPage {
    pub root: gtk::ScrolledWindow,
    pub search: gtk::Entry,
    tiles: gtk::FlowBox,
    empty: gtk::Label,
}

impl NewTabPage {
    /// `go` loads a URL (already resolved) in this tab.
    pub fn new(go: Rc<dyn Fn(&str)>) -> Self {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
        col.add_css_class("m3-ntp");
        col.set_halign(gtk::Align::Center);
        col.set_valign(gtk::Align::Start);

        let now = glib::DateTime::now_local().ok();
        let hello = gtk::Label::new(Some(greeting(now.as_ref().map(|n| n.hour() as u32).unwrap_or(12))));
        hello.add_css_class("m3-title-large");
        hello.add_css_class("m3-ntp-greeting");
        let clock = gtk::Label::new(None);
        clock.add_css_class("m3-display-large-emphasized");
        clock.add_css_class("m3-ntp-clock");
        let set_clock = {
            let (clock, hello) = (clock.clone(), hello.clone());
            move || {
                if let Ok(n) = glib::DateTime::now_local() {
                    if let Ok(s) = n.format("%H:%M") {
                        clock.set_text(&s);
                    }
                    hello.set_text(greeting(n.hour() as u32));
                }
            }
        };
        set_clock();
        let weak = clock.downgrade();
        glib::timeout_add_seconds_local(15, move || {
            if weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            set_clock();
            glib::ControlFlow::Continue
        });
        col.append(&hello);
        col.append(&clock);

        let search = gtk::Entry::new();
        search.add_css_class("m3-search");
        search.add_css_class("m3-ntp-search");
        search.set_placeholder_text(Some("Search the web or type an address"));
        search.set_primary_icon_name(Some("system-search-symbolic"));
        search.update_property(&[gtk::accessible::Property::Label("Search or address")]);
        col.append(&search);

        let label = gtk::Label::new(Some("Bookmarks"));
        label.add_css_class("m3-title-small-emphasized");
        label.add_css_class("m3-ntp-section");
        label.set_xalign(0.0);
        col.append(&label);
        let tiles = gtk::FlowBox::new();
        tiles.add_css_class("m3-ntp-tiles");
        tiles.set_selection_mode(gtk::SelectionMode::None);
        tiles.set_homogeneous(true);
        tiles.set_max_children_per_line(6);
        tiles.set_min_children_per_line(3);
        tiles.set_row_spacing(8);
        tiles.set_column_spacing(8);
        col.append(&tiles);
        let empty = gtk::Label::new(Some("Pages you bookmark with the star appear here."));
        empty.add_css_class("m3-secondary-text");
        empty.add_css_class("m3-body-medium");
        empty.set_xalign(0.0);
        empty.set_margin_start(12);
        col.append(&empty);

        let root = gtk::ScrolledWindow::new();
        root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        root.add_css_class("m3-ntp-page");
        root.set_child(Some(&col));

        search.connect_activate(move |e| go(&e.text()));
        Self { root, search, tiles, empty }
    }

    /// Rebuild the bookmark tiles (most recently added first).
    pub fn set_bookmarks(&self, bookmarks: &[Bookmark], open: Rc<dyn Fn(&str)>) {
        while let Some(c) = self.tiles.first_child() {
            self.tiles.remove(&c);
        }
        let mut list: Vec<&Bookmark> = bookmarks.iter().collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.added));
        self.empty.set_visible(list.is_empty());
        for (i, b) in list.into_iter().take(12).enumerate() {
            let title = if b.title.is_empty() {
                crate::address::host_of(&b.url).unwrap_or_else(|| b.url.clone())
            } else {
                b.title.clone()
            };
            let host = crate::address::host_of(&b.url).unwrap_or_default();
            let initial = host
                .trim_start_matches("www.")
                .chars()
                .next()
                .or_else(|| title.chars().next())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            let shape = TILE_SHAPES[i % TILE_SHAPES.len()];
            let s = ShapeIcon::new(shape, &initial, 64, false);
            s.widget().add_css_class(TILE_COLORS[i % TILE_COLORS.len()]);
            s.label().add_css_class("m3-ntp-initial");
            let lbl = gtk::Label::new(Some(&title));
            lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
            lbl.set_max_width_chars(12);
            lbl.add_css_class("m3-label-large");
            let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
            v.append(s.widget());
            v.append(&lbl);
            let btn = gtk::Button::new();
            btn.add_css_class("m3-ntp-tile");
            btn.set_child(Some(&v));
            btn.set_tooltip_text(Some(&b.url));
            btn.update_property(&[gtk::accessible::Property::Label(&title)]);
            m3_widgets::morph_on_interaction(&btn, s.shape(), shape, Shape::Sunny(8), Shape::Clover(4));
            let (open, url) = (open.clone(), b.url.clone());
            btn.connect_clicked(move |_| open(&url));
            self.tiles.append(&btn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_pages_escape_everything() {
        let t = m3_theme::Theme::resolve(&m3_core::config::ThemeConfig::default(), None, 0);
        let html = error_page(&t, ErrorKind::Tls, "<b>x</b>", "a & b", "https://e.example/\"><script>");
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(html.contains("a &amp; b"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("Try again"), "no bypass/retry on certificate errors");
        let html = error_page(&t, ErrorKind::Load, "Offline", "", "https://e.example/");
        assert!(html.contains("Try again"));
    }

    #[test]
    fn tls_reasons() {
        use gtk::gio::TlsCertificateFlags as F;
        let r = tls_reason(F::EXPIRED | F::BAD_IDENTITY);
        assert!(r.contains("expired") && r.contains("different site"));
        assert!(tls_reason(F::empty()).contains("could not be verified"));
    }

    #[test]
    fn greetings() {
        assert_eq!(greeting(8), "Good morning");
        assert_eq!(greeting(13), "Good afternoon");
        assert_eq!(greeting(20), "Good evening");
        assert_eq!(greeting(3), "Hello, night owl");
    }
}
