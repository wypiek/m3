//! The user picture: `~/.face` clipped to an expressive shape, or the
//! user's initial on that shape.

use std::path::Path;

use gtk::{cairo, gdk, prelude::*};
use m3_core::config::AvatarShape;
use m3_widgets::Shape;

pub const SIZE: i32 = 112;

/// The m3-widgets shape for a configured avatar shape.
pub fn shape_of(s: AvatarShape) -> Shape {
    match s {
        AvatarShape::Circle => Shape::Circle,
        AvatarShape::Square => Shape::Square,
        AvatarShape::Cookie4 => Shape::Cookie(4),
        AvatarShape::Cookie9 => Shape::Cookie(9),
        AvatarShape::Cookie12 => Shape::Cookie(12),
        AvatarShape::Clover4 => Shape::Clover(4),
        AvatarShape::Clover8 => Shape::Clover(8),
        AvatarShape::Sunny => Shape::Sunny(8),
        AvatarShape::Flower => Shape::Flower(8),
        AvatarShape::Burst => Shape::Burst(12),
        AvatarShape::Pentagon => Shape::Pentagon,
    }
}

/// Load an image file into a cairo surface (premultiplied BGRA = ARGB32).
fn surface_for(path: &Path) -> Option<cairo::ImageSurface> {
    let tex = gdk::Texture::from_filename(path).ok()?;
    let (w, h) = (tex.width(), tex.height());
    let stride = cairo::Format::ARgb32.stride_for_width(u32::try_from(w).ok()?).ok()?;
    let mut data = vec![0u8; usize::try_from(stride).ok()? * usize::try_from(h).ok()?];
    tex.download(&mut data, usize::try_from(stride).ok()?);
    cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, w, h, stride).ok()
}

/// The avatar widget for `initial` (and `~/.face` when it exists).
pub fn build(face: &Path, initial: &str, shape: AvatarShape) -> gtk::Widget {
    let s = shape_of(shape);
    if let Some(surface) = face.is_file().then(|| surface_for(face)).flatten() {
        let radii = s.radii();
        let area = gtk::DrawingArea::builder().content_width(SIZE).content_height(SIZE).build();
        area.add_css_class("m3-lock-avatar");
        area.set_halign(gtk::Align::Center);
        area.set_accessible_role(gtk::AccessibleRole::Img);
        area.set_draw_func(move |_, cr, w, h| {
            let (w, h) = (f64::from(w), f64::from(h));
            let pts = m3_widgets::shape::outline(&radii, w, h, 0.0);
            let Some(&(x0, y0)) = pts.first() else { return };
            cr.move_to(x0, y0);
            for &(x, y) in &pts[1..] {
                cr.line_to(x, y);
            }
            cr.close_path();
            cr.clip();
            let (iw, ih) = (f64::from(surface.width()), f64::from(surface.height()));
            if iw <= 0.0 || ih <= 0.0 {
                return;
            }
            // Cover the shape's box, centred.
            let k = (w / iw).max(h / ih);
            cr.translate((w - iw * k) / 2.0, (h - ih * k) / 2.0);
            cr.scale(k, k);
            if cr.set_source_surface(&surface, 0.0, 0.0).is_ok() {
                let _ = cr.paint();
            }
        });
        return area.upcast();
    }
    let icon = m3_widgets::ShapeIcon::new(s, initial, SIZE, false);
    icon.widget().add_css_class("m3-lock-avatar-shape");
    icon.widget().add_css_class("tertiary");
    icon.widget().clone().upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_maps() {
        for s in AvatarShape::ALL {
            let r = shape_of(s).radii();
            assert!(r.iter().all(|v| *v > 0.0), "{s:?}");
        }
        assert_eq!(shape_of(AvatarShape::Cookie12), Shape::Cookie(12));
    }
}
