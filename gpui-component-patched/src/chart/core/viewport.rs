//! Continuous viewport in candle-index units.
//!
//! The viewport is the single source of truth for what part of the candle
//! buffer is visible. `start` is a fractional index at the left edge of the
//! plot; `span` is the number of candle slots that fit. Keeping both as `f64`
//! makes panning and zooming sub-pixel smooth and removes the integer-floor
//! jitter of the legacy `ChartState`.

use std::ops::Range;

use super::geometry::Rect;

/// Default number of candles shown when a chart first opens.
pub const DEFAULT_SPAN: f64 = 100.0;
/// Fewest candles that can be shown at maximum zoom-in.
pub const MIN_SPAN: f64 = 12.0;
/// Most candles that can be shown at maximum zoom-out.
///
/// The legacy renderer capped this at 300. The new engine allows a much wider
/// view because semantic zoom aggregates candles into an envelope below a few
/// pixels per slot.
pub const MAX_SPAN: f64 = 5_000.0;

/// Constraints applied to a viewport when it changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportLimits {
    pub min_span: f64,
    pub max_span: f64,
    /// Extra candles rendered outside the visible range to avoid edge popping.
    pub overscan: usize,
    /// Empty slots kept to the right of the newest candle when pinned.
    pub right_margin: f64,
    /// Blank screens available before the oldest candle.
    pub past_blank_screens: f64,
    /// Blank screens available after the newest candle.
    pub future_blank_screens: f64,
    /// Safety cap for the blank margins, in candle slots.
    pub max_blank_slots: f64,
}

impl Default for ViewportLimits {
    fn default() -> Self {
        Self {
            min_span: MIN_SPAN,
            max_span: MAX_SPAN,
            overscan: 2,
            right_margin: 3.0,
            past_blank_screens: 1.0,
            future_blank_screens: 1.0,
            max_blank_slots: 10_000.0,
        }
    }
}

impl ViewportLimits {
    fn sanitized(self) -> Self {
        let min_span = if self.min_span.is_finite() && self.min_span >= 1.0 {
            self.min_span
        } else {
            MIN_SPAN
        };
        let max_span = if self.max_span.is_finite() && self.max_span >= min_span {
            self.max_span
        } else {
            min_span.max(DEFAULT_SPAN)
        };
        Self {
            min_span,
            max_span,
            overscan: self.overscan.min(64),
            right_margin: if self.right_margin.is_finite() {
                self.right_margin.max(0.0)
            } else {
                0.0
            },
            past_blank_screens: if self.past_blank_screens.is_finite() {
                self.past_blank_screens.clamp(0.0, 4.0)
            } else {
                0.0
            },
            future_blank_screens: if self.future_blank_screens.is_finite() {
                self.future_blank_screens.clamp(0.0, 4.0)
            } else {
                0.0
            },
            max_blank_slots: if self.max_blank_slots.is_finite() {
                self.max_blank_slots.clamp(0.0, 100_000.0)
            } else {
                0.0
            },
        }
    }

    fn blank_margins(&self, span: f64) -> (f64, f64) {
        let past = (span * self.past_blank_screens).min(self.max_blank_slots);
        let future = (span * self.future_blank_screens).min(self.max_blank_slots);
        (past.max(0.0), future.max(0.0))
    }
}

/// How far the viewport was dragged past its allowed bounds, in candle units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportOverflow {
    pub before: f64,
    pub after: f64,
}

impl ViewportOverflow {
    pub fn is_zero(self) -> bool {
        self.before <= f64::EPSILON && self.after <= f64::EPSILON
    }

    /// Convert the overflow to pixels for the given plot width.
    pub fn pixels(self, plot_width: f32, span: f64) -> (f32, f32) {
        if plot_width <= 0.0 || span <= 0.0 {
            return (0.0, 0.0);
        }
        let slot = (plot_width as f64 / span) as f32;
        ((self.before as f32) * slot, (self.after as f32) * slot)
    }
}

/// Continuous viewport over a candle buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Fractional index at the left edge of the plot.
    pub start: f64,
    /// Number of candle slots visible.
    pub span: f64,
    /// Whether the view follows the live edge.
    pub pinned: bool,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            start: 0.0,
            span: DEFAULT_SPAN,
            pinned: true,
        }
    }
}

