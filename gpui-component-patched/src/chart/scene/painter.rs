//! Batched scene painter.
//!
//! The painter is the only place that touches GPUI's low-level paint API. It
//! walks layers in order, converts chart-local geometry to window pixels, and
//! paints text through the shaped-text cache.

use gpui::{
    App, Bounds, PathBuilder, Pixels, Point as GpuiPoint, TextAlign, Window, fill, point, px,
};

use crate::chart::core::{Point, Rect, geometry::snap};

use super::layer::Layer;
use super::primitive::Primitive;
use super::scene::Scene;
use super::text_cache::{ShapedText, TextCache};

pub struct Painter {
    pub text: TextCache,
    dpr: f32,
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Painter {
    pub fn new() -> Self {
        Self {
            text: TextCache::default(),
            dpr: 1.0,
        }
    }

    pub fn paint(&mut self, scene: &Scene, bounds: Rect, window: &mut Window, cx: &mut App) {
        self.dpr = window.scale_factor();
        let clip = to_bounds(bounds);
        window.paint_layer(clip, |window| {
            for layer in Layer::ALL {
                for primitive in scene.layer(layer) {
                    self.paint_primitive(primitive, window, cx);
                }
            }
        });
    }

    fn paint_primitive(&mut self, primitive: &Primitive, window: &mut Window, cx: &mut App) {
        match primitive {
            Primitive::Rect {
                rect,
                color,
                radius,
            } => {
                let bounds = to_bounds(self.snapped(*rect));
                let quad = fill(bounds, *color);
                if *radius > 0.5 {
                    window.paint_quad(quad.corner_radii(px(*radius)));
                } else {
                    window.paint_quad(quad);
                }
            }
            Primitive::Segment {
                from,
                to,
                width,
                color,
                dash,
            } => {
                let mut builder = PathBuilder::stroke(px(*width));
                if let Some([on, off]) = dash {
                    builder = builder.dash_array(&[px(*on), px(*off)]);
                }
                builder.move_to(self.gpui_point(*from));
                builder.line_to(self.gpui_point(*to));
                if let Ok(path) = builder.build() {
                    window.paint_path(path, *color);
                }
            }
            Primitive::Segments {
                segments,
                width,
                color,
                dash,
            } => {
                if segments.is_empty() {
                    return;
                }
                let mut builder = PathBuilder::stroke(px(*width));
                if let Some([on, off]) = dash {
                    builder = builder.dash_array(&[px(*on), px(*off)]);
                }
                for (from, to) in segments {
                    builder.move_to(self.gpui_point(*from));
                    builder.line_to(self.gpui_point(*to));
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, *color);
                }
            }
            Primitive::Polyline {
                points,
                width,
                color,
                closed,
                fill: fill_color,
            } => {
                if points.len() < 2 {
                    return;
                }
                if let Some(fill_color) = fill_color {
                    let mut builder = PathBuilder::fill();
                    builder.move_to(self.gpui_point(points[0]));
                    for point in points.iter().skip(1) {
                        builder.line_to(self.gpui_point(*point));
                    }
                    builder.close();
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, *fill_color);
                    }
                }
                if *width > 0.0 {
                    let mut builder = PathBuilder::stroke(px(*width));
                    builder.move_to(self.gpui_point(points[0]));
                    for point in points.iter().skip(1) {
                        builder.line_to(self.gpui_point(*point));
                    }
                    if *closed {
                        builder.close();
                    }
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, *color);
                    }
                }
            }
            Primitive::Dot {
                center,
                radius,
                color,
            } => {
                let bounds = to_bounds(Rect::new(
                    center.x - radius,
                    center.y - radius,
                    radius * 2.0,
                    radius * 2.0,
                ));
                window.paint_quad(fill(bounds, *color).corner_radii(px(*radius)));
            }
            Primitive::Glyph {
                text,
                origin,
                font_size,
                weight,
                color,
                align,
            } => {
                let mut font = window.text_style().font();
                font.weight = *weight;
                let origin = self.snapped_point(*origin);
                let shaped: &ShapedText = self
                    .text
                    .get_or_shape(text, &font, *font_size, *color, window);
                let x = match align {
                    TextAlign::Left => origin.x,
                    TextAlign::Center => origin.x - shaped.width * 0.5,
                    TextAlign::Right => origin.x - shaped.width,
                };
                let mut y = origin.y;
                let line_height = px(*font_size);
                for line in &shaped.lines {
                    let _ = line.paint(
                        point(px(x), px(y)),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                    y += *font_size * 1.2;
                }
            }
        }
    }

    fn snapped_point(&self, point: Point) -> Point {
        Point::new(snap(point.x, self.dpr), snap(point.y, self.dpr))
    }

    fn snapped(&self, rect: Rect) -> Rect {
        Rect {
            x: snap(rect.x, self.dpr),
            y: snap(rect.y, self.dpr),
            w: snap(rect.w, self.dpr),
            h: snap(rect.h, self.dpr),
        }
    }

    fn gpui_point(&self, point: Point) -> GpuiPoint<Pixels> {
        gpui::point(px(snap(point.x, self.dpr)), px(snap(point.y, self.dpr)))
    }
}

fn to_bounds(rect: Rect) -> Bounds<Pixels> {
    Bounds::from_corners(
        gpui::point(px(rect.x), px(rect.y)),
        gpui::point(px(rect.right()), px(rect.bottom())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelsExt;

    #[test]
    fn rect_conversion_preserves_edges() {
        let bounds = to_bounds(Rect::new(10.0, 20.0, 100.0, 50.0));
        assert_eq!(bounds.origin.x.as_f32(), 10.0);
        assert_eq!(bounds.origin.y.as_f32(), 20.0);
        assert_eq!(bounds.size.width.as_f32(), 100.0);
        assert_eq!(bounds.size.height.as_f32(), 50.0);
    }
}
