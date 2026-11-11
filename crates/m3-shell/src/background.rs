//! Desktop wallpaper: one background layer surface per monitor.
//!
//! Images are decoded off the main thread; the decoded image also yields the
//! Material seed colour (cached by m3-theme). Without a configured image, m3
//! draws its own default wallpaper from the current colour scheme, so it
//! always matches the theme.

use std::cell::RefCell;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use m3_core::config::{ThemeConfig, WallpaperFit};
use m3_theme::{Colors, Rgb};
use tracing::{debug, warn};

pub struct Background {
    pub window: gtk::Window,
    stack: gtk::Stack,
    default_art: gtk::DrawingArea,
    colors: Rc<RefCell<Colors>>,
    current: RefCell<Option<PathBuf>>,
    generation: RefCell<u64>,
}

/// Decoded image ready to become a texture on the main thread.
struct Decoded {
    width: i32,
    height: i32,
    stride: usize,
    has_alpha: bool,
    bytes: glib::Bytes,
    seed: Option<u32>,
}

fn decode(path: &Path) -> Result<Decoded, String> {
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_file(path).map_err(|e| e.to_string())?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    let seed = m3_theme::wallpaper::cached_seed(path).or_else(|| {
        let small = pixbuf.scale_simple(128, 128, gtk::gdk_pixbuf::InterpType::Bilinear)?;
        let small = if small.has_alpha() { small } else { small.add_alpha(false, 0, 0, 0).ok()? };
        let stride = small.rowstride() as usize;
        let bytes = small.read_pixel_bytes();
        let mut rgba = Vec::with_capacity(128 * 128 * 4);
        for y in 0..small.height() as usize {
            let row = &bytes[y * stride..y * stride + small.width() as usize * 4];
            rgba.extend_from_slice(row);
        }
        let seed = m3_theme::wallpaper::seed_from_rgba(&rgba, small.width() as usize, small.height() as usize)?;
        m3_theme::wallpaper::store_seed(path, seed);
        Some(seed)
    });
    Ok(Decoded {
        width: pixbuf.width(),
        height: pixbuf.height(),
        stride: pixbuf.rowstride() as usize,
        has_alpha: pixbuf.has_alpha(),
        bytes: pixbuf.read_pixel_bytes(),
        seed,
    })
}

impl Background {
    pub fn new(app: &gtk::Application, monitor: &gdk::Monitor, colors: Colors) -> Rc<Self> {
        let window = gtk::Window::builder().application(app).decorated(false).build();
        window.init_layer_shell();
        window.set_layer(Layer::Background);
        window.set_namespace(Some("m3-wallpaper"));
        window.set_monitor(Some(monitor));
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::None);
        window.add_css_class("m3-layer");
        window.add_css_class("m3-wallpaper");

        let colors = Rc::new(RefCell::new(colors));
        let default_art = gtk::DrawingArea::new();
        {
            let colors = colors.clone();
            default_art.set_draw_func(move |_, cr, w, h| draw_default(cr, w as f64, h as f64, &colors.borrow()));
        }
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.add_named(&default_art, Some("default"));
        window.set_child(Some(&stack));
        window.present();
        Rc::new(Self { window, stack, default_art, colors, current: RefCell::new(None), generation: RefCell::new(0) })
    }

    pub fn set_colors(&self, colors: Colors) {
        *self.colors.borrow_mut() = colors;
        self.default_art.queue_draw();
    }

    /// Show `path` (or the default art when `None`). `on_seed` receives the
    /// extracted seed colour once the image is decoded.
    pub fn set_wallpaper(self: &Rc<Self>, path: Option<PathBuf>, cfg: &ThemeConfig, on_seed: impl Fn(u32) + 'static) {
        self.stack.set_transition_duration(if cfg.reduced_motion { 0 } else { cfg.wallpaper.transition_ms });
        if *self.current.borrow() == path {
            return;
        }
        *self.current.borrow_mut() = path.clone();
        let generation = {
            let mut g = self.generation.borrow_mut();
            *g += 1;
            *g
        };
        let Some(path) = path else {
            self.stack.set_visible_child_name("default");
            return;
        };
        let fit = cfg.wallpaper.fit;
        let this = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let p = path.clone();
            let result = gio::spawn_blocking(move || decode(&p)).await;
            let Some(this) = this.upgrade() else { return };
            if *this.generation.borrow() != generation {
                return; // superseded by a newer wallpaper
            }
            match result {
                Ok(Ok(d)) => {
                    let format = if d.has_alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
                    let texture = gdk::MemoryTexture::new(d.width, d.height, format, &d.bytes, d.stride);
                    let picture = gtk::Picture::for_paintable(&texture);
                    picture.set_content_fit(match fit {
                        WallpaperFit::Fill | WallpaperFit::Tile => gtk::ContentFit::Cover,
                        WallpaperFit::Fit => gtk::ContentFit::Contain,
                        WallpaperFit::Stretch => gtk::ContentFit::Fill,
                        WallpaperFit::Center => gtk::ContentFit::ScaleDown,
                    });
                    picture.set_can_shrink(true);
                    let name = format!("img{generation}");
                    this.stack.add_named(&picture, Some(&name));
                    this.stack.set_visible_child_name(&name);
                    // Drop older pictures after the crossfade.
                    let stack = this.stack.clone();
                    let keep = name.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || {
                        let mut child = stack.first_child();
                        while let Some(c) = child {
                            child = c.next_sibling();
                            let n = stack.page(&c).name();
                            if n.as_deref() != Some(keep.as_str()) && n.as_deref() != Some("default") {
                                stack.remove(&c);
                            }
                        }
                    });
                    debug!("wallpaper {} shown", path.display());
                    if let Some(seed) = d.seed {
                        on_seed(seed);
                    }
                }
                Ok(Err(e)) => {
                    warn!("cannot load wallpaper {}: {e}", path.display());
                    this.stack.set_visible_child_name("default");
                }
                Err(_) => warn!("wallpaper decoder crashed"),
            }
        });
    }
}