impl Viewport {
    pub fn new(start: f64, span: f64) -> Self {
        Self {
            start,
            span,
            pinned: false,
        }
    }

    pub fn pinned(span: f64) -> Self {
        Self {
            start: 0.0,
            span,
            pinned: true,
        }
    }

    pub fn slot_width(&self, plot: Rect) -> f32 {
        if plot.w <= 0.0 || self.span <= 0.0 {
            0.0
        } else {
            (plot.w as f64 / self.span) as f32
        }
    }

    /// Pixel x of a candle's centre. Fractional indices are supported.
    pub fn index_to_x(&self, index: f64, plot: Rect) -> f32 {
        let slot = self.slot_width(plot);
        plot.x + ((index - self.start) + 0.5) as f32 * slot
    }

    /// Continuous candle index at a pixel x.
    ///
    /// The result is the index whose centre would be at `x`; it is not rounded.
    pub fn x_to_index(&self, x: f32, plot: Rect) -> f64 {
        let slot = self.slot_width(plot);
        if slot <= 0.0 {
            return self.start;
        }
        self.start + ((x - plot.x) / slot) as f64 - 0.5
    }

    /// Continuous data position (left edge = 0 slots) under a pixel x.
    pub fn x_to_position(&self, x: f32, plot: Rect) -> f64 {
        let slot = self.slot_width(plot);
        if slot <= 0.0 {
            return self.start;
        }
        self.start + ((x - plot.x) / slot) as f64
    }

    /// Visible index range including overscan, clamped to the buffer.
    pub fn visible(&self, total: usize, limits: &ViewportLimits) -> Range<usize> {
        if total == 0 {
            return 0..0;
        }
        let limits = limits.sanitized();
        let start = self.start.floor().max(0.0) as usize;
        let start = start.saturating_sub(limits.overscan).min(total);
        let end = (self.start + self.span).ceil().max(0.0) as usize + limits.overscan;
        start..end.min(total)
    }

    /// First and last allowed `start` values.
    pub fn bounds(&self, total: usize, limits: &ViewportLimits) -> (f64, f64) {
        if total == 0 {
            return (0.0, 0.0);
        }
        let total = total as f64;
        let limits = limits.sanitized();
        let (past, future) = limits.blank_margins(self.span);
        // The live edge leaves `right_margin` slots after the newest candle.
        // Beyond it, the user can pan into blank future time. Before the first
        // candle, `past` slots of blank past time are available.
        let live_start = total - self.span + limits.right_margin;
        let min_start = -past;
        let max_start = live_start + future;
        (min_start.min(max_start), max_start)
    }

    pub fn overflow(&self, total: usize, limits: &ViewportLimits) -> ViewportOverflow {
        let (min_start, max_start) = self.bounds(total, limits);
        ViewportOverflow {
            before: (min_start - self.start).max(0.0),
            after: (self.start - max_start).max(0.0),
        }
    }

    pub fn clamp(&mut self, total: usize, limits: &ViewportLimits) {
        let limits = limits.sanitized();
        self.span = self.span.clamp(limits.min_span, limits.max_span);
        let (min_start, max_start) = self.bounds(total, &limits);
        self.start = self.start.clamp(min_start, max_start);
    }

    /// Zoom by `factor` while keeping the data position under `anchor_x`
    /// stationary. `factor < 1` zooms in; `factor > 1` zooms out.
    pub fn zoom_at(
        &mut self,
        factor: f64,
        anchor_x: f32,
        plot: Rect,
        total: usize,
        limits: &ViewportLimits,
    ) {
        if !factor.is_finite() || factor <= 0.0 || plot.w <= 0.0 {
            return;
        }
        let limits = limits.sanitized();
        let anchor_frac = (((anchor_x - plot.x) / plot.w) as f64).clamp(0.0, 1.0);
        let data_pos = self.start + anchor_frac * self.span;
        let new_span = (self.span * factor).clamp(limits.min_span, limits.max_span);
        self.span = new_span;
        self.start = data_pos - anchor_frac * new_span;
        self.clamp(total, &limits);
    }

