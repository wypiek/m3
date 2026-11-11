//! Cover art: scaled textures, the expressive shape-clipped cover, and the
//! album-art colour scheme.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use m3_core::config::ThemeConfig;
use m3_widgets::shape::{self, Shape};

/// Load `path` scaled to fit `size`×`size` (keeps aspect ratio).
pub fn load_pixbuf(path: &Path, size: i32) -> Option<Pixbuf> {
    match Pixbuf::from_file_at_scale(path, size, size, true) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::debug!("cannot load cover {}: {e}", path.display());
            None
        }
    }
}

/// Tightly packed RGBA8 pixels of a pixbuf.
pub fn rgba_pixels(p: &Pixbuf) -> (Vec<u8>, usize, usize) {
    let p = if p.has_alpha() { p.clone() } else { p.add_alpha(false, 0, 0, 0).unwrap_or_else(|_| p.clone()) };
    let (w, h, stride, n) = (p.width() as usize, p.height() as usize, p.rowstride() as usize, p.n_channels() as usize);
    let bytes = p.read_pixel_bytes();
    let src: &[u8] = &bytes;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let i = y * stride + x * n;
            if i + 3 < src.len() && n == 4 {
                out.extend_from_slice(&src[i..i + 4]);
            } else if i + 2 < src.len() {
                out.extend_from_slice(&[src[i], src[i + 1], src[i + 2], 255]);
            }
        }
    }
    (out, w, h)
}

pub fn texture(p: &Pixbuf) -> gdk::Texture {
    let (rgba, w, h) = rgba_pixels(p);
    let bytes = glib::Bytes::from_owned(rgba);
    gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, w * 4).upcast()
}

/// A cairo image surface (premultiplied ARGB32) from a pixbuf.
pub fn surface(p: &Pixbuf) -> Option<cairo::ImageSurface> {
    let (rgba, w, h) = rgba_pixels(p);
    let mut s = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32).ok()?;
    let stride = s.stride() as usize;
    {
        let mut data = s.data().ok()?;
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b, a) = (rgba[i] as u32, rgba[i + 1] as u32, rgba[i + 2] as u32, rgba[i + 3] as u32);
                let o = y * stride + x * 4;
                // Native-endian ARGB32 is BGRA in memory on little endian.
                let px = (a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255);
                data[o..o + 4].copy_from_slice(&px.to_ne_bytes());
            }
        }
    }
    Some(s)
}

type Key = (PathBuf, i32);
type Waiter = Box<dyn FnOnce(Option<&gdk::Texture>)>;

/// Scaled cover textures, decoded off the main thread so scrolling a big
/// library never blocks the UI. Results are cached per (path, size).
pub struct Thumbs {
    cache: RefCell<HashMap<Key, Option<gdk::Texture>>>,
    waiting: RefCell<HashMap<Key, Vec<Waiter>>>,
    jobs: std::sync::mpsc::Sender<Key>,
}

/// Decoded pixels from a worker: (key, rgba, width, height).
type Decoded = (Key, Option<(Vec<u8>, usize, usize)>);

impl Thumbs {
    pub fn new() -> Rc<Self> {
        let (jobs, rx) = std::sync::mpsc::channel::<Key>();
        let (done_tx, done_rx) = async_channel::unbounded::<Decoded>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        for n in 0..2 {
            let (rx, done_tx) = (rx.clone(), done_tx.clone());
            let spawned = std::thread::Builder::new().name(format!("m3-music-thumbs-{n}")).spawn(move || {
                loop {
                    let job = match rx.lock() {
                        Ok(r) => r.recv(),
                        Err(_) => break,
                    };
                    let Ok(key) = job else { break };
                    let px = load_pixbuf(&key.0, key.1).map(|p| rgba_pixels(&p));
                    if done_tx.send_blocking((key, px)).is_err() {
                        break;
                    }
                }
            });
            if let Err(e) = spawned {
                tracing::warn!("cannot start a thumbnail worker: {e}");
            }
        }
        let me = Rc::new(Self { cache: RefCell::default(), waiting: RefCell::default(), jobs });
        let weak = Rc::downgrade(&me);
        glib::spawn_future_local(async move {
            while let Ok((key, px)) = done_rx.recv().await {
                let Some(me) = weak.upgrade() else { break };
                let tex = px.map(|(rgba, w, h)| {
                    let bytes = glib::Bytes::from_owned(rgba);
                    gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, w * 4)
                        .upcast::<gdk::Texture>()
                });
                me.cache.borrow_mut().insert(key.clone(), tex.clone());
                let waiters = me.waiting.borrow_mut().remove(&key).unwrap_or_default();
                for w in waiters {
                    w(tex.as_ref());
                }
            }
        });
        me
    }

    /// Call `f` with the texture for `path` at `size` — immediately when
    /// cached, otherwise once a worker has decoded it.
    pub fn request(&self, path: &Path, size: i32, f: impl FnOnce(Option<&gdk::Texture>) + 'static) {
        let key = (path.to_owned(), size);
        if let Some(t) = self.cache.borrow().get(&key) {
            f(t.as_ref());
            return;
        }
        let mut waiting = self.waiting.borrow_mut();
        let first = !waiting.contains_key(&key);
        waiting.entry(key.clone()).or_default().push(Box::new(f));
        if first && self.jobs.send(key.clone()).is_err() {
            // No workers: decode here rather than never.
            drop(waiting);
            let tex = load_pixbuf(path, size).map(|p| texture(&p));
            self.cache.borrow_mut().insert(key.clone(), tex.clone());
            for w in self.waiting.borrow_mut().remove(&key).unwrap_or_default() {
                w(tex.as_ref());
            }
        }
    }
}

