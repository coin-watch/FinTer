//! Hit-testing for candles, axis rails, and pane separators.

use super::geometry::{Point, Rect};
use super::layout::ChartLayout;
use super::viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisHandle {
    Price,
    Time,
}

/// Nearest candle index for a pixel x position.
///
/// Returns `None` when the buffer is empty or the plot has no width.
pub fn nearest_candle(x: f32, viewport: &Viewport, plot: Rect, total: usize) -> Option<usize> {
    if total == 0 || plot.w <= 0.0 || !x.is_finite() {
        return None;
    }
    let index = viewport.x_to_index(x, plot).round();
    if !index.is_finite() {
        return None;
    }
    Some((index.max(0.0) as usize).min(total - 1))
}

/// Which axis rail, if any, contains the point.
pub fn axis_handle(point: Point, layout: &ChartLayout, tolerance: f32) -> Option<AxisHandle> {
    let tolerance = if tolerance.is_finite() && tolerance >= 0.0 {
        tolerance
    } else {
        0.0
    };
    let plot = layout.plot;
    if plot.is_empty() {
        return None;
    }
    let near_price = point.x >= plot.right() - tolerance;
    let near_time = point.y >= plot.bottom() - tolerance;
    if near_price && point.y <= plot.bottom() + layout.time_rail.h {
        Some(AxisHandle::Price)
    } else if near_time && point.x <= plot.right() {
        Some(AxisHandle::Time)
    } else {
        None
    }
}

/// Index of the pane separator near a y position.
pub fn pane_separator(y: f32, layout: &ChartLayout, tolerance: f32) -> Option<usize> {
    let tolerance = if tolerance.is_finite() && tolerance >= 0.0 {
        tolerance
    } else {
        0.0
    };
    layout
        .separator_ys()
        .into_iter()
        .find_map(|(index, separator_y)| {
            if (y - separator_y).abs() <= tolerance {
                Some(index)
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::core::layout::{LayoutOptions, compute};

    fn plot() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 400.0)
    }

    #[test]
    fn nearest_candle_clamps_to_buffer() {
        let viewport = Viewport::new(0.0, 100.0);
        let p = plot();
        assert_eq!(nearest_candle(-100.0, &viewport, p, 100), Some(0));
        assert_eq!(nearest_candle(10_000.0, &viewport, p, 100), Some(99));
        assert_eq!(nearest_candle(0.0, &viewport, p, 0), None);
        assert_eq!(nearest_candle(0.0, &viewport, Rect::ZERO, 100), None);
    }

    #[test]
    fn nearest_candle_matches_slot_centres() {
        let viewport = Viewport::new(0.0, 100.0);
        let p = plot();
        let slot = 8.0;
        for index in [0usize, 1, 50, 99] {
            let x = p.x + (index as f32 + 0.5) * slot;
            assert_eq!(nearest_candle(x, &viewport, p, 100), Some(index));
        }
    }

    #[test]
    fn axis_handle_detects_price_and_time_rails() {
        let layout = compute(
            Rect::new(0.0, 0.0, 800.0, 500.0),
            60.0,
            &[],
            false,
            LayoutOptions::default(),
        );
        assert_eq!(
            axis_handle(Point::new(780.0, 200.0), &layout, 8.0),
            Some(AxisHandle::Price)
        );
        assert_eq!(
            axis_handle(Point::new(300.0, 495.0), &layout, 8.0),
            Some(AxisHandle::Time)
        );
        assert_eq!(axis_handle(Point::new(300.0, 200.0), &layout, 8.0), None);
    }

    #[test]
    fn pane_separator_detects_the_gap() {
        let layout = compute(
            Rect::new(0.0, 0.0, 800.0, 500.0),
            60.0,
            &[0.2, 0.2],
            true,
            LayoutOptions::default(),
        );
        let separators = layout.separator_ys();
        assert_eq!(separators.len(), 2);
        let (index, y) = separators[0];
        assert_eq!(pane_separator(y, &layout, 10.0), Some(index));
        assert_eq!(pane_separator(y + 40.0, &layout, 10.0), None);
    }
}
