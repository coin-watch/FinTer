//! Pointer gesture helpers.

use super::geometry::Point;

/// Tracks pointer velocity over a short time window for pan inertia.
#[derive(Clone, Debug)]
pub struct VelocityTracker {
    samples: Vec<(f64, f32)>,
    window_secs: f64,
}

impl Default for VelocityTracker {
    fn default() -> Self {
        Self::new(0.08)
    }
}

impl VelocityTracker {
    pub fn new(window_secs: f64) -> Self {
        Self {
            samples: Vec::with_capacity(8),
            window_secs: if window_secs.is_finite() && window_secs > 0.0 {
                window_secs.min(1.0)
            } else {
                0.08
            },
        }
    }

    pub fn push(&mut self, time_secs: f64, x: f32) {
        if !time_secs.is_finite() || !x.is_finite() {
            return;
        }
        let cutoff = time_secs - self.window_secs;
        self.samples.retain(|(t, _)| *t >= cutoff);
        self.samples.push((time_secs, x));
        if self.samples.len() > 8 {
            let excess = self.samples.len() - 8;
            self.samples.drain(0..excess);
        }
    }

    /// Pixels per second, positive to the right.
    pub fn velocity(&self) -> f32 {
        let Some((t0, x0)) = self.samples.first().copied() else {
            return 0.0;
        };
        let Some((t1, x1)) = self.samples.last().copied() else {
            return 0.0;
        };
        let dt = t1 - t0;
        if dt <= 1e-4 {
            return 0.0;
        }
        ((x1 - x0) as f64 / dt) as f32
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

/// Result of a pointer move while a button is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragUpdate {
    /// The pointer has not exceeded the drag threshold yet.
    Pending,
    /// The pointer is dragging, with displacement from the press origin.
    Dragging { dx: f32, dy: f32 },
}

/// Result of a pointer release.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragRelease {
    pub was_drag: bool,
    pub dx: f32,
    pub dy: f32,
}

/// Classifies a press-move-release sequence as a click or a drag.
#[derive(Clone, Debug)]
pub struct DragClassifier {
    threshold: f32,
    origin: Point,
    active: bool,
}

impl DragClassifier {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold: if threshold.is_finite() && threshold > 0.0 {
                threshold
            } else {
                3.0
            },
            origin: Point::ZERO,
            active: false,
        }
    }

    pub fn press(&mut self, point: Point) {
        self.origin = point;
        self.active = false;
    }

    pub fn update(&mut self, point: Point) -> DragUpdate {
        if !self.active {
            if self.origin.distance(point) < self.threshold {
                return DragUpdate::Pending;
            }
            self.active = true;
        }
        DragUpdate::Dragging {
            dx: point.x - self.origin.x,
            dy: point.y - self.origin.y,
        }
    }

    pub fn release(&mut self, point: Point) -> DragRelease {
        let dx = point.x - self.origin.x;
        let dy = point.y - self.origin.y;
        let was_drag = self.active || self.origin.distance(point) >= self.threshold;
        self.active = false;
        DragRelease { was_drag, dx, dy }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_uses_the_window() {
        let mut tracker = VelocityTracker::new(0.1);
        tracker.push(0.0, 0.0);
        tracker.push(0.05, 50.0);
        tracker.push(0.10, 100.0);
        assert!((tracker.velocity() - 1000.0).abs() < 1.0);
    }

    #[test]
    fn velocity_discards_old_samples() {
        let mut tracker = VelocityTracker::new(0.05);
        tracker.push(0.0, 0.0);
        tracker.push(1.0, 1000.0);
        assert_eq!(tracker.len(), 1);
        assert_eq!(tracker.velocity(), 0.0);
    }

    #[test]
    fn velocity_ignores_non_finite_input() {
        let mut tracker = VelocityTracker::default();
        tracker.push(f64::NAN, 10.0);
        tracker.push(1.0, f32::NAN);
        assert!(tracker.is_empty());
    }

    #[test]
    fn drag_classifier_waits_for_threshold() {
        let mut drag = DragClassifier::new(3.0);
        drag.press(Point::new(10.0, 10.0));
        assert_eq!(drag.update(Point::new(11.0, 11.0)), DragUpdate::Pending);
        assert_eq!(
            drag.update(Point::new(15.0, 10.0)),
            DragUpdate::Dragging { dx: 5.0, dy: 0.0 }
        );
        assert!(drag.is_active());
    }

    #[test]
    fn release_reports_click_and_drag() {
        let mut drag = DragClassifier::new(3.0);
        drag.press(Point::new(0.0, 0.0));
        drag.update(Point::new(1.0, 1.0));
        let release = drag.release(Point::new(1.0, 1.0));
        assert!(!release.was_drag);

        drag.press(Point::new(0.0, 0.0));
        drag.update(Point::new(10.0, 0.0));
        let release = drag.release(Point::new(10.0, 0.0));
        assert!(release.was_drag);
        assert_eq!(release.dx, 10.0);
    }
}

