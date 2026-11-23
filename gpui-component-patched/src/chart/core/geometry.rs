//! Geometry primitives used by the chart core.
//!
//! These are deliberately independent of `gpui` so viewport and layout math can
//! be tested without a window. The scene layer converts them to GPUI pixels at
//! the paint boundary.

use std::ops::{Add, Sub};

/// A point in chart-local coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// An axis-aligned rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn from_ltrb(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            x: left,
            y: top,
            w: right - left,
            h: bottom - top,
        }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn center(self) -> Point {
        Point::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn is_empty(self) -> bool {
        !(self.w.is_finite() && self.h.is_finite()) || self.w <= 0.0 || self.h <= 0.0
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    /// Shrink the rectangle by the given insets.
    pub fn inset(self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        let x = self.x + left;
        let y = self.y + top;
        let w = (self.w - left - right).max(0.0);
        let h = (self.h - top - bottom).max(0.0);
        Self { x, y, w, h }
    }

    /// Clamp a point to the rectangle bounds.
    pub fn clamp_point(self, point: Point) -> Point {
        Point::new(
            point.x.clamp(self.x, self.right()),
            point.y.clamp(self.y, self.bottom()),
        )
    }

    /// Intersection of two rectangles, or a zero rectangle when disjoint.
    pub fn intersect(self, other: Self) -> Self {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            Self::ZERO
        } else {
            Self::from_ltrb(left, top, right, bottom)
        }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }
}

/// Snap a value to the nearest device pixel.
///
/// `scale` is the display scale factor (for example `2.0` on a Retina display).
/// Values that are not finite are returned unchanged.
pub fn snap(value: f32, scale: f32) -> f32 {
    if value.is_finite() && scale.is_finite() && scale > 0.0 {
        (value * scale).round() / scale
    } else {
        value
    }
}

/// Snap a point to the nearest device pixels.
pub fn snap_point(point: Point, scale: f32) -> Point {
    Point::new(snap(point.x, scale), snap(point.y, scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_edges_and_center() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
        assert_eq!(r.center(), Point::new(60.0, 45.0));
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(Point::new(0.0, 0.0)));
        assert!(r.contains(Point::new(9.99, 9.99)));
        assert!(!r.contains(Point::new(10.0, 5.0)));
        assert!(!r.contains(Point::new(5.0, 10.0)));
    }

    #[test]
    fn inset_never_produces_negative_size() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        let inset = r.inset(20.0, 20.0, 20.0, 20.0);
        assert_eq!(inset.w, 0.0);
        assert_eq!(inset.h, 0.0);
    }

    #[test]
    fn intersection_handles_disjoint() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 20.0, 5.0, 5.0);
        assert_eq!(a.intersect(b), Rect::ZERO);
    }

    #[test]
    fn snapping_uses_device_pixels() {
        assert_eq!(snap(1.4, 1.0), 1.0);
        assert_eq!(snap(1.6, 1.0), 2.0);
        assert_eq!(snap(1.26, 2.0), 1.5);
        assert_eq!(snap(1.24, 2.0), 1.0);
        assert_eq!(snap(1.26, 0.0), 1.26);
    }

    #[test]
    fn snap_point_only_changes_finite_values() {
        assert_eq!(snap_point(Point::new(1.6, 2.4), 1.0), Point::new(2.0, 2.0));
    }
}
