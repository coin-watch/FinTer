use std::rc::Rc;
use std::f32::consts::PI;

use gpui_component_macros::IntoPlot;
use gpui::{
    App, Bounds, Hsla, PathBuilder, Pixels, Point, Window, point, px,
    SharedString
};

use crate::plot::Plot;
use crate::PixelsExt;

const HALF_PI: f32 = PI / 2.;
const TAU: f32 = PI * 2.;

#[derive(IntoPlot)]
pub struct RadarChart<T: 'static> {
    data: Vec<T>,
    value: Option<Rc<dyn Fn(&T) -> f32>>,
    label: Option<Rc<dyn Fn(&T) -> SharedString>>,
    fill: Option<Hsla>,
    stroke: Option<Hsla>,
    grid_levels: usize,
    max_value: f32,
    hover_pos: Option<Point<Pixels>>,
}

impl<T> RadarChart<T> {
    pub fn new<I>(data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: data.into_iter().collect(),
            value: None,
            label: None,
            fill: None,
            stroke: None,
            grid_levels: 5,
            max_value: 100.,
            hover_pos: None,
        }
    }
    
    pub fn hover_pos(mut self, pos: Option<Point<Pixels>>) -> Self {
        self.hover_pos = pos;
        self
    }

    pub fn value(mut self, value: impl Fn(&T) -> f32 + 'static) -> Self {
        self.value = Some(Rc::new(value));
        self
    }

    pub fn label(mut self, label: impl Fn(&T) -> SharedString + 'static) -> Self {
        self.label = Some(Rc::new(label));
        self
    }

    pub fn fill(mut self, fill: impl Into<Hsla>) -> Self {
        self.fill = Some(fill.into());
        self
    }

    pub fn stroke(mut self, stroke: impl Into<Hsla>) -> Self {
        self.stroke = Some(stroke.into());
        self
    }

    pub fn max_value(mut self, max: f32) -> Self {
        self.max_value = max;
        self
    }
}

impl<T> Plot for RadarChart<T> {
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let n = self.data.len();
        if n == 0 {
            return;
        }

        let Some(value_fn) = self.value.as_ref() else { return; };

        let outer_radius = (bounds.size.width.min(bounds.size.height).as_f32() / 2.) * 0.75;
        let center_x = bounds.origin.x.as_f32() + bounds.size.width.as_f32() / 2.;
        let center_y = bounds.origin.y.as_f32() + bounds.size.height.as_f32() / 2.;
        let angle_step = TAU / n as f32;

        let grid_stroke = gpui::hsla(0., 0., 0.5, 0.3);

        // Grid
        for level in 1..=self.grid_levels {
            let radius = outer_radius * level as f32 / self.grid_levels as f32;
            let mut path = PathBuilder::stroke(px(1.0));
            for i in 0..n {
                let angle = i as f32 * angle_step - HALF_PI;
                let pt = point(px(center_x + radius * angle.cos()), px(center_y + radius * angle.sin()));
                if i == 0 {
                    path.move_to(pt);
                } else {
                    path.line_to(pt);
                }
            }
            let path = path.build().unwrap();
            
            window.paint_path(path, grid_stroke);
        }

        // Spokes
        for i in 0..n {
            let angle = i as f32 * angle_step - HALF_PI;
            let mut path = PathBuilder::stroke(px(1.0));
            path.move_to(point(px(center_x), px(center_y)));
            path.line_to(point(px(center_x + outer_radius * angle.cos()), px(center_y + outer_radius * angle.sin())));
            window.paint_path(path.build().unwrap(), grid_stroke);
        }

        // Data Polygon
        let mut fill_path = PathBuilder::fill();
        let mut stroke_path = PathBuilder::stroke(px(2.0));
        let mut pts = vec![];
        for (i, d) in self.data.iter().enumerate() {
            let val = value_fn(d);
            let radius = outer_radius * (val / self.max_value).max(0.0).min(1.0);
            let angle = i as f32 * angle_step - HALF_PI;
            let pt = point(px(center_x + radius * angle.cos()), px(center_y + radius * angle.sin()));
            pts.push(pt);
        }

        for (i, pt) in pts.iter().enumerate() {
            if i == 0 {
                fill_path.move_to(*pt);
                stroke_path.move_to(*pt);
            } else {
                fill_path.line_to(*pt);
                stroke_path.line_to(*pt);
            }
        }
        
        let fill_path = fill_path.build().unwrap();
        let stroke_path = stroke_path.build().unwrap();
        
        let mut fill_color = self.fill.clone().unwrap_or(Hsla::transparent_black());
        let stroke_color = self.stroke.clone().unwrap_or(fill_color);
        if self.fill.is_none() { fill_color.a = 0.2; }
        
