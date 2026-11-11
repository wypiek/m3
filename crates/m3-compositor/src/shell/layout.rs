//! Pure layout geometry. Everything here is side-effect free and unit-tested.

use m3_protocol::Direction;
use smithay::utils::{Logical, Point, Rectangle, Size};

pub type Rect = Rectangle<i32, Logical>;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rectangle::new(Point::from((x, y)), Size::from((w.max(1), h.max(1))))
}

/// Shrink a rectangle by `by` on every side.
pub fn inset(r: Rect, by: i32) -> Rect {
    rect(r.loc.x + by, r.loc.y + by, r.size.w - 2 * by, r.size.h - 2 * by)
}

/// Master/stack tiling: the first window takes `ratio` of the width on the
/// left, the rest share the right column. `gap` separates windows and the
/// output edges.
pub fn master_stack(area: Rect, n: usize, ratio: f64, gap: i32) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let area = inset(area, gap);
    if n == 1 {
        return vec![area];
    }
    let master_w = ((area.size.w - gap) as f64 * ratio.clamp(0.1, 0.9)).round() as i32;
    let stack_w = area.size.w - gap - master_w;
    let mut out = vec![rect(area.loc.x, area.loc.y, master_w, area.size.h)];
    let stack_n = (n - 1) as i32;
    let total_h = area.size.h - gap * (stack_n - 1);
    let mut y = area.loc.y;
    for i in 0..stack_n {
        // Distribute rounding remainder to the last window.
        let h = if i == stack_n - 1 { area.loc.y + area.size.h - y } else { total_h / stack_n };
        out.push(rect(area.loc.x + master_w + gap, y, stack_w, h));
        y += h + gap;
    }
    out
}

/// Monocle: every window fills the area.
pub fn monocle(area: Rect, n: usize, gap: i32) -> Vec<Rect> {
    vec![inset(area, gap); n]
}

/// Half (or quarter, for diagonal combinations) of the area for snapping.
pub fn snap(area: Rect, dir: Direction, gap: i32) -> Rect {
    let a = inset(area, gap);
    let half_w = (a.size.w - gap) / 2;
    let half_h = (a.size.h - gap) / 2;
    match dir {
        Direction::Left => rect(a.loc.x, a.loc.y, half_w, a.size.h),
        Direction::Right => rect(a.loc.x + a.size.w - half_w, a.loc.y, half_w, a.size.h),
        Direction::Up => rect(a.loc.x, a.loc.y, a.size.w, half_h),
        Direction::Down => rect(a.loc.x, a.loc.y + a.size.h - half_h, a.size.w, half_h),
    }
}

/// Position a window of `size` centered in `area`, stepping diagonally by
/// `step` while the spot is already taken by one of `occupied`.
pub fn place_floating(
    area: Rect,
    size: Size<i32, Logical>,
    occupied: &[Point<i32, Logical>],
    step: i32,
) -> Point<i32, Logical> {
    let w = size.w.min(area.size.w);
    let h = size.h.min(area.size.h);
    let mut p = Point::from((area.loc.x + (area.size.w - w) / 2, area.loc.y + (area.size.h - h) / 2));
    for _ in 0..32 {
        if !occupied.contains(&p) {
            break;
        }
        let next = Point::from((p.x + step, p.y + step));
        if next.x + w > area.loc.x + area.size.w || next.y + h > area.loc.y + area.size.h {
            break;
        }
        p = next;
    }
    p
}

/// Keep at least `visible` pixels of a window inside `area` so it can always
/// be grabbed again.
pub fn keep_reachable(geo: Rect, area: Rect, visible: i32) -> Point<i32, Logical> {
    let min_x = area.loc.x - geo.size.w + visible;
    let max_x = area.loc.x + area.size.w - visible;
    let min_y = area.loc.y;
    let max_y = area.loc.y + area.size.h - visible;
    Point::from((geo.loc.x.clamp(min_x, max_x.max(min_x)), geo.loc.y.clamp(min_y, max_y.max(min_y))))
}

/// Snap a moving window's edges to the area edges when within `threshold`.
pub fn edge_snap(
    loc: Point<i32, Logical>,
    size: Size<i32, Logical>,
    area: Rect,
    threshold: i32,
) -> Point<i32, Logical> {
    let mut p = loc;
    if threshold <= 0 {
        return p;
    }
    let right = area.loc.x + area.size.w;
    let bottom = area.loc.y + area.size.h;
    if (p.x - area.loc.x).abs() <= threshold {
        p.x = area.loc.x;
    } else if (p.x + size.w - right).abs() <= threshold {
        p.x = right - size.w;
    }
    if (p.y - area.loc.y).abs() <= threshold {
        p.y = area.loc.y;
    } else if (p.y + size.h - bottom).abs() <= threshold {
        p.y = bottom - size.h;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        rect(0, 0, 1000, 800)
    }

    fn no_overlap(rs: &[Rect]) {
        for (i, a) in rs.iter().enumerate() {
            for b in &rs[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn master_stack_basic() {
        assert!(master_stack(area(), 0, 0.5, 0).is_empty());
        assert_eq!(master_stack(area(), 1, 0.5, 10), vec![rect(10, 10, 980, 780)]);
        let r = master_stack(area(), 3, 0.5, 10);
        assert_eq!(r.len(), 3);
        no_overlap(&r);
        // Master fills the full height.
        assert_eq!(r[0].size.h, 780);
        // Stack windows share the height and end at the bottom edge.
        assert_eq!(r[2].loc.y + r[2].size.h, 790);
        // Everything stays inside the inset area.
        for x in &r {
            assert!(inset(area(), 10).contains_rect(*x), "{x:?}");
        }
    }

    #[test]
    fn master_stack_many_windows_no_overlap() {
        for n in 2..12 {
            let r = master_stack(area(), n, 0.6, 8);
            assert_eq!(r.len(), n);
            no_overlap(&r);
        }
    }

    #[test]
    fn snapping() {
        let l = snap(area(), Direction::Left, 0);
        let r = snap(area(), Direction::Right, 0);
        assert_eq!(l, rect(0, 0, 500, 800));
        assert_eq!(r, rect(500, 0, 500, 800));
        assert!(!snap(area(), Direction::Left, 8).overlaps(snap(area(), Direction::Right, 8)));
    }

    #[test]
    fn floating_placement_cascades() {
        let s = Size::from((400, 300));
        let p1 = place_floating(area(), s, &[], 32);
        assert_eq!(p1, Point::from((300, 250)));
        let p2 = place_floating(area(), s, &[p1], 32);
        assert_eq!(p2, Point::from((332, 282)));
        // Oversized windows are placed at the area origin.
        assert_eq!(place_floating(area(), Size::from((2000, 2000)), &[], 32), Point::from((0, 0)));
    }

    #[test]
    fn reachable() {
        let g = rect(-5000, 5000, 400, 300);
        let p = keep_reachable(g, area(), 50);
        assert_eq!(p, Point::from((-350, 750)));
    }

    #[test]
    fn edge_snapping() {
        let s = Size::from((200, 100));
        assert_eq!(edge_snap(Point::from((7, 300)), s, area(), 16), Point::from((0, 300)));
        assert_eq!(edge_snap(Point::from((790, 695)), s, area(), 16), Point::from((800, 700)));
        assert_eq!(edge_snap(Point::from((400, 300)), s, area(), 16), Point::from((400, 300)));
    }
}
