//! XCursor theme loading and per-shape image caching.

use std::collections::HashMap;
use std::rc::Rc;

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Physical, Point, Transform},
};
use tracing::warn;
use xcursor::{
    CursorTheme,
    parser::{Image, parse_xcursor},
};

/// A cursor frame and its hotspot in physical pixels.
type CursorBuffer = (MemoryRenderBuffer, Point<i32, Physical>);

pub struct CursorManager {
    theme_name: String,
    theme: CursorTheme,
    size: u32,
    images: HashMap<(String, u32), Rc<Vec<Image>>>,
    buffers: HashMap<(String, u32, usize), CursorBuffer>,
}

impl CursorManager {
    pub fn new(theme: &str, size: u32) -> Self {
        let theme_name =
            std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| theme.to_owned());
        // Let clients that draw their own cursors match ours.
        // SAFETY: called during single-threaded initialization / reload.
        unsafe {
            std::env::set_var("XCURSOR_THEME", &theme_name);
            std::env::set_var("XCURSOR_SIZE", size.to_string());
        }
        Self {
            theme: CursorTheme::load(&theme_name),
            theme_name,
            size,
            images: HashMap::new(),
            buffers: HashMap::new(),
        }
    }

    pub fn reconfigure(&mut self, theme: &str, size: u32) {
        if theme != self.theme_name || size != self.size {
            *self = CursorManager::new(theme, size);
        }
    }

    fn load(&self, name: &str) -> Option<Vec<Image>> {
        let path = self.theme.load_icon(name)?;
        let data = std::fs::read(path).ok()?;
        parse_xcursor(&data).filter(|v| !v.is_empty())
    }

    fn images_for(&mut self, icon: CursorIcon, scale: u32) -> Rc<Vec<Image>> {
        let key = (icon.name().to_owned(), scale);
        if let Some(i) = self.images.get(&key) {
            return i.clone();
        }
        let names = std::iter::once(icon.name()).chain(icon.alt_names().iter().copied());
        let loaded = names
            .clone()
            .find_map(|n| self.load(n))
            .or_else(|| self.load("default"))
            .or_else(|| self.load("left_ptr"))
            .unwrap_or_else(|| {
                if icon == CursorIcon::Default {
                    warn!("cursor theme `{}` has no default cursor; using the built-in arrow", self.theme_name);
                }
                vec![fallback_arrow(self.size * scale)]
            });
        let rc = Rc::new(loaded);
        self.images.insert(key, rc.clone());
        rc
    }

    /// Image and hotspot (physical pixels) for `icon` at `scale`, animated
    /// cursors pick the frame for `time_ms`.
    pub fn buffer(&mut self, icon: CursorIcon, scale: u32, time_ms: u32) -> (MemoryRenderBuffer, Point<i32, Physical>) {
        let images = self.images_for(icon, scale);
        let target = (self.size * scale) as i32;
        let nearest = images.iter().min_by_key(|i| (i.size as i32 - target).abs()).map(|i| i.size).unwrap_or(0);
        let frames: Vec<(usize, &Image)> = images.iter().enumerate().filter(|(_, i)| i.size == nearest).collect();
        let total: u32 = frames.iter().map(|(_, i)| i.delay).sum();
        let mut t = if total > 0 { time_ms % total } else { 0 };
        let mut chosen = frames[0];
        for f in &frames {
            if t < f.1.delay {
                chosen = *f;
                break;
            }
            t -= f.1.delay;
        }
        let key = (icon.name().to_owned(), scale, chosen.0);
        if let Some(b) = self.buffers.get(&key) {
            return b.clone();
        }
        let img = chosen.1;
        let buffer = MemoryRenderBuffer::from_slice(
            &img.pixels_rgba,
            Fourcc::Abgr8888,
            (img.width as i32, img.height as i32),
            1,
            Transform::Normal,
            None,
        );
        let hotspot = Point::from((img.xhot as i32, img.yhot as i32));
        self.buffers.insert(key, (buffer.clone(), hotspot));
        (buffer, hotspot)
    }

    pub fn is_animated(&mut self, icon: CursorIcon) -> bool {
        self.images_for(icon, 1).len() > 1
    }

    pub fn default_image(&mut self) -> Image {
        self.images_for(CursorIcon::Default, 1)[0].clone()
    }
}

/// A plain arrow, used when no cursor theme is installed.
fn fallback_arrow(size: u32) -> Image {
    let s = size.max(16) as usize;
    let mut px = vec![0u8; s * s * 4];
    // Arrow outline: a triangle from the tip with a tail.
    let inside = |x: f64, y: f64| -> bool {
        let (x, y) = (x / s as f64 * 24.0, y / s as f64 * 24.0);
        let head = (0.0..=17.0).contains(&y) && x >= 0.0 && x <= y * 0.66;
        let tail = (4.5..=8.0).contains(&x) && (12.0..=21.0).contains(&y) && (x - 4.5) * 2.2 >= (y - 12.0) - 3.0;
        head || tail
    };
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let i = (y * s + x) * 4;
            if inside(fx, fy) {
                let edge =
                    !inside(fx - 1.5, fy) || !inside(fx + 1.5, fy) || !inside(fx, fy - 1.5) || !inside(fx, fy + 1.5);
                let c = if edge { 255 } else { 20 };
                px[i..i + 4].copy_from_slice(&[c, c, c, 255]);
            }
        }
    }
    Image {
        size,
        width: s as u32,
        height: s as u32,
        xhot: 1,
        yhot: 1,
        delay: 0,
        pixels_rgba: px,
        pixels_argb: Vec::new(),
    }
}
