use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawingPoint {
    pub timestamp: i64,
    pub price: f64,
}

impl DrawingPoint {
    pub fn new(timestamp: i64, price: f64) -> Self {
        Self { timestamp, price }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DrawingTool {
    HorizontalLine,
    VerticalLine,
    Trendline,
    Ray,
    Segment,
    Fibonacci,
    PriceChannel,
    Text,
    LongPosition,
    ShortPosition,
}

impl DrawingTool {
    pub fn label(&self) -> &'static str {
        match self {
            Self::HorizontalLine => "Horizontal Line",
            Self::VerticalLine => "Vertical Line",
            Self::Trendline => "Trendline",
            Self::Ray => "Ray",
            Self::Segment => "Segment",
            Self::Fibonacci => "Fib Retracement",
            Self::PriceChannel => "Parallel Channel",
            Self::Text => "Text Note",
            Self::LongPosition => "Long Position",
            Self::ShortPosition => "Short Position",
        }
    }

    pub fn points_required(&self) -> usize {
        match self {
            Self::HorizontalLine | Self::VerticalLine | Self::Text => 1,
            Self::Trendline
            | Self::Ray
            | Self::Segment
            | Self::Fibonacci
            | Self::LongPosition
            | Self::ShortPosition => 2,
            Self::PriceChannel => 3,
        }
    }

    pub fn all() -> Vec<Self> {
        vec![
            Self::Trendline,
            Self::Ray,
            Self::Segment,
            Self::HorizontalLine,
            Self::VerticalLine,
            Self::Fibonacci,
            Self::PriceChannel,
            Self::LongPosition,
            Self::ShortPosition,
            Self::Text,
        ]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawingStyle {
    pub color: String,
    pub width: f64,
    pub dashed: bool,
    pub show_label: bool,
}

impl Default for DrawingStyle {
    fn default() -> Self {
        Self {
            color: "#C4E855".into(),
            width: 1.5,
            dashed: false,
            show_label: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrawingInstance {
    pub id: String,
    pub tool: DrawingTool,
    pub points: Vec<DrawingPoint>,
    pub style: DrawingStyle,
    pub text: Option<String>,
}

impl DrawingInstance {
    pub fn new(id: impl Into<String>, tool: DrawingTool) -> Self {
        Self {
            id: id.into(),
            tool,
            points: Vec::new(),
            style: DrawingStyle::default(),
            text: None,
        }
    }

    pub fn move_by(&mut self, delta_ts: i64, delta_price: f64) {
        for pt in &mut self.points {
            pt.timestamp = pt.timestamp.saturating_add(delta_ts);
            pt.price += delta_price;
        }
    }

    pub fn set_point(&mut self, idx: usize, pt: DrawingPoint) {
        if idx < self.points.len() {
            self.points[idx] = pt;
        } else if idx == self.points.len() {
            self.points.push(pt);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DrawingInteractionState {
    Idle,
    Placing {
        tool: DrawingTool,
        points: Vec<DrawingPoint>,
    },
    Selected {
        drawing_id: String,
    },
    DraggingPoint {
        drawing_id: String,
        point_index: usize,
    },
    MovingDrawing {
        drawing_id: String,
        anchor_point: DrawingPoint,
        original_points: Vec<DrawingPoint>,
    },
}

/// Helper math: distance from point (px, py) to line segment (x1, y1)-(x2, y2).
pub fn distance_to_segment(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let t = (((px - x1) * dx + (py - y1) * dy) / len_sq).clamp(0.0, 1.0);
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
}

/// Helper math: distance from point (px, py) to ray starting at (x1, y1) in direction of (x2, y2).
pub fn distance_to_ray(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let t = ((px - x1) * dx + (py - y1) * dy) / len_sq;
    if t <= 0.0 {
        return ((px - x1).powi(2) + (py - y1).powi(2)).sqrt();
    }
    let proj_x = x1 + t * dx;
    let proj_y = y1 + t * dy;
    ((px - proj_x).powi(2) + (py - proj_y).powi(2)).sqrt()
}

/// Hit-test against a set of drawings.
/// `to_screen` translates (timestamp, price) into (screen_x, screen_y).
pub fn hit_test_drawings<F>(
    screen_x: f64,
    screen_y: f64,
    drawings: &[DrawingInstance],
    tolerance_px: f64,
    to_screen: F,
) -> Option<String>
where
    F: Fn(i64, f64) -> Option<(f64, f64)>,
{
    for drawing in drawings.iter().rev() {
        if drawing.points.is_empty() {
            continue;
        }

        match drawing.tool {
            DrawingTool::HorizontalLine => {
                if let Some((_, sy)) = to_screen(drawing.points[0].timestamp, drawing.points[0].price) {
                    if (screen_y - sy).abs() <= tolerance_px {
                        return Some(drawing.id.clone());
                    }
                }
            }
            DrawingTool::VerticalLine => {
                if let Some((sx, _)) = to_screen(drawing.points[0].timestamp, drawing.points[0].price) {
                    if (screen_x - sx).abs() <= tolerance_px {
                        return Some(drawing.id.clone());
                    }
                }
            }
            DrawingTool::Trendline | DrawingTool::Segment => {
                if drawing.points.len() >= 2 {
                    if let (Some((x1, y1)), Some((x2, y2))) = (
                        to_screen(drawing.points[0].timestamp, drawing.points[0].price),
                        to_screen(drawing.points[1].timestamp, drawing.points[1].price),
                    ) {
                        if distance_to_segment(screen_x, screen_y, x1, y1, x2, y2) <= tolerance_px {
                            return Some(drawing.id.clone());
                        }
                    }
                }
            }
            DrawingTool::Ray => {
                if drawing.points.len() >= 2 {
                    if let (Some((x1, y1)), Some((x2, y2))) = (
                        to_screen(drawing.points[0].timestamp, drawing.points[0].price),
                        to_screen(drawing.points[1].timestamp, drawing.points[1].price),
                    ) {
                        if distance_to_ray(screen_x, screen_y, x1, y1, x2, y2) <= tolerance_px {
                            return Some(drawing.id.clone());
                        }
                    }
                }
            }
            DrawingTool::Fibonacci => {
                if drawing.points.len() >= 2 {
                    let start_p = drawing.points[0].price;
                    let end_p = drawing.points[1].price;
                    let levels = crate::klinechart::fibonacci_levels(start_p, end_p);
                    for (_, price) in levels {
                        if let Some((_, sy)) = to_screen(drawing.points[0].timestamp, price) {
                            if (screen_y - sy).abs() <= tolerance_px {
                                return Some(drawing.id.clone());
                            }
                        }
                    }
                }
            }
            DrawingTool::PriceChannel => {
                if drawing.points.len() >= 2 {
                    if let (Some((x1, y1)), Some((x2, y2))) = (
                        to_screen(drawing.points[0].timestamp, drawing.points[0].price),
                        to_screen(drawing.points[1].timestamp, drawing.points[1].price),
                    ) {
                        if distance_to_segment(screen_x, screen_y, x1, y1, x2, y2) <= tolerance_px {
                            return Some(drawing.id.clone());
                        }
                        if drawing.points.len() >= 3 {
                            if let Some((_, y3)) =
                                to_screen(drawing.points[2].timestamp, drawing.points[2].price)
                            {
                                let offset_y = y3 - y1;
                                if distance_to_segment(
                                    screen_x,
                                    screen_y,
                                    x1,
                                    y1 + offset_y,
                                    x2,
                                    y2 + offset_y,
                                ) <= tolerance_px
                                {
                                    return Some(drawing.id.clone());
                                }
                            }
                        }
                    }
                }
            }
            DrawingTool::Text => {
                if let Some((sx, sy)) = to_screen(drawing.points[0].timestamp, drawing.points[0].price) {
                    if (screen_x - sx).abs() <= tolerance_px * 2.0 && (screen_y - sy).abs() <= tolerance_px * 2.0 {
                        return Some(drawing.id.clone());
                    }
                }
            }
            DrawingTool::LongPosition | DrawingTool::ShortPosition => {
                if drawing.points.len() >= 2 {
                    if let (Some((x1, y1)), Some((x2, y2))) = (
                        to_screen(drawing.points[0].timestamp, drawing.points[0].price),
                        to_screen(drawing.points[1].timestamp, drawing.points[1].price),
                    ) {
                        let min_x = x1.min(x2) - tolerance_px;
                        let max_x = x1.max(x2) + tolerance_px;
                        let min_y = y1.min(y2) - tolerance_px;
                        let max_y = y1.max(y2) + tolerance_px;
                        if screen_x >= min_x && screen_x <= max_x && screen_y >= min_y && screen_y <= max_y {
                            return Some(drawing.id.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

pub use hit_test_drawings as hit_test;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_remain_data_anchored_after_roundtrip() {
        let d = DrawingInstance {
            id: "line-1".into(),
            tool: DrawingTool::Trendline,
            points: vec![DrawingPoint {
                timestamp: 123,
                price: 42.0,
            }],
            style: Default::default(),
            text: None,
        };
        assert_eq!(
            serde_json::from_str::<DrawingInstance>(&serde_json::to_string(&d).unwrap()).unwrap(),
            d
        );
    }

    #[test]
    fn hit_test_detects_horizontal_line() {
        let drawing = DrawingInstance {
            id: "hl-1".into(),
            tool: DrawingTool::HorizontalLine,
            points: vec![DrawingPoint::new(100, 50.0)],
            style: Default::default(),
            text: None,
        };

        let to_screen = |ts: i64, price: f64| Some((ts as f64, price * 2.0));

        // Click near price 50.0 (y = 100.0)
        assert_eq!(
            hit_test_drawings(50.0, 102.0, &[drawing.clone()], 5.0, to_screen),
            Some("hl-1".into())
        );

        // Click far away
        assert_eq!(
            hit_test_drawings(50.0, 120.0, &[drawing], 5.0, to_screen),
            None
        );
    }

    #[test]
    fn move_by_updates_all_anchors() {
        let mut d = DrawingInstance {
            id: "t-1".into(),
            tool: DrawingTool::Trendline,
            points: vec![
                DrawingPoint::new(100, 50.0),
                DrawingPoint::new(200, 60.0),
            ],
            style: Default::default(),
            text: None,
        };
        d.move_by(10, 5.0);
        assert_eq!(d.points[0], DrawingPoint::new(110, 55.0));
        assert_eq!(d.points[1], DrawingPoint::new(210, 65.0));
    }
}
