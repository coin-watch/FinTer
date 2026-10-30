/// Viewport + interaction state for the interactive candlestick chart.
///
/// Lives in `ChartPanel` (in-memory only; resets on restart).
/// Passed by shared reference into `CandlestickChart` on every render frame.
#[derive(Clone, Debug)]
pub struct ChartState {
    /// Index into the full candle buffer of the **first visible** candle.
    /// Clamped to `[0, total_candles - visible_count]` by mutators.
    pub viewport_offset: f64,

    /// How many candles fit in the visible window (zoom level).
    /// Clamped to `[MIN_VISIBLE, MAX_VISIBLE]` by mutators.
    pub visible_count: usize,

    /// Index into the **visible slice** of the candle that is under the
    /// crosshair, or `None` when the cursor is outside the chart area.
    pub crosshair_index: std::rc::Rc<std::cell::Cell<Option<usize>>>,

    /// Raw pixel position of the cursor relative to the chart area origin,
    /// used for positioning the crosshair lines and tooltip.
    pub crosshair_pos: Option<gpui::Point<gpui::Pixels>>,

    /// Whether the user is currently dragging (panning) the chart.
    pub is_dragging: bool,

    /// X pixel position where the drag started (relative to chart area).
    pub drag_start_x: f32,

    /// `viewport_offset` captured at the moment the drag began.
    pub drag_start_offset: f64,

    /// When `true`, the chart auto-scrolls to keep the latest candle visible
    /// as new data arrives.  Set to `false` the moment the user manually pans
    /// or zooms away from the right edge.  Reset to `true` if the user scrolls
    /// back to the right edge (e.g. by calling `scroll_to_latest` explicitly).
    pub pinned_to_latest: bool,

    /// Stored during paint: cursor X fraction (0.0 to 1.0) relative to chart bounds.
    pub last_cursor_frac: std::rc::Rc<std::cell::Cell<f32>>,

    /// Height ratio of the volume / indicator pane (default 0.18).
    pub volume_height_ratio: f32,
    pub subpane_ratios: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<usize, f32>>>,
    
    /// Is the user dragging the volume pane separator?
    pub is_dragging_volume: bool,
    pub is_dragging_separator: Option<usize>,

    pub drag_start_y: f32,
    pub drag_start_vol_ratio: f32,
    pub chart_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    /// Saved during paint: Absolute window Y pixel of the separator
    pub separator_y: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    pub separator_ys: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<usize, f32>>>,
    /// Saved during paint: Absolute window height of the total chart area
    pub chart_height: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    /// Saved during paint: candle plot origin (origin.x, origin.y) in pixels
    pub candle_plot_origin: std::rc::Rc<std::cell::Cell<Option<(f32, f32)>>>,
    /// Saved during paint: candle plot width and height (chart_w, candle_h) in pixels
    pub candle_plot_size: std::rc::Rc<std::cell::Cell<Option<(f32, f32)>>>,
    /// Saved during paint: (ts_min, ts_max)
    pub candle_ts_range: std::rc::Rc<std::cell::Cell<Option<(f64, f64)>>>,
    /// Saved during paint: (y_min_padded, y_max_padded)
    pub candle_price_range: std::rc::Rc<std::cell::Cell<Option<(f64, f64)>>>,
    /// Hitboxes registered during paint for interactive overlay buttons (e.g. close position, cancel order)
    pub overlay_hitboxes: std::rc::Rc<std::cell::RefCell<Vec<ChartOverlayHitbox>>>,
}

/// Clickable action for an overlay on the chart (position close or order cancel).
#[derive(Clone, Debug, PartialEq)]
pub enum ChartOverlayAction {
    ClosePosition { position_id: String },
    CancelOrder { order_id: String },
}

/// Hitbox for an overlay button (e.g. close position 'X' or cancel order 'X').
#[derive(Clone, Debug, PartialEq)]
pub struct ChartOverlayHitbox {
    pub bounds: gpui::Bounds<gpui::Pixels>,
    pub action: ChartOverlayAction,
}

/// Fewest candles that can be shown at maximum zoom-in.
pub const MIN_VISIBLE: usize = 20;
/// Most candles that can be shown at maximum zoom-out.
pub const MAX_VISIBLE: usize = 300;
/// Default number of visible candles.
pub const DEFAULT_VISIBLE: usize = 100;