    /// Pan by a pixel delta. Dragging the content to the right (positive `dx`)
    /// moves the viewport toward older candles.
    pub fn pan_pixels(&mut self, dx: f32, plot: Rect) {
        let slot = self.slot_width(plot);
        if slot <= 0.0 || !dx.is_finite() {
            return;
        }
        self.start -= (dx / slot) as f64;
        self.pinned = false;
    }

    /// Pan with a rubber-band limit. The caller should keep calling `settle`
    /// after the pointer is released.
    pub fn pan_pixels_rubber(
        &mut self,
        dx: f32,
        plot: Rect,
        total: usize,
        limits: &ViewportLimits,
        max_overshoot_candles: f64,
    ) -> ViewportOverflow {
        self.pan_pixels(dx, plot);
        let limits = limits.sanitized();
        let (min_start, max_start) = self.bounds(total, &limits);
        let overshoot = max_overshoot_candles.max(0.0);
        self.start = self
            .start
            .clamp(min_start - overshoot, max_start + overshoot);
        self.overflow(total, &limits)
    }

    /// Return an overflowing viewport toward its bounds.
    ///
    /// Returns `true` while the viewport is still settling.
    pub fn settle(&mut self, dt: f32, total: usize, limits: &ViewportLimits) -> bool {
        let (min_start, max_start) = self.bounds(total, limits);
        let target = self.start.clamp(min_start, max_start);
        let delta = self.start - target;
        if delta.abs() <= 0.01 {
            self.start = target;
            return false;
        }
        if !dt.is_finite() || dt <= 0.0 {
            return true;
        }
        // Exponential return is equivalent to a critically damped spring in
        // this one-dimensional case and is stable at any frame rate.
        let dt = dt.min(0.064) as f64;
        self.start = target + delta * (-12.0 * dt).exp();
        true
    }

    /// Shift the viewport when older candles are prepended to the buffer.
    pub fn prepend(&mut self, count: usize) {
        if count > 0 {
            self.start += count as f64;
        }
    }

    /// Pin the viewport to the newest candles.
    pub fn scroll_to_latest(&mut self, total: usize, limits: &ViewportLimits) {
        let limits = limits.sanitized();
        self.span = self.span.clamp(limits.min_span, limits.max_span);
        if total == 0 {
            self.start = 0.0;
            self.pinned = true;
            return;
        }
        let total_f = total as f64;
        let mut start = total_f - self.span + limits.right_margin;
        let (min_start, max_start) = self.bounds(total, &limits);
        start = start.clamp(min_start, max_start);
        self.start = start;
        self.pinned = true;
    }

    /// Fit a buffer range to the plot.
    pub fn fit_to(&mut self, range: Range<usize>, total: usize, limits: &ViewportLimits) {
        let limits = limits.sanitized();
        let len = range.end.saturating_sub(range.start).max(1) as f64;
        self.span = len.clamp(limits.min_span, limits.max_span);
        self.start = range.start as f64;
        self.pinned = range.end >= total;
        self.clamp(total, &limits);
    }

    /// Scroll the minimum amount needed to make `index` visible.
    /// Continuous slot range covering the viewport, including overscan and
    /// blank time. Used by the time axis and the crosshair.
    pub fn slots(&self, _total: usize, limits: &ViewportLimits) -> std::ops::Range<i64> {
        let limits = limits.sanitized();
        let start = self.start.floor() as i64 - limits.overscan as i64;
        let end = (self.start + self.span).ceil() as i64 + limits.overscan as i64;
        start..end.max(start + 1)
    }

    /// Scroll the minimum amount needed to keep a continuous slot visible.
    pub fn ensure_slot_visible(
        &mut self,
        slot: f64,
        margin: f64,
        total: usize,
        limits: &ViewportLimits,
    ) {
        if !slot.is_finite() {
            return;
        }
        let margin = margin.max(0.0).min(self.span * 0.25);
        if slot < self.start + margin {
            self.start = slot - margin;
        } else if slot > self.start + self.span - 1.0 - margin {
            self.start = slot - self.span + 1.0 + margin;
        } else {
            return;
        }
        self.pinned = false;
        self.clamp(total, limits);
    }