/// Scale a price range by dragging a price axis.
///
/// `dy` is the pointer displacement in pixels. Dragging up (negative `dy`)
/// shrinks the range around `anchor_price`; dragging down expands it. The
/// normalized position of the anchor stays fixed, so the price under the
/// pointer remains stationary.
pub fn drag_price_range(
    mode: crate::chart::core::PriceScaleMode,
    start_range: (f64, f64),
    anchor_price: f64,
    dy: f32,
) -> (f64, f64) {
    let (mut min, mut max) = start_range;
    if !min.is_finite() || !max.is_finite() || max <= min || !anchor_price.is_finite() {
        return start_range;
    }
    let factor = ((dy.clamp(-600.0, 600.0) as f64) * 0.006)
        .exp()
        .clamp(0.01, 100.0);

    let logarithmic = matches!(mode, crate::chart::core::PriceScaleMode::Log)
        && min > 0.0
        && max > 0.0
        && anchor_price > 0.0;

    if logarithmic {
        let anchor = anchor_price.ln();
        let lo = anchor - (anchor - min.ln()) * factor;
        let hi = anchor + (max.ln() - anchor) * factor;
        min = lo.exp();
        max = hi.exp();
    } else {
        min = anchor_price - (anchor_price - min) * factor;
        max = anchor_price + (max - anchor_price) * factor;
    }

    if !min.is_finite() || !max.is_finite() || max <= min {
        return start_range;
    }
    (min, max)
}

/// Pan a price range by dragging a price axis.
///
/// `dy` is the pointer displacement in pixels and `plot_h` is the candle plot
/// height in pixels. The range keeps its span and shifts by `dy` pixels
/// converted through the current pixels-per-price-unit. Invalid ranges,
/// non-finite input, and a non-positive plot height return `start_range`
/// unchanged.
pub fn pan_price_range(start_range: (f64, f64), dy: f32, plot_h: f32) -> (f64, f64) {
    let (min, max) = start_range;
    if !min.is_finite() || !max.is_finite() || max <= min {
        return start_range;
    }
    if !dy.is_finite() || !plot_h.is_finite() || plot_h <= 0.0 {
        return start_range;
    }
    let ppu = (max - min) / plot_h.max(1.0) as f64;
    let delta = dy as f64 * ppu;
    let (lo, hi) = (min + delta, max + delta);
    if !lo.is_finite() || !hi.is_finite() || hi <= lo {
        return start_range;
    }
    (lo, hi)
}

/// Scale the visible time span by dragging a time axis.
///
/// Dragging right (positive `dx`) widens the span; dragging left narrows it.
pub fn drag_time_span(start_span: f64, dx: f32, min_span: f64, max_span: f64) -> f64 {
    if !start_span.is_finite() || start_span <= 0.0 {
        return start_span;
    }
    let factor = ((dx.clamp(-1200.0, 1200.0) as f64) * 0.008)
        .exp()
        .clamp(0.01, 100.0);
    (start_span * factor).clamp(min_span.max(1.0), max_span.max(min_span.max(1.0)))
}