impl ChartState {
    pub fn new() -> Self {
        Self {
            viewport_offset: 0.0,
            visible_count: DEFAULT_VISIBLE,
            crosshair_index: std::rc::Rc::new(std::cell::Cell::new(None)),
            crosshair_pos: None,
            is_dragging: false,
            drag_start_x: 0.0,
            drag_start_offset: 0.0,
            pinned_to_latest: true,
            last_cursor_frac: std::rc::Rc::new(std::cell::Cell::new(0.5)),
            volume_height_ratio: 0.18,
            subpane_ratios: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            is_dragging_separator: None,
            separator_ys: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            is_dragging_volume: false,
            drag_start_y: 0.0,
            drag_start_vol_ratio: 0.18,
            chart_bounds: None,
            separator_y: std::rc::Rc::new(std::cell::Cell::new(None)),
            chart_height: std::rc::Rc::new(std::cell::Cell::new(None)),
            candle_plot_origin: std::rc::Rc::new(std::cell::Cell::new(None)),
            candle_plot_size: std::rc::Rc::new(std::cell::Cell::new(None)),
            candle_ts_range: std::rc::Rc::new(std::cell::Cell::new(None)),
            candle_price_range: std::rc::Rc::new(std::cell::Cell::new(None)),
            overlay_hitboxes: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
        }
    }

    pub fn price_to_y(&self, price: f64) -> Option<f32> {
        let origin = self.candle_plot_origin.get()?;
        let size = self.candle_plot_size.get()?;
        let (y_min, y_max) = self.candle_price_range.get()?;
        let scale = super::linear_scale::LinearScale::new(y_min, y_max, size.1, 10.0);
        Some(origin.1 + scale.scale(price))
    }

    pub fn y_to_price(&self, y: f32) -> Option<f64> {
        let origin = self.candle_plot_origin.get()?;
        let size = self.candle_plot_size.get()?;
        let (y_min, y_max) = self.candle_price_range.get()?;
        let scale = super::linear_scale::LinearScale::new(y_min, y_max, size.1, 10.0);
        Some(scale.inverse(y - origin.1))
    }

    pub fn ts_to_x(&self, ts: f64) -> Option<f32> {
        let origin = self.candle_plot_origin.get()?;
        let size = self.candle_plot_size.get()?;
        let (ts_min, ts_max) = self.candle_ts_range.get()?;
        let scale = super::linear_scale::LinearScale::new(ts_min, ts_max, 0.0, size.0);
        Some(origin.0 + scale.scale(ts))
    }

    pub fn x_to_ts(&self, x: f32) -> Option<f64> {
        let origin = self.candle_plot_origin.get()?;
        let size = self.candle_plot_size.get()?;
        let (ts_min, ts_max) = self.candle_ts_range.get()?;
        let scale = super::linear_scale::LinearScale::new(ts_min, ts_max, 0.0, size.0);
        Some(scale.inverse(x - origin.0))
    }

    // -----------------------------------------------------------------------
    // Viewport helpers
    // -----------------------------------------------------------------------

    /// Clamp `viewport_offset` so the view never exceeds the buffer.
    pub fn clamp_offset(&mut self, total: usize) {
        if total == 0 {
            self.viewport_offset = 0.0;
            return;
        }
        let max_offset = (total as f64 - 2.0).max(0.0);
        let min_offset = (2.0 - self.visible_count as f64).min(0.0);
        self.viewport_offset = self.viewport_offset.min(max_offset).max(min_offset);
    }

    /// Scroll the viewport by `delta` candles (positive = scroll right / older).
    /// Unpins from latest so auto-follow stops until the user scrolls back.
    pub fn scroll_by(&mut self, delta: f64, total: usize) {
        self.viewport_offset += delta;
        self.clamp_offset(total);
        self.pinned_to_latest = false;
    }

    /// Zoom the viewport, anchoring on `cursor_frac` (0.0 = left edge, 1.0 = right edge).
    ///
    /// The candle that was under the cursor remains stationary after the zoom.
    /// `factor > 1.0` zooms out (more candles), `factor < 1.0` zooms in (fewer).
    ///
    /// Pass `0.5` to get the old centre-anchored behaviour.
    pub fn zoom(&mut self, factor: f32, total: usize, cursor_frac: f32) {
        let cursor_frac = cursor_frac.clamp(0.0, 1.0) as f64;
        
        let visible_f = (self.visible_count as f64 - 1.0).max(0.0);
        let exact_anchor = self.viewport_offset + visible_f * cursor_frac;

        let new_visible = ((self.visible_count as f32) * factor).round() as usize;
        self.visible_count = new_visible
            .clamp(MIN_VISIBLE, MAX_VISIBLE)
            .min(total.max(1));

        let new_visible_f = (self.visible_count as f64 - 1.0).max(0.0);
        let new_offset = exact_anchor - new_visible_f * cursor_frac;
        
        self.viewport_offset = new_offset.max(0.0);
        self.clamp_offset(total);
    }

