//! Display-list primitives.
//!
//! A scene is built from data + viewport + style, then painted by a single
//! batched painter. Primitives are intentionally simple: rectangles, segments,
//! polylines, dots, and text.

use gpui::{FontWeight, Hsla, SharedString, TextAlign};

use crate::chart::core::{Point, Rect};

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    /// Filled rectangle, optionally rounded.
    Rect {
        rect: Rect,
        color: Hsla,
        radius: f32,
    },
    /// Straight segment, optionally dashed.
    Segment {
        from: Point,
        to: Point,
        width: f32,
        color: Hsla,
        dash: Option<[f32; 2]>,
    },
    /// A batch of disjoint segments. Used for grid lines and candle wicks.
    Segments {
        segments: Vec<(Point, Point)>,
        width: f32,
        color: Hsla,
        dash: Option<[f32; 2]>,
    },
    /// Open or closed polyline. A closed polyline with `fill` paints a shape.
    Polyline {
        points: Vec<Point>,
        width: f32,
        color: Hsla,
        closed: bool,
        fill: Option<Hsla>,
    },
    /// A small filled circle, painted as a rounded quad.
    Dot {
        center: Point,
        radius: f32,
        color: Hsla,
    },
    /// A single line of text.
    Glyph {
        text: SharedString,
        /// Alignment anchor. `Align::Left` starts at `origin.x`.
        origin: Point,
        font_size: f32,
        weight: FontWeight,
        color: Hsla,
        align: TextAlign,
    },
}

impl Primitive {
    pub fn rect(rect: Rect, color: Hsla) -> Self {
        Self::Rect {
            rect,
            color,
            radius: 0.0,
        }
    }

    pub fn segment(from: Point, to: Point, width: f32, color: Hsla) -> Self {
        Self::Segment {
            from,
            to,
            width,
            color,
            dash: None,
        }
    }

    pub fn text(text: impl Into<SharedString>, origin: Point, font_size: f32, color: Hsla) -> Self {
        Self::Glyph {
            text: text.into(),
            origin,
            font_size,
            weight: FontWeight::NORMAL,
            color,
            align: TextAlign::Left,
        }
    }

    /// Approximate bounds for culling and tests.
    pub fn bounds(&self) -> Rect {
        match self {
            Self::Rect { rect, .. } => *rect,
            Self::Segment { from, to, .. } => Rect::from_ltrb(
                from.x.min(to.x),
                from.y.min(to.y),
                from.x.max(to.x),
                from.y.max(to.y),
            ),
            Self::Segments { segments, .. } => {
                let mut bounds = Rect::ZERO;
                let mut first = true;
                for (from, to) in segments {
                    let segment = Rect::from_ltrb(
                        from.x.min(to.x),
                        from.y.min(to.y),
                        from.x.max(to.x),
                        from.y.max(to.y),
                    );
                    if first {
                        bounds = segment;
                        first = false;
                    } else {
                        let left = bounds.x.min(segment.x);
                        let top = bounds.y.min(segment.y);
                        let right = bounds.right().max(segment.right());
                        let bottom = bounds.bottom().max(segment.bottom());
                        bounds = Rect::from_ltrb(left, top, right, bottom);
                    }
                }
                bounds
            }
            Self::Polyline { points, .. } => {
                let Some(first) = points.first().copied() else {
                    return Rect::ZERO;
                };
                let mut left = first.x;
                let mut top = first.y;
                let mut right = first.x;
                let mut bottom = first.y;
                for point in points.iter().skip(1) {
                    left = left.min(point.x);
                    top = top.min(point.y);
                    right = right.max(point.x);
                    bottom = bottom.max(point.y);
                }
                Rect::from_ltrb(left, top, right, bottom)
            }
            Self::Dot { center, radius, .. } => Rect::new(
                center.x - radius,
                center.y - radius,
                radius * 2.0,
                radius * 2.0,
            ),
            Self::Glyph {
                origin, font_size, ..
            } => Rect::new(origin.x, origin.y, font_size * 8.0, *font_size * 1.4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::hsla;

    fn color() -> Hsla {
        hsla(0.0, 0.0, 1.0, 1.0)
    }

    #[test]
    fn rect_bounds() {
        let primitive = Primitive::rect(Rect::new(1.0, 2.0, 3.0, 4.0), color());
        assert_eq!(primitive.bounds(), Rect::new(1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn segment_bounds_normalize_direction() {
        let primitive =
            Primitive::segment(Point::new(10.0, 20.0), Point::new(0.0, 5.0), 1.0, color());
        assert_eq!(primitive.bounds(), Rect::from_ltrb(0.0, 5.0, 10.0, 20.0));
    }

    #[test]
    fn polyline_bounds_cover_all_points() {
        let primitive = Primitive::Polyline {
            points: vec![
                Point::new(5.0, 5.0),
                Point::new(-2.0, 10.0),
                Point::new(20.0, 0.0),
            ],
            width: 1.0,
            color: color(),
            closed: false,
            fill: None,
        };
        assert_eq!(primitive.bounds(), Rect::from_ltrb(-2.0, 0.0, 20.0, 10.0));
    }

    #[test]
    fn empty_polyline_has_zero_bounds() {
        let primitive = Primitive::Polyline {
            points: Vec::new(),
            width: 1.0,
            color: color(),
            closed: false,
            fill: None,
        };
        assert_eq!(primitive.bounds(), Rect::ZERO);
    }
}