// ---- the default m3 wallpaper -------------------------------------------

fn set(cr: &gtk::cairo::Context, c: Rgb, a: f64) {
    cr.set_source_rgba(c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0, a);
}

fn stop(g: &gtk::cairo::Gradient, at: f64, c: Rgb, a: f64) {
    g.add_color_stop_rgba(at, c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0, a);
}

/// An organic blob: r(θ) = R·(1 + Σ aₖ·sin(kθ + φₖ)).
fn blob(cr: &gtk::cairo::Context, cx: f64, cy: f64, r: f64) {
    let steps = 480;
    for i in 0..=steps {
        let t = i as f64 / steps as f64 * 2.0 * PI;
        let rr = r * (1.0 + 0.085 * (3.0 * t + 0.9).sin() + 0.055 * (5.0 * t + 2.1).sin() + 0.03 * (7.0 * t).sin());
        let (x, y) = (cx + rr * t.cos(), cy + rr * t.sin());
        if i == 0 { cr.move_to(x, y) } else { cr.line_to(x, y) }
    }
    cr.close_path();
}

/// The default m3 wallpaper: a warm wash from peach to deep wine, a dark
/// wedge from the top, a large organic blob and thin flowing lines. Uses
/// the scheme's *fixed* tones so it looks the same in light and dark mode
/// (and the desktop widgets drawn on it keep their contrast).
fn draw_default(cr: &gtk::cairo::Context, w: f64, h: f64, c: &Colors) {
    let s = w.min(h);
    let peach = c.tertiary_fixed.mix(c.primary_fixed, 0.35);
    let rose = c.primary_fixed_dim.mix(c.on_primary_fixed_variant, 0.35);
    let wine = c.on_primary_fixed_variant;
    let ink = c.on_primary_fixed;
    let night = c.on_primary_fixed.mix(Rgb(0, 0, 0), 0.55);
    let gold = c.tertiary_fixed_dim;

    // Base wash, top-left light to bottom-right dark.
    let g = gtk::cairo::LinearGradient::new(0.0, 0.0, w * 0.85, h);
    stop(&g, 0.0, peach, 1.0);
    stop(&g, 0.32, rose, 1.0);
    stop(&g, 0.62, wine, 1.0);
    stop(&g, 1.0, ink, 1.0);
    let _ = cr.set_source(&g);
    let _ = cr.paint();

    // Night falls towards the right edge.
    let g = gtk::cairo::LinearGradient::new(w * 0.55, 0.0, w, 0.0);
    stop(&g, 0.0, night, 0.0);
    stop(&g, 1.0, night, 0.92);
    let _ = cr.set_source(&g);
    let _ = cr.paint();

    // Dark wedge hanging from the top edge.
    cr.move_to(w * 0.27, -1.0);
    cr.curve_to(w * 0.33, h * 0.30, w * 0.36, h * 0.52, w * 0.47, h * 0.57);
    cr.curve_to(w * 0.56, h * 0.61, w * 0.63, h * 0.40, w * 0.72, -1.0);
    cr.close_path();
    let g = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, h * 0.6);
    stop(&g, 0.0, night, 0.95);
    stop(&g, 0.7, ink, 0.85);
    stop(&g, 1.0, wine, 0.6);
    let _ = cr.set_source(&g);
    let _ = cr.fill();

    // The big blob, lit from the top left.
    let (bx, by, br) = (w * 0.74, h * 0.60, s * 0.46);
    blob(cr, bx, by, br);
    let g = gtk::cairo::LinearGradient::new(bx - br, by - br, bx + br * 0.6, by + br);
    stop(&g, 0.0, gold, 1.0);
    stop(&g, 0.38, rose, 1.0);
    stop(&g, 0.7, wine, 1.0);
    stop(&g, 1.0, night, 1.0);
    let _ = cr.set_source(&g);
    let _ = cr.fill();
    // Its shadowed inner echo.
    blob(cr, bx + br * 0.08, by + br * 0.06, br * 0.72);
    let g = gtk::cairo::LinearGradient::new(bx - br * 0.6, by - br * 0.6, bx + br * 0.6, by + br * 0.6);
    stop(&g, 0.0, rose, 0.0);
    stop(&g, 1.0, night, 0.85);
    let _ = cr.set_source(&g);
    let _ = cr.fill();

    // Thin lines drifting across.
    cr.set_line_width((s * 0.0012).max(1.2));
    set(cr, ink, 0.9);
    cr.move_to(w * 0.20, -1.0);
    cr.curve_to(w * 0.20, h * 0.35, w * 0.30, h * 0.62, w * 0.44, h * 0.66);
    cr.curve_to(w * 0.58, h * 0.70, w * 0.70, h * 0.52, w * 0.88, h * 0.30);
    let _ = cr.stroke();
    cr.move_to(w * 0.60, h + 1.0);
    cr.curve_to(w * 0.55, h * 0.72, w * 0.70, h * 0.45, w * 0.78, h * 0.32);
    cr.curve_to(w * 0.84, h * 0.22, w * 0.80, h * 0.08, w * 0.86, -1.0);
    let _ = cr.stroke();
    set(cr, c.on_tertiary_fixed_variant, 0.9);
    cr.move_to(w * 0.18, -1.0);
    cr.curve_to(w * 0.24, h * 0.18, w * 0.22, h * 0.36, w * 0.30, h * 0.48);
    let _ = cr.stroke();
}