    /// Pin the viewport to the most recent candles (right edge).
    /// Also re-enables auto-follow.
    pub fn scroll_to_latest(&mut self, total: usize) {
        // Add 3 candles of empty right margin for the newest candle
        let default_max_offset = total.saturating_sub(self.visible_count) as f64 + 3.0;
        let absolute_max = (total as f64 - 2.0).max(0.0);
        self.viewport_offset = default_max_offset.min(absolute_max).max(0.0);
        self.pinned_to_latest = true;
    }

    /// Restore the default zoom and pin the chart to its newest candles.
    pub fn reset_view(&mut self, total: usize) {
        self.visible_count = DEFAULT_VISIBLE.min(total.max(1));
        self.scroll_to_latest(total);
        if total <= DEFAULT_VISIBLE {
            self.viewport_offset = 0.0;
        }
    }

    /// Returns the visible slice indices `[start, end)`.
    pub fn visible_range(&self, total: usize) -> std::ops::Range<usize> {
        let start = (self.viewport_offset.floor() as isize).max(0) as usize;
        let start = start.min(total);
        let end = (start + self.visible_count + 2).min(total);
        start..end
    }
}

impl Default for ChartState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_view_restores_default_zoom_and_pins_to_latest() {
        let mut s = ChartState::new();
        s.visible_count = MIN_VISIBLE;
        s.viewport_offset = 0.0;
        s.pinned_to_latest = false;

        s.reset_view(250);

        assert_eq!(s.visible_count, DEFAULT_VISIBLE);
        assert_eq!(s.viewport_offset, 153.0);
        assert!(s.pinned_to_latest);
    }

    #[test]
    fn reset_view_handles_empty_and_short_history() {
        let mut s = ChartState::new();
        for total in [0, 8] {
            s.visible_count = MIN_VISIBLE;
            s.viewport_offset = 10.0;
            s.pinned_to_latest = false;
            s.reset_view(total);
            assert_eq!(s.visible_count, total.max(1));
            assert_eq!(s.viewport_offset, 0.0);
            assert!(s.pinned_to_latest);
        }
    }

    #[test]
    fn clamp_offset_empty() {
        let mut s = ChartState::new();
        s.viewport_offset = 99.0;
        s.clamp_offset(0);
        assert_eq!(s.viewport_offset, 0.0);
    }

    #[test]
    fn clamp_offset_caps_at_max() {
        let mut s = ChartState::new();
        s.visible_count = 50;
        s.viewport_offset = 999.0;
        s.clamp_offset(100);
        assert_eq!(s.viewport_offset, 50.0);
    }

    #[test]
    fn scroll_by_clamps_to_zero() {
        let mut s = ChartState::new();
        s.viewport_offset = 5.0;
        s.scroll_by(-100, 200);
        assert_eq!(s.viewport_offset, 0.0);
    }

    #[test]
    fn zoom_in_reduces_visible_count() {
        let mut s = ChartState::new();
        s.visible_count = 100;
        s.viewport_offset = 0.0;
        s.zoom(0.5, 300, 0.5);
        assert_eq!(s.visible_count, 50);
    }

    #[test]
    fn zoom_out_clamps_to_max() {
        let mut s = ChartState::new();
        s.visible_count = MAX_VISIBLE;
        s.zoom(2.0, 500, 0.5);
        assert_eq!(s.visible_count, MAX_VISIBLE);
    }

    #[test]
    fn zoom_cursor_anchor_keeps_candle_stationary() {
        let mut s = ChartState::new();
        s.viewport_offset = 50.0;
        s.visible_count = 100;
        // Cursor is at 25% from left → anchor candle index in buffer = 50 + 100*0.25 = 75
        s.zoom(0.5, 500, 0.25);
        // After zoom: visible_count = 50; offset = 75 - 50*0.25 = 75 - 12.5 = 62 → 62
        assert_eq!(s.visible_count, 50);
        assert_eq!(s.viewport_offset, 62.0);
    }

    #[test]
    fn visible_range_is_correct() {
        let mut s = ChartState::new();
        s.viewport_offset = 10.0;
        s.visible_count = 30;
        let r = s.visible_range(100);
        assert_eq!(r, 10..42);
    }
}