        window.paint_path(fill_path, fill_color);
        window.paint_path(stroke_path, stroke_color);
        
        
        // Labels
        if let Some(label_fn) = self.label.as_ref() {
            let label_radius = outer_radius + 20.0;
            for (i, d) in self.data.iter().enumerate() {
                let text = label_fn(d);
                let angle = i as f32 * angle_step - HALF_PI;
                let lx = center_x + label_radius * angle.cos();
                let ly = center_y + label_radius * angle.sin();
                
                let font_size = px(12.0);
                let text_run = gpui::TextRun {
                    len: text.len(),
                    font: window.text_style().font(),
                    color: grid_stroke,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                if let Ok(shaped) = window.text_system().shape_text(text, font_size, &[text_run], None, None) {
                    for line in shaped {
                        let line_size = line.size(font_size);
                        let w = line_size.width.as_f32();
                        let h = font_size.as_f32();
                        let tx = bounds.origin.x.as_f32() + lx - w / 2.0;
                        let ty = bounds.origin.y.as_f32() + ly - h / 2.0;
                        let _ = line.paint(gpui::point(px(tx), px(ty)), font_size, gpui::TextAlign::Left, None, window, cx);
                    }
                }
            }
        }


        // Hover tooltips
        if let Some(cursor) = self.hover_pos {
            let cx_rel = cursor.x - bounds.origin.x;
            let cy_rel = cursor.y - bounds.origin.y;
            
            let mut closest_i = 0;
            let mut closest_dist = f32::MAX;
            for i in 0..n {
                let angle = i as f32 * angle_step - HALF_PI;
                let v = value_fn(&self.data[i]);
                let r = v / self.max_value * outer_radius;
                let vx = center_x + r * angle.cos();
                let vy = center_y + r * angle.sin();
                
                let dist = ((cx_rel.as_f32() - vx).powi(2) + (cy_rel.as_f32() - vy).powi(2)).sqrt();
                if dist < closest_dist {
                    closest_dist = dist;
                    closest_i = i;
                }
            }
            
            if closest_dist < 40.0 {
                let angle = closest_i as f32 * angle_step - HALF_PI;
                let v = value_fn(&self.data[closest_i]);
                let r = v / self.max_value * outer_radius;
                let vx = bounds.origin.x.as_f32() + center_x + r * angle.cos();
                let vy = bounds.origin.y.as_f32() + center_y + r * angle.sin();
                
                // Draw dot
                let dot_bounds = Bounds::new(
                    gpui::point(px(vx - 4.0), px(vy - 4.0)),
                    gpui::size(px(8.0), px(8.0)),
                );
                let mut quad = gpui::fill(dot_bounds, stroke_color);
                quad.corner_radii = gpui::Corners::all(px(4.0));
                window.paint_quad(quad);
                
                // Draw Tooltip text box
                let tooltip_text = if let Some(label_fn) = self.label.as_ref() {
                    let label_str = label_fn(&self.data[closest_i]);
                    // If label already contains value skip it, but our FNG is just states.
                    // Wait, Coinwatch FNG is 0-100, so we just show the state and the current FNG value.
                    // Actually, the polygon point value represents FNG score skew, not the literal FNG.
                    // But in gpui-kit radar charts, the tooltip just shows the value.
                    gpui::SharedString::from(format!("{}: {:.1}", label_str, v))
                } else {
                    gpui::SharedString::from(format!("{:.1}", v))
                };
                
                let font_size = px(12.0);
                let text_run = gpui::TextRun {
                    len: tooltip_text.len(),
                    font: window.text_style().font(),
                    color: gpui::white(), // white text for tooltip
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                if let Ok(shaped) = window.text_system().shape_text(tooltip_text, font_size, &[text_run], None, None) {
                    for line in shaped {
                        let line_size = line.size(font_size);
                        let w = line_size.width.as_f32() + 16.0;
                        let h = font_size.as_f32() + 12.0;
                        let mut tx = vx + 10.0;
                        let ty = vy - h / 2.0;

                        if tx + w > bounds.origin.x.as_f32() + bounds.size.width.as_f32() {
                            tx = vx - w - 10.0;
                        }

                        let bg_bounds = Bounds::new(
                            gpui::point(px(tx), px(ty)),
                            gpui::size(px(w), px(h)),
                        );
                        let mut bg_quad = gpui::fill(bg_bounds, gpui::rgba(0x373a40ff));
                        bg_quad.border_widths = gpui::Edges::all(px(1.0));
                        let bc: Hsla = gpui::rgba(0x686D76ff).into(); bg_quad.border_color = bc;
                        // RadarChart doesn't have CornerRadii imported, we use gpui::CornerRadii
                        bg_quad.corner_radii = gpui::Corners::all(px(4.0));
                        window.paint_quad(bg_quad);
                        
                        let _ = line.paint(gpui::point(px(tx + 8.0), px(ty + (h - font_size.as_f32()) / 2.0)), font_size, gpui::TextAlign::Left, None, window, cx);
                    }
                }
            }
        }
    }
}