    pub fn ensure_visible(
        &mut self,
        index: usize,
        margin: f64,
        total: usize,
        limits: &ViewportLimits,
    ) {
        let index = index as f64;
        let margin = margin.max(0.0).min(self.span * 0.25);
        if index < self.start + margin {
            self.start = index - margin;
        } else if index > self.start + self.span - 1.0 - margin {
            self.start = index - self.span + 1.0 + margin;
        } else {
            return;
        }
        self.pinned = false;
        self.clamp(total, limits);
    }

    /// Restore the default zoom and right-align (or fit) the newest candles.
    pub fn reset_view(&mut self, total: usize, limits: &ViewportLimits) {
        let limits = limits.sanitized();
        if total as f64 <= DEFAULT_SPAN {
            // A short history should fill the pane rather than being padded
            // out to the minimum zoom span.
            self.span = (total.max(1) as f64).clamp(1.0, limits.max_span);
            self.start = 0.0;
            self.pinned = true;
            return;
        }
        self.span = DEFAULT_SPAN.clamp(limits.min_span, limits.max_span);
        self.scroll_to_latest(total, &limits);
    }

    /// True when the viewport is close enough to the right edge to be pinned.
    pub fn at_latest(&self, total: usize, limits: &ViewportLimits) -> bool {
        if total == 0 {
            return true;
        }
        let limits = limits.sanitized();
        let (min_start, max_start) = self.bounds(total, &limits);
        let expected = (total as f64 - self.span + limits.right_margin).clamp(min_start, max_start);
        (self.start - expected).abs() < 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot() -> Rect {
        Rect::new(0.0, 0.0, 800.0, 400.0)
    }

    #[test]
    fn default_viewport_is_pinned_to_latest() {
        let v = Viewport::default();
        assert!(v.pinned);
        assert_eq!(v.span, DEFAULT_SPAN);
    }

    #[test]
    fn x_and_index_round_trip() {
        let v = Viewport::new(10.0, 50.0);
        let x = v.index_to_x(24.0, plot());
        assert!((v.x_to_index(x, plot()) - 24.0).abs() < 1e-6);
    }

    #[test]
    fn visible_range_includes_overscan_and_clamps() {
        let v = Viewport::new(10.0, 30.0);
        let limits = ViewportLimits {
            overscan: 2,
            ..Default::default()
        };
        assert_eq!(v.visible(100, &limits), 8..42);
        assert_eq!(v.visible(20, &limits), 8..20);
    }

    #[test]
    fn empty_buffer_has_empty_range() {
        let v = Viewport::new(-5.0, 20.0);
        assert_eq!(v.visible(0, &ViewportLimits::default()), 0..0);
    }

    #[test]
    fn zoom_keeps_anchor_position_stationary() {
        let mut v = Viewport::new(50.0, 100.0);
        let plot = plot();
        let anchor_x = 200.0;
        let before = v.x_to_position(anchor_x, plot);
        v.zoom_at(0.5, anchor_x, plot, 500, &ViewportLimits::default());
        let after = v.x_to_position(anchor_x, plot);
        assert!((before - after).abs() < 1e-9, "{before} vs {after}");
        assert_eq!(v.span, 50.0);
    }

    #[test]
    fn zoom_clamps_to_limits() {
        let mut v = Viewport::new(0.0, MIN_SPAN);
        let plot = plot();
        v.zoom_at(0.1, 400.0, plot, 500, &ViewportLimits::default());
        assert_eq!(v.span, MIN_SPAN);

        let mut v = Viewport::new(0.0, MAX_SPAN);
        v.zoom_at(10.0, 400.0, plot, 10_000, &ViewportLimits::default());
        assert_eq!(v.span, MAX_SPAN);
    }

    #[test]
    fn pan_moves_one_slot_per_slot_width() {
        let mut v = Viewport::new(10.0, 50.0);
        let plot = plot();
        v.pan_pixels(160.0, plot); // 10 slots
        assert!((v.start - 0.0).abs() < 1e-6);
        assert!(!v.pinned);
    }

    #[test]
    fn rubber_band_returns_to_bounds() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 50.0);
        let overflow = v.pan_pixels_rubber(10_000.0, plot(), 100, &limits, 8.0);
        assert!(!overflow.is_zero());
        let mut steps = 0;
        while v.settle(1.0 / 60.0, 100, &limits) {
            steps += 1;
            assert!(steps < 600, "viewport never settled");
        }
        assert!(v.overflow(100, &limits).is_zero());
    }

    #[test]
    fn prepend_preserves_visible_candles() {
        let mut v = Viewport::new(20.0, 50.0);
        let before = v.start;
        v.prepend(10);
        assert_eq!(v.start, before + 10.0);
    }

    #[test]
    fn scroll_to_latest_keeps_right_margin() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 100.0);
        v.scroll_to_latest(250, &limits);
        assert_eq!(v.start, 153.0);
        assert!(v.pinned);
    }

    #[test]
    fn short_history_is_right_aligned_when_pinned() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 100.0);
        v.scroll_to_latest(8, &limits);
        assert_eq!(v.start, -89.0);
        assert_eq!(v.visible(8, &limits), 0..8);
    }

    #[test]
    fn reset_view_fits_short_history() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(20.0, 50.0);
        v.reset_view(8, &limits);
        assert_eq!(v.start, 0.0);
        assert_eq!(v.span, 8.0);
        assert!(v.pinned);
    }

    #[test]
    fn reset_view_uses_default_span_for_long_history() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 20.0);
        v.reset_view(250, &limits);
        assert_eq!(v.span, DEFAULT_SPAN);
        assert_eq!(v.start, 153.0);
        assert!(v.pinned);
    }

    #[test]
    fn fit_to_range() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 100.0);
        v.fit_to(40..90, 200, &limits);
        assert_eq!(v.start, 40.0);
        assert_eq!(v.span, 50.0);
        assert!(!v.pinned);
    }

    #[test]
    fn ensure_visible_scrolls_minimally() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 20.0);
        v.ensure_visible(25, 2.0, 100, &limits);
        assert_eq!(v.start, 8.0);
        v.ensure_visible(9, 2.0, 100, &limits);
        assert_eq!(v.start, 7.0);
    }

    #[test]
    fn long_history_can_pan_into_blank_past() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 20.0);
        v.pan_pixels(10_000.0, plot());
        v.clamp(100, &limits);
        // One blank screen of past time is available before the oldest candle.
        assert_eq!(v.start, -20.0);
    }

    #[test]
    fn blank_future_can_leave_the_viewport_empty() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 20.0);
        let (_, max_start) = v.bounds(100, &limits);
        v.start = max_start;
        assert!(v.visible(100, &limits).is_empty());
        assert!(max_start > 100.0);
    }

    #[test]
    fn scroll_to_latest_stays_at_the_live_edge() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 100.0);
        v.scroll_to_latest(250, &limits);
        let (_, max_start) = v.bounds(250, &limits);
        assert_eq!(v.start, 153.0);
        assert!(max_start > v.start);
        assert!(v.at_latest(250, &limits));
    }

    #[test]
    fn slots_cover_blank_time_and_overscan() {
        let limits = ViewportLimits {
            overscan: 2,
            ..Default::default()
        };
        let v = Viewport::new(-10.0, 20.0);
        assert_eq!(v.slots(100, &limits), -12..12);
    }

    #[test]
    fn ensure_slot_visible_scrolls_into_blank_past() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 20.0);
        v.ensure_slot_visible(-5.0, 2.0, 100, &limits);
        assert_eq!(v.start, -7.0);
        assert!(!v.pinned);
    }

    #[test]
    fn at_latest_detects_pinned_edge() {
        let limits = ViewportLimits::default();
        let mut v = Viewport::new(0.0, 100.0);
        v.scroll_to_latest(250, &limits);
        assert!(v.at_latest(250, &limits));
        v.pan_pixels(-100.0, plot());
        v.clamp(250, &limits);
        assert!(!v.at_latest(250, &limits));
    }
}