/// The colour seed of a cover (Material quantizer on a small version).
pub fn seed(path: &Path) -> Option<u32> {
    let p = load_pixbuf(path, 128)?;
    let (rgba, w, h) = rgba_pixels(&p);
    m3_theme::wallpaper::seed_from_rgba(&rgba, w, h)
}

/// CSS that scopes a scheme generated from `seed` to `.m3-art-scheme`.
pub fn scheme_css(cfg: &ThemeConfig, seed: u32) -> String {
    let mut c = cfg.clone();
    c.source = m3_core::config::ColorSource::Wallpaper;
    let theme = m3_theme::Theme::resolve(&c, Some(seed), m3_core::time::local_minutes());
    scope_variables(&m3_theme::css::variables(&theme, &cfg.font_family, &cfg.monospace_font_family, false))
}

/// Keep only the custom-property block of `variables()` output and move it
/// from `:root` to `.m3-art-scheme`.
pub fn scope_variables(vars: &str) -> String {
    let block = vars.split_once("}\n").map(|(b, _)| format!("{b}}}\n")).unwrap_or_default();
    block.replacen(":root", ".m3-art-scheme", 1)
}

/// A cover drawn inside an expressive shape that can rotate slowly.
#[derive(Clone)]
pub struct ShapedCover {
    area: gtk::DrawingArea,
    surface: Rc<RefCell<Option<cairo::ImageSurface>>>,
    angle: Rc<Cell<f64>>,
    spinning: Rc<Cell<bool>>,
    ticking: Rc<Cell<bool>>,
}

impl ShapedCover {
    pub fn new(size: i32, shape: Shape) -> Self {
        let area = gtk::DrawingArea::builder().content_width(size).content_height(size).build();
        area.add_css_class("m3-shaped-cover");
        let me = Self {
            area,
            surface: Rc::new(RefCell::new(None)),
            angle: Rc::new(Cell::new(0.0)),
            spinning: Rc::new(Cell::new(false)),
            ticking: Rc::new(Cell::new(false)),
        };
        let radii = shape.radii();
        let (surf, angle) = (me.surface.clone(), me.angle.clone());
        me.area.set_draw_func(move |a, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            let pts = shape::outline(&radii, w, h, angle.get());
            let Some(&(x0, y0)) = pts.first() else { return };
            cr.move_to(x0, y0);
            for &(x, y) in &pts[1..] {
                cr.line_to(x, y);
            }
            cr.close_path();
            match surf.borrow().as_ref() {
                Some(s) => {
                    cr.save().ok();
                    cr.clip();
                    // Cover-fit the image, counter-rotated so the art stays
                    // upright while the shape turns.
                    let (sw, sh) = (f64::from(s.width()), f64::from(s.height()));
                    let scale = (w / sw).max(h / sh);
                    cr.translate(w / 2.0, h / 2.0);
                    cr.scale(scale, scale);
                    cr.translate(-sw / 2.0, -sh / 2.0);
                    let _ = cr.set_source_surface(s, 0.0, 0.0);
                    let _ = cr.paint();
                    cr.restore().ok();
                }
                None => {
                    let c = a.color();
                    cr.set_source_rgba(f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()), 1.0);
                    let _ = cr.fill();
                }
            }
        });
        me
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Change the cover's size (responsive layouts).
    pub fn set_size(&self, size: i32) {
        self.area.set_content_width(size);
        self.area.set_content_height(size);
    }

    pub fn set_cover(&self, path: Option<&Path>) {
        let size = self.area.content_width().max(64) * 2;
        *self.surface.borrow_mut() = path.and_then(|p| load_pixbuf(p, size)).and_then(|p| surface(&p));
        self.area.queue_draw();
    }

    /// Rotate slowly while playing (respects reduced motion).
    pub fn set_spinning(&self, on: bool) {
        let enabled = gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
        self.spinning.set(on && enabled);
        if !self.spinning.get() || self.ticking.get() {
            return;
        }
        self.ticking.set(true);
        let (angle, spinning, ticking) = (self.angle.clone(), self.spinning.clone(), self.ticking.clone());
        let last = Rc::new(Cell::new(None::<i64>));
        self.area.add_tick_callback(move |a, clock| {
            let now = clock.frame_time();
            let dt = last.get().map(|l| (now - l) as f64 / 1e6).unwrap_or(0.0).min(0.1);
            last.set(Some(now));
            if !spinning.get() {
                ticking.set(false);
                return glib::ControlFlow::Break;
            }
            angle.set((angle.get() + dt * 8.0) % 360.0);
            a.queue_draw();
            glib::ControlFlow::Continue
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoping() {
        let vars = ":root {\n  --m3-primary: #112233;\n}\n\n.m3-body-large { font-size: 16px; }\n";
        let css = scope_variables(vars);
        assert!(css.starts_with(".m3-art-scheme {"));
        assert!(css.contains("--m3-primary: #112233;"));
        assert!(!css.contains("font-size"), "type classes are not duplicated");
    }

    #[test]
    fn scheme_from_seed() {
        let css = scheme_css(&ThemeConfig::default(), 0xd04040);
        assert!(css.starts_with(".m3-art-scheme"));
        assert!(css.contains("--m3-primary:") && css.contains("--m3-surface-container:"));
    }
}