#[cfg(test)]
mod axis_tests {
    use super::*;
    use crate::chart::core::{PriceScale, PriceScaleMode, Rect};

    fn norm(value: f64, range: (f64, f64)) -> f64 {
        (value - range.0) / (range.1 - range.0)
    }

    #[test]
    fn price_drag_keeps_anchor_position() {
        let start = (100.0, 200.0);
        let anchor = 150.0;
        let after = drag_price_range(PriceScaleMode::Linear, start, anchor, -40.0);
        let before_norm = norm(anchor, start);
        let after_norm = norm(anchor, after);
        assert!((before_norm - after_norm).abs() < 1e-9);
        assert!(after.1 - after.0 < start.1 - start.0);
    }

    #[test]
    fn price_drag_down_expands() {
        let start = (100.0, 200.0);
        let after = drag_price_range(PriceScaleMode::Linear, start, 150.0, 40.0);
        assert!(after.1 - after.0 > start.1 - start.0);
    }

    #[test]
    fn log_price_drag_keeps_anchor_position() {
        let start = (10.0, 1000.0);
        let anchor = 100.0;
        let after = drag_price_range(PriceScaleMode::Log, start, anchor, -30.0);
        let before_frac = (anchor.ln() - start.0.ln()) / (start.1.ln() - start.0.ln());
        let after_frac = (anchor.ln() - after.0.ln()) / (after.1.ln() - after.0.ln());
        assert!((before_frac - after_frac).abs() < 1e-9);
        let scale_before = PriceScale::new(
            PriceScaleMode::Log,
            start.0,
            start.1,
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        let scale_after = PriceScale::new(
            PriceScaleMode::Log,
            after.0,
            after.1,
            Rect::new(0.0, 0.0, 100.0, 100.0),
        );
        assert!((scale_before.to_y(anchor) - scale_after.to_y(anchor)).abs() < 0.01);
    }

    #[test]
    fn price_pan_keeps_the_span_and_follows_the_drag() {
        let start = (100.0, 200.0);
        assert_eq!(pan_price_range(start, 10.0, 100.0), (110.0, 210.0));
        assert_eq!(pan_price_range(start, -10.0, 100.0), (90.0, 190.0));
        assert_eq!(pan_price_range(start, 10.0, 50.0), (120.0, 220.0));
    }

    #[test]
    fn price_pan_rejects_invalid_input() {
        let start = (100.0, 200.0);
        assert_eq!(pan_price_range(start, 10.0, 0.0), start);
        assert_eq!(pan_price_range(start, 10.0, -5.0), start);
        assert_eq!(pan_price_range(start, 10.0, f32::NAN), start);
        assert_eq!(pan_price_range(start, f32::NAN, 100.0), start);
        assert_eq!(pan_price_range((200.0, 100.0), 10.0, 100.0), (200.0, 100.0));
        assert_eq!(pan_price_range((100.0, 100.0), 10.0, 100.0), (100.0, 100.0));
        let nan_min = pan_price_range((f64::NAN, 200.0), 10.0, 100.0);
        assert!(nan_min.0.is_nan());
        assert_eq!(nan_min.1, 200.0);
    }

    #[test]
    fn time_drag_direction_and_limits() {
        let wider = drag_time_span(100.0, 40.0, 12.0, 5000.0);
        let tighter = drag_time_span(100.0, -40.0, 12.0, 5000.0);
        assert!(wider > 100.0);
        assert!(tighter < 100.0);
        assert_eq!(drag_time_span(12.0, -10_000.0, 12.0, 5000.0), 12.0);
        assert_eq!(drag_time_span(5000.0, 10_000.0, 12.0, 5000.0), 5000.0);
    }

    #[test]
    fn invalid_ranges_are_unchanged() {
        assert_eq!(
            drag_price_range(PriceScaleMode::Linear, (10.0, 10.0), 10.0, 20.0),
            (10.0, 10.0)
        );
        assert!(drag_time_span(f64::NAN, 10.0, 12.0, 5000.0).is_nan());
    }
}
