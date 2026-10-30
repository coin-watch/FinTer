use std::rc::Rc;

use gpui::{
    fill, px, rgb, rgba, App, Bounds, Corner, Hsla, PathBuilder, Pixels, SharedString, TextAlign, Window,
};
use gpui_component_macros::IntoPlot;
use num_traits::{Num, ToPrimitive};

use crate::{
    plot::{origin_point, scale::Sealed, Grid, Plot, AXIS_GAP},
    ActiveTheme, PixelsExt,
};

use super::chart_state::{ChartOverlayAction, ChartOverlayHitbox, ChartState};
use super::linear_scale::LinearScale;

// ---------------------------------------------------------------------------
// Position & Order Overlays
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct PositionOverlaySpec {
    pub id: String,
    pub symbol: String,
    pub is_long: bool,
    pub entry_price: f64,
    pub quantity: f64,
    pub margin: f64,
    pub leverage: u32,
    pub current_price: f64,
    pub liquidation_price: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderOverlaySpec {
    pub id: String,
    pub symbol: String,
    pub is_long: bool,
    pub order_type: String,
    pub price: f64,
    pub quantity: f64,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Height fraction reserved for the volume sub-pane (below candles).

/// Vertical gap between the candle area and the volume bars (pixels).
const VOLUME_GAP: f32 = 4.;
/// Font size used for tooltips and axis labels.
const TIP_FONT: f32 = 10.;
/// Estimated character width at TIP_FONT size, used for dynamic Y-axis sizing.
const CHAR_WIDTH_EST: f32 = 6.0;
/// Minimum Y-axis label area width.
const Y_AXIS_MIN_W: f32 = 50.0;
/// Padding inside the Y-axis label area.
const Y_AXIS_PAD: f32 = 6.0;

// ---------------------------------------------------------------------------
// CandleType enum
// ---------------------------------------------------------------------------

/// Visual style for rendering candlesticks.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaneTarget {
    Main,
    Subpane(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IndicatorStyle {
    Line,
    Histogram,
    VolumeHistogram,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CandleType {
    /// Filled body (default). Both bullish and bearish candles are solid.
    #[default]
    Solid,
    /// Outline-only body (hollow). Both bullish and bearish candles are stroked.
    Stroke,
    /// Bullish candles hollow, bearish candles filled.
    UpStroke,
    /// Bearish candles hollow, bullish candles filled.
    DownStroke,
    /// Bar chart style: vertical high-low line with left tick at open, right tick at close.
    Ohlc,
    /// Line chart with area fill below the close line at reduced opacity.
    Area,
    /// Heikin Ashi uses modified OHLC formulas for smoother trends.
    HeikinAshi,
}

// ---------------------------------------------------------------------------
// CandlestickChart
// ---------------------------------------------------------------------------

#[derive(IntoPlot)]
pub struct CandlestickChart<T, Y>
where
    T: 'static,
    Y: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    data: Vec<T>,
    /// Accessor for candle Unix timestamp in milliseconds.
    timestamp_ms: Option<Rc<dyn Fn(&T) -> i64>>,
    open: Option<Rc<dyn Fn(&T) -> Y>>,
    high: Option<Rc<dyn Fn(&T) -> Y>>,
    low: Option<Rc<dyn Fn(&T) -> Y>>,
    close: Option<Rc<dyn Fn(&T) -> Y>>,
    /// Optional per-candle volume value.
    volume: Option<Rc<dyn Fn(&T) -> f64>>,
    /// Body width as a fraction of the per-candle slot width (0.0–1.0).
    body_width_ratio: f32,
    /// Interactive viewport / crosshair state.
    state: Option<ChartState>,
    /// Optional price formatter (for labels and tooltip).
    format_price: Option<Rc<dyn Fn(f64) -> SharedString>>,
    /// Optional timezone-aware timestamp formatters.
    format_timestamp_short: Option<Rc<dyn Fn(i64, f64, Option<i64>) -> SharedString>>,
    format_timestamp_long: Option<Rc<dyn Fn(i64) -> SharedString>>,
    /// Visual style for candles.
    candle_type: CandleType,
    /// Toggle for in-plot horizontal dashed guide lines.
    show_horizontal_grid: bool,
    show_candles: bool,
    show_volume_pane: bool,
    subpane_count: usize,
    /// Timestamp/value series painted over the price pane.
    indicator_lines: Vec<(Vec<(i64, f64)>, Hsla, IndicatorStyle, PaneTarget)>,
    /// Active trading position overlays (entry line, PnL badge, liquidation line).
    position_overlays: Vec<PositionOverlaySpec>,
    /// Open order overlays (price line, type/side badge, cancel action).
    order_overlays: Vec<OrderOverlaySpec>,
}

impl<T, Y> CandlestickChart<T, Y>
where
    Y: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    pub fn new<I>(data: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: data.into_iter().collect(),
            timestamp_ms: None,
            open: None,
            high: None,
            low: None,
            close: None,
            volume: None,
            body_width_ratio: 0.8,
            state: None,
            format_price: None,
            format_timestamp_short: None,
            format_timestamp_long: None,
            candle_type: CandleType::default(),
            show_horizontal_grid: false,
            show_candles: true,
            show_volume_pane: true,
            subpane_count: 0,
            indicator_lines: Vec::new(),
            position_overlays: Vec::new(),
            order_overlays: Vec::new(),
        }
    }

    /// Attach a Unix-millisecond timestamp accessor (replaces `x()`).
    pub fn timestamp_ms(mut self, f: impl Fn(&T) -> i64 + 'static) -> Self {
        self.timestamp_ms = Some(Rc::new(f));
        self
    }

    /// Backwards-compatible alias: accept a string-returning x accessor and
    /// silently ignore it (timestamp_ms is required for correct rendering).
    #[allow(dead_code)]
    pub fn x<X>(self, _f: impl Fn(&T) -> X + 'static) -> Self {
        // No-op — kept for call-site compatibility during transition.
        self
    }

    pub fn open(mut self, open: impl Fn(&T) -> Y + 'static) -> Self {
        self.open = Some(Rc::new(open));
        self
    }

    pub fn high(mut self, high: impl Fn(&T) -> Y + 'static) -> Self {
        self.high = Some(Rc::new(high));
        self
    }

    pub fn low(mut self, low: impl Fn(&T) -> Y + 'static) -> Self {
        self.low = Some(Rc::new(low));
        self
    }

    pub fn close(mut self, close: impl Fn(&T) -> Y + 'static) -> Self {
        self.close = Some(Rc::new(close));
        self
    }

    /// Attach per-candle volume data for the volume sub-pane.
    pub fn volume(mut self, volume: impl Fn(&T) -> f64 + 'static) -> Self {
        self.volume = Some(Rc::new(volume));
        self
    }

    /// Kept for call-site compatibility; unused now that X-axis is timestamp-based.
    #[allow(dead_code)]
    pub fn tick_margin(self, _tick_margin: usize) -> Self {
        self
    }

    pub fn body_width_ratio(mut self, ratio: f32) -> Self {
        self.body_width_ratio = ratio;
        self
    }

    /// Attach chart interaction state (viewport offset, zoom, crosshair).
    pub fn chart_state(mut self, state: ChartState) -> Self {
        self.state = Some(state);
        self
    }

    /// Optional price formatter for Y-axis labels and tooltip.
    pub fn format_price(mut self, f: impl Fn(f64) -> SharedString + 'static) -> Self {
        self.format_price = Some(Rc::new(f));
        self
    }

    /// Format X-axis and short crosshair timestamps using the caller's timezone.
    pub fn format_timestamp_short(
        mut self,
        f: impl Fn(i64, f64, Option<i64>) -> SharedString + 'static,
    ) -> Self {
        self.format_timestamp_short = Some(Rc::new(f));
        self
    }

    /// Format detailed crosshair timestamps using the caller's timezone.
    pub fn format_timestamp_long(mut self, f: impl Fn(i64) -> SharedString + 'static) -> Self {
        self.format_timestamp_long = Some(Rc::new(f));
        self
    }

    /// Set the visual style for rendering candles.
    pub fn candle_type(mut self, ct: CandleType) -> Self {
        self.candle_type = ct;
        self
    }

    /// Toggle in-plot horizontal dashed guide lines.
    pub fn show_horizontal_grid(mut self, show: bool) -> Self {
        self.show_horizontal_grid = show;
        self
    }

    /// Add an indicator line (a series of timestamp -> value points) to be drawn over the price chart.
    pub fn indicator_line(mut self, series: Vec<(i64, f64)>, color: Hsla, style: IndicatorStyle, pane: PaneTarget) -> Self {
        self.indicator_lines.push((series, color, style, pane));
        self
    }

    pub fn show_candles(mut self, show: bool) -> Self {
        self.show_candles = show;
        self
    }

    pub fn subpane_count(mut self, count: usize) -> Self {
        self.subpane_count = count;
        self
    }

    pub fn show_volume_pane(mut self, show: bool) -> Self {
        self.show_volume_pane = show;
        self
    }

    /// Attach active position overlay specs (entry price lines, PnL badges, liquidation lines).
    pub fn position_overlays(mut self, overlays: Vec<PositionOverlaySpec>) -> Self {
        self.position_overlays = overlays;
        self
    }

    /// Attach open order overlay specs (order price lines, side/type badges, cancel action).
    pub fn order_overlays(mut self, overlays: Vec<OrderOverlaySpec>) -> Self {
        self.order_overlays = overlays;
        self
    }
}

// ---------------------------------------------------------------------------
// Plot implementation
// ---------------------------------------------------------------------------

impl<T, Y> Plot for CandlestickChart<T, Y>
where
    Y: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    #[allow(clippy::too_many_lines)]
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (Some(ts_fn), Some(open_fn), Some(high_fn), Some(low_fn), Some(close_fn)) = (
            self.timestamp_ms.as_ref(),
            self.open.as_ref(),
            self.high.as_ref(),
            self.low.as_ref(),
            self.close.as_ref(),
        ) else {
            return;
        };

        if self.data.is_empty() {
            return;
        }

        // ------------------------------------------------------------------ //
        // 1. Determine visible slice via ChartState
        // ------------------------------------------------------------------ //
        let total = self.data.len();
        let (vis_start, vis_end) = if let Some(ref st) = self.state {
            let r = st.visible_range(total);
            (r.start, r.end)
        } else {
            (0, total)
        };

        let visible: Vec<&T> = self.data[vis_start..vis_end].iter().collect();
        if visible.is_empty() {
            return;
        }

        // ------------------------------------------------------------------ //
        // 2. Layout rectangles
        // ------------------------------------------------------------------ //
        let full_w = bounds.size.width.as_f32();
        let full_h = bounds.size.height.as_f32();
        if !full_w.is_finite() || !full_h.is_finite() || full_w <= 1.0 || full_h <= 1.0 {
            return;
        }
        let _has_volume = self.volume.is_some() || self.indicator_lines.iter().any(|l| matches!(l.3, PaneTarget::Subpane(_)));

        // Collect visible timestamps early; we use interval for countdown sizing on Y-axis.
        let timestamps: Vec<i64> = visible.iter().map(|d| ts_fn(d)).collect();

        // Calculate dynamic best_interval for X-axis labels globally
        let chart_w_est = bounds.size.width.as_f32() - 60.0;
        let x_tick_count_est = (chart_w_est / 150.0).max(2.0) as usize;
        let ts_min_val = *timestamps.first().unwrap_or(&0);
        let ts_max_val = *timestamps.last().unwrap_or(&0);
        let time_span_est = ts_max_val.saturating_sub(ts_min_val) as f64;
        let target_interval_est = time_span_est / x_tick_count_est.max(1) as f64;
        let nice_intervals: [i64; 13] = [
            60_000, 5 * 60_000, 15 * 60_000, 30 * 60_000,
            3600_000, 4 * 3600_000, 12 * 3600_000,
            86400_000, 2 * 86400_000, 7 * 86400_000,
            30 * 86400_000, 90 * 86400_000, 365 * 86400_000
        ];
        let mut best_interval: f64 = nice_intervals[0] as f64;
        let mut min_diff = f64::MAX;
        for &int_val in &nice_intervals {
            let diff = ((int_val as f64) - target_interval_est).abs();
            if diff < min_diff {
                min_diff = diff;
                best_interval = int_val as f64;
            }
        }

        let n = timestamps.len();
        let interval_ms: i64 = if n >= 2 {
            let mut gaps: Vec<i64> = timestamps
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .filter(|&g| g > 0)
                .collect();
            gaps.sort_unstable();
            *gaps.get(gaps.len() / 2).unwrap_or(&60_000)
        } else {
            60_000 // default 1m
        };

        // Determine Y-axis width dynamically based on the longest price label.
        let all_y_vals: Vec<f64> = visible
            .iter()
            .flat_map(|d| {
                vec![
                    high_fn(d).to_f64().unwrap_or(0.0),
                    low_fn(d).to_f64().unwrap_or(0.0),
                    open_fn(d).to_f64().unwrap_or(0.0),
                    close_fn(d).to_f64().unwrap_or(0.0),
                ]
            })
            .collect();

        let y_domain_min = all_y_vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let y_domain_max = all_y_vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        // Guard: degenerate y-domain (all candles at same price, or NaN/Inf)
        if !y_domain_min.is_finite() || !y_domain_max.is_finite() || y_domain_max <= y_domain_min {
            return;
        }

        // Use the widest of price/countdown labels for dynamic Y-axis width.
        // Include both exact-interval and interval-1s countdown variants because
        // boundary values like "1h" become longer strings like "59m:59s".
        let sample_label = self.price_str(y_domain_max);
        let interval_secs = (interval_ms / 1_000).max(0);
        let countdown_sample_exact: SharedString = format_countdown_duration(interval_secs).into();
        let countdown_sample_worst: SharedString =
            format_countdown_duration(interval_secs.saturating_sub(1)).into();
        let font_size = px(TIP_FONT);
        let measure_text_width = |text: &SharedString| -> f32 {
            let text_run = gpui::TextRun {
                len: text.len(),
                font: window.text_style().font(),
                color: gpui::white(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };

            window
                .text_system()
                .shape_text(text.clone(), font_size, &[text_run], None, None)
                .ok()
                .and_then(|lines| lines.into_iter().next())
                .map(|line| line.size(font_size).width.as_f32())
                .unwrap_or(text.len() as f32 * CHAR_WIDTH_EST)
        };
        let widest_label_w = measure_text_width(&sample_label)
            .max(measure_text_width(&countdown_sample_exact))
            .max(measure_text_width(&countdown_sample_worst));
        let y_axis_w = (widest_label_w + Y_AXIS_PAD * 2.0 + 2.0).max(Y_AXIS_MIN_W);

        // Candle drawing area: full width minus Y-axis on the right.
        let chart_w = full_w - y_axis_w;
        if !chart_w.is_finite() || chart_w <= 1.0 {
            return;
        }

        let has_subpanes = self.subpane_count > 0;
        let mut total_subpane_ratio = 0.0;
        let mut subpane_ratios = vec![0.18; self.subpane_count];
        if let Some(st) = &self.state {
            let ratios = st.subpane_ratios.borrow();
            for i in 0..self.subpane_count {
                subpane_ratios[i] = *ratios.get(&i).unwrap_or(&0.18);
                total_subpane_ratio += subpane_ratios[i];
            }
        }
        
        if total_subpane_ratio > 0.8 {
            let scale = 0.8 / total_subpane_ratio;
            for i in 0..self.subpane_count {
                subpane_ratios[i] *= scale;
            }
            total_subpane_ratio = 0.8;
        }

        let subpane_total_gap = if has_subpanes { VOLUME_GAP * self.subpane_count as f32 } else { 0.0 };
        
        let avail = full_h - AXIS_GAP;
        let candle_h = if has_subpanes {
            (avail * (1.0 - total_subpane_ratio) - subpane_total_gap).max(10.0)
        } else {
            avail.max(10.0)
        };
        
        if !candle_h.is_finite() || candle_h <= 1.0 {
            return;
        }

        if let Some(st) = &self.state {
            st.overlay_hitboxes.borrow_mut().clear();
        }

        // ------------------------------------------------------------------ //
        // 3. Timestamp-based X scale (LinearScale over Unix ms)
        // ------------------------------------------------------------------ //
        let visible_count_f = if let Some(ref st) = self.state {
            st.visible_count as f64
        } else {
            100.0
        };

        let mut ts_min = *timestamps.first().unwrap_or(&0) as f64;
        let mut ts_max = ts_min + (visible_count_f - 1.0f64) * (interval_ms as f64);

        // Apply panning shift to X-axis domain to support dragging into empty space
        let shift_candles = if let Some(ref st) = self.state {
            let actual_start = (st.viewport_offset.floor() as isize).max(0) as f64;
            actual_start - st.viewport_offset
        } else {
            0.0
        };
        let ts_shift = shift_candles * (interval_ms as f64);
        ts_min -= ts_shift;
        ts_max -= ts_shift;

        let x_scale = LinearScale::new(ts_min, ts_max, 0.0, chart_w);

        // ------------------------------------------------------------------ //
        // 4. Y scale (price → pixel, inverted)
        // ------------------------------------------------------------------ //
        // Re-calculate y_domain to ONLY include candles physically on-screen
        // Compute Heikin Ashi data if needed
        let mut ha_data = Vec::with_capacity(visible.len());
        if self.candle_type == CandleType::HeikinAshi {
            let mut prev_ha_open = 0.0;
            let mut prev_ha_close = 0.0;
            for (i, c) in visible.iter().enumerate() {
                let open = open_fn(c).to_f64().unwrap_or(0.0);
                let high = high_fn(c).to_f64().unwrap_or(0.0);
                let low = low_fn(c).to_f64().unwrap_or(0.0);
                let close = close_fn(c).to_f64().unwrap_or(0.0);

                let ha_close = (open + high + low + close) / 4.0;
                let ha_open = if i == 0 {
                    (open + close) / 2.0
                } else {
                    (prev_ha_open + prev_ha_close) / 2.0
                };
                let ha_high = high.max(ha_open).max(ha_close);
                let ha_low = low.min(ha_open).min(ha_close);

                ha_data.push((ha_open, ha_high, ha_low, ha_close));
                prev_ha_open = ha_open;
                prev_ha_close = ha_close;
            }
        }

        let visible_y_vals: Vec<f64> = visible
            .iter()
            .enumerate()
            .zip(timestamps.iter())
            .filter(|&((_, _), &ts)| (ts as f64) >= ts_min && (ts as f64) <= ts_max)
            .flat_map(|((i, d), _)| {
                if self.candle_type == CandleType::HeikinAshi {
                    let (ho, hh, hl, hc) = ha_data[i];
                    vec![hh, hl, ho, hc]
                } else {
                    vec![
                        high_fn(d).to_f64().unwrap_or(0.0),
                        low_fn(d).to_f64().unwrap_or(0.0),
                        open_fn(d).to_f64().unwrap_or(0.0),
                        close_fn(d).to_f64().unwrap_or(0.0),
                    ]
                }
            })
            .collect();

        let mut y_domain_min = visible_y_vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let mut y_domain_max = visible_y_vals
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        if y_domain_min.is_infinite() || y_domain_max.is_infinite() {
            y_domain_min = 0.0;
            y_domain_max = 100.0;
        } else if (y_domain_max - y_domain_min).abs() < f64::EPSILON {
            y_domain_min *= 0.9;
            y_domain_max *= 1.1;
        }

        // Add 5% padding to Y range so candles don't touch the edges.
        let y_range = (y_domain_max - y_domain_min).max(f64::EPSILON);
        let y_pad = y_range * 0.05;
        let y_min_padded = y_domain_min - y_pad;
        let y_max_padded = y_domain_max + y_pad;

        // Generate nice Y-axis ticks
        let mut y_ticks = Vec::new();
        let y_range = y_max_padded - y_min_padded;
        if y_range > 0.0 {
            let max_ticks = ((candle_h as f64) / 60.0).max(3.0);
            let exponent = (y_range / max_ticks).log10().floor();
            let fraction = (y_range / max_ticks) / 10_f64.powf(exponent);
            let tick_spacing = if fraction < 1.5 {
                1.0
            } else if fraction < 3.0 {
                2.0
            } else if fraction < 7.0 {
                5.0
            } else {
                10.0
            } * 10_f64.powf(exponent);

            let nice_min = (y_min_padded / tick_spacing).ceil() * tick_spacing;
            let nice_max = (y_max_padded / tick_spacing).floor() * tick_spacing;

            let mut val = nice_min;
            while val <= nice_max + 1e-9 {
                y_ticks.push(val);
                val += tick_spacing;
            }
        } else {
            y_ticks.push(y_min_padded);
        }

        // range_min=candle_h (bottom), range_max=10 (top) → inverted axis
        let y_scale = LinearScale::new(y_min_padded, y_max_padded, candle_h, 10.0);

        // ------------------------------------------------------------------ //
        // 5. In-plot horizontal grid lines
        // ------------------------------------------------------------------ //
        if self.show_horizontal_grid {
            let grid_y_coords: Vec<f32> =
                y_ticks.iter().map(|&price| y_scale.scale(price)).collect();
            Grid::new()
                .y(grid_y_coords)
                .stroke(cx.theme().muted_foreground.opacity(0.35))
                .dash_array(&[px(4.), px(2.)])
                .paint(&bounds, window);
        }

        // ------------------------------------------------------------------ //
        // 8. Draw candlesticks (or alternative chart types)
        // ------------------------------------------------------------------ //
        let origin = bounds.origin;
        if let Some(ref st) = self.state {
            st.candle_plot_origin.set(Some((origin.x.as_f32(), origin.y.as_f32())));
            st.candle_plot_size.set(Some((chart_w, candle_h)));
            st.candle_ts_range.set(Some((ts_min, ts_max)));
            st.candle_price_range.set(Some((y_min_padded, y_max_padded)));
        }
        let bullish_color = cx.theme().bullish;
        let bearish_color = cx.theme().bearish;
        
        let is_light = cx.theme().background.l > 0.5;
        let (crosshair_bg_raw, crosshair_line_raw) = if is_light {
            (gpui::rgb(0x686D76), gpui::rgb(0x76808F))
        } else {
            (gpui::rgb(0x373a40), gpui::rgb(0x929AA5))
        };
        let crosshair_bg: gpui::Hsla = crosshair_bg_raw.into();
        let crosshair_line: gpui::Hsla = crosshair_line_raw.into();
        let candle_type = self.candle_type;

        // Per-candle slot width in pixels.
        let slot_w = if n > 1 {
            let first_px = x_scale.scale(timestamps[0] as f64);
            let second_px = x_scale.scale(timestamps[1] as f64);
            (second_px - first_px).abs()
        } else {
            chart_w / 10.0
        };
        let body_w = (slot_w * self.body_width_ratio).max(1.0);

        let mut period_high_f = f64::NEG_INFINITY;
        let mut period_low_f = f64::INFINITY;
        let mut period_high_px = (0f32, 0f32);
        let mut period_low_px = (0f32, 0f32);

        // Per-candle centre X pixel (relative to chart area, not bounds).
        let mut centres_x: Vec<f32> = Vec::with_capacity(n);
        // For Area type: collect (x, close_y) points.
        let mut area_points: Vec<(f32, f32)> = Vec::with_capacity(n);

        // Determine which candle is being hovered (if any)
        let hovered_candle_idx: Option<usize> = if let Some(ref st) = self.state {
            if let Some(cursor) = st.crosshair_pos {
                let cursor_rel = cursor - origin;
                let cursor_x_in_chart = cursor_rel.x.as_f32();
                if chart_w > 0.0 {
                    st.last_cursor_frac.set(cursor_x_in_chart / chart_w);
                }
                if cursor_x_in_chart >= 0.0 && cursor_x_in_chart <= chart_w {
                    let cursor_ts = x_scale.inverse(cursor_x_in_chart);
                    timestamps
                        .iter()
                        .enumerate()
                        .min_by_key(|&(_, ts)| (*ts as f64 - cursor_ts).abs() as i64)
                        .map(|(idx, _)| idx)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        for (i, (d, &ts)) in visible.iter().zip(timestamps.iter()).enumerate() {
            let center_x = x_scale.scale(ts as f64);
            centres_x.push(center_x);

            let mut open_v = open_fn(d).to_f64().unwrap_or(0.0);
            let mut high_v = high_fn(d).to_f64().unwrap_or(0.0);
            let mut low_v = low_fn(d).to_f64().unwrap_or(0.0);
            let mut close_v = close_fn(d).to_f64().unwrap_or(0.0);

            if self.candle_type == CandleType::HeikinAshi {
                let (ho, hh, hl, hc) = ha_data[i];
                open_v = ho;
                high_v = hh;
                low_v = hl;
                close_v = hc;
            }

            let open_y = y_scale.scale(open_v);
            let high_y = y_scale.scale(high_v);
            let low_y = y_scale.scale(low_v);
            let close_y = y_scale.scale(close_v);

            let is_bullish = close_v >= open_v;
            let color: Hsla = if is_bullish {
                bullish_color
            } else {
                bearish_color
            };

            // Add subtle glow effect if this candle is being hovered
            let is_hovered = hovered_candle_idx == Some(i);
            let color = if is_hovered {
                // Brighten the color slightly when hovered
                Hsla {
                    h: color.h,
                    s: color.s,
                    l: (color.l * 1.2).min(1.0),
                    a: color.a,
                }
            } else {
                color
            };

            // Track period high / low.
            if high_v > period_high_f {
                period_high_f = high_v;
                period_high_px = (center_x, high_y);
            }
            if low_v < period_low_f {
                period_low_f = low_v;
                period_low_px = (center_x, low_y);
            }



            // Collect for Area chart.
            area_points.push((center_x, close_y));

            let body_left = center_x - body_w / 2.;
            let body_right = center_x + body_w / 2.;



            if self.show_candles {
                match candle_type {
                CandleType::Area => {
                    // Area chart draws a line + fill after the loop; skip per-candle rendering.
                }
                CandleType::HeikinAshi => {
                    // Same as Solid, but data is transformed externally.
                    // Wick (always drawn as a line).
                    let mut wick = PathBuilder::stroke(px(1.));
                    wick.move_to(origin_point(px(center_x), px(high_y), origin));
                    wick.line_to(origin_point(px(center_x), px(low_y), origin));
                    if let Ok(path) = wick.build() {
                        window.paint_path(path, color);
                    }

                    // Body.
                    let (top, bottom) = if is_bullish {
                        (close_y, open_y)
                    } else {
                        (open_y, close_y)
                    };
                    let bottom = bottom.max(top + 1.0); // ensure doji is visible
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            origin_point(px(body_left), px(top), origin),
                            origin_point(px(body_right), px(bottom), origin),
                        ),
                        color,
                    ));
                }
                CandleType::Ohlc => {
                    // OHLC bar: vertical high-low line + left tick at open + right tick at close.
                    let tick_len = body_w / 2.0;

                    // Vertical high-low line.
                    let mut hl = PathBuilder::stroke(px(1.));
                    hl.move_to(origin_point(px(center_x), px(high_y), origin));
                    hl.line_to(origin_point(px(center_x), px(low_y), origin));
                    if let Ok(p) = hl.build() {
                        window.paint_path(p, color);
                    }

                    // Left tick at open.
                    let mut ot = PathBuilder::stroke(px(1.));
                    ot.move_to(origin_point(px(center_x - tick_len), px(open_y), origin));
                    ot.line_to(origin_point(px(center_x), px(open_y), origin));
                    if let Ok(p) = ot.build() {
                        window.paint_path(p, color);
                    }

                    // Right tick at close.
                    let mut ct = PathBuilder::stroke(px(1.));
                    ct.move_to(origin_point(px(center_x), px(close_y), origin));
                    ct.line_to(origin_point(px(center_x + tick_len), px(close_y), origin));
                    if let Ok(p) = ct.build() {
                        window.paint_path(p, color);
                    }
                }
                CandleType::Solid
                | CandleType::Stroke
                | CandleType::UpStroke
                | CandleType::DownStroke => {
                    // Determine whether body should be filled or stroked.
                    let use_stroke = match candle_type {
                        CandleType::Stroke => true,
                        CandleType::UpStroke => is_bullish,
                        CandleType::DownStroke => !is_bullish,
                        _ => false, // Solid
                    };

                    // Body.
                    let (top, bottom) = if is_bullish {
                        (close_y, open_y)
                    } else {
                        (open_y, close_y)
                    };
                    let bottom = bottom.max(top + 1.0); // ensure doji is visible

                    // Wick (always drawn as a line).
                    // If hollow, don't draw the wick *through* the body.
                    if use_stroke {
                        let mut top_wick = PathBuilder::stroke(px(1.));
                        top_wick.move_to(origin_point(px(center_x), px(high_y), origin));
                        top_wick.line_to(origin_point(px(center_x), px(top), origin));
                        if let Ok(path) = top_wick.build() {
                            window.paint_path(path, color);
                        }
                        
                        let mut bot_wick = PathBuilder::stroke(px(1.));
                        bot_wick.move_to(origin_point(px(center_x), px(bottom), origin));
                        bot_wick.line_to(origin_point(px(center_x), px(low_y), origin));
                        if let Ok(path) = bot_wick.build() {
                            window.paint_path(path, color);
                        }
                    } else {
                        let mut wick = PathBuilder::stroke(px(1.));
                        wick.move_to(origin_point(px(center_x), px(high_y), origin));
                        wick.line_to(origin_point(px(center_x), px(low_y), origin));
                        if let Ok(path) = wick.build() {
                            window.paint_path(path, color);
                        }
                    }

                    if use_stroke {
                        // Stroked (hollow) body.
                        let mut body = PathBuilder::stroke(px(1.));
                        body.move_to(origin_point(px(body_left), px(top), origin));
                        body.line_to(origin_point(px(body_right), px(top), origin));
                        body.line_to(origin_point(px(body_right), px(bottom), origin));
                        body.line_to(origin_point(px(body_left), px(bottom), origin));
                        body.line_to(origin_point(px(body_left), px(top), origin));
                        if let Ok(p) = body.build() {
                            window.paint_path(p, color);
                        }
                    } else {
                        // Filled body.
                        window.paint_quad(fill(
                            Bounds::from_corners(
                                origin_point(px(body_left), px(top), origin),
                                origin_point(px(body_right), px(bottom), origin),
                            ),
                            color,
                        ));
                    }
                }
                }
            }
        }

        // Draw area fill + line for Area chart type.
        if self.show_candles && candle_type == CandleType::Area && !area_points.is_empty() {
            let line_color = bullish_color;
            let mut fill_color = bullish_color;
            fill_color.a = 0.2; // Opacity applied here

            // Fill polygon: first point down to bottom, across bottom, up to last point, then close line path.
            let first = area_points.first().unwrap();
            let last = area_points.last().unwrap();
            let bottom_y = candle_h; // bottom of candle area

            let mut fill_path = PathBuilder::fill();
            fill_path.move_to(origin_point(px(first.0), px(bottom_y), origin));
            for &(ax, ay) in &area_points {
                fill_path.line_to(origin_point(px(ax), px(ay), origin));
            }
            fill_path.line_to(origin_point(px(last.0), px(bottom_y), origin));
            fill_path.line_to(origin_point(px(first.0), px(bottom_y), origin));
            if let Ok(p) = fill_path.build() {
                window.paint_path(p, fill_color);
            }

            // Line stroke along close prices.
            let mut line_path = PathBuilder::stroke(px(2.));
            let mut first_point = true;
            for &(ax, ay) in &area_points {
                if first_point {
                    line_path.move_to(origin_point(px(ax), px(ay), origin));
                    first_point = false;
                } else {
                    line_path.line_to(origin_point(px(ax), px(ay), origin));
                }
            }
            if let Ok(p) = line_path.build() {
                window.paint_path(p, line_color);
            }
        }

        // ------------------------------------------------------------------ //
                // ------------------------------------------------------------------ //
        // Custom indicator lines
        // ------------------------------------------------------------------ //
        
        let mut grouped_lines: std::collections::HashMap<PaneTarget, Vec<&(Vec<(i64, f64)>, Hsla, IndicatorStyle, PaneTarget)>> = std::collections::HashMap::new();
        for line in &self.indicator_lines {
            grouped_lines.entry(line.3).or_default().push(line);
        }

        let draw_lines = |window: &mut gpui::Window, pane_y0: f32, pane_h: f32, lines: &[&(Vec<(i64, f64)>, Hsla, IndicatorStyle, PaneTarget)], min_val: f64, max_val: f64| {
            let scale_y = |val: f64| {
                if (max_val - min_val).abs() < f64::EPSILON { pane_y0 + pane_h / 2.0 }
                else { pane_y0 + pane_h * (1.0 - (val - min_val) / (max_val - min_val)) as f32 }
            };

            for (series, color, style, _) in lines {
                if *style == IndicatorStyle::Line {
                    let mut line_path = gpui::PathBuilder::stroke(gpui::px(1.5));
                    let mut started = false;
                    for &(ts, val) in series {
                        let px_x = x_scale.scale(ts as f64);
                        if px_x < 0.0 || px_x > chart_w || val.is_nan() { continue; }
                        let y_abs = origin.y.as_f32() + scale_y(val);
                        let x_abs = origin.x.as_f32() + px_x;
                        if !started {
                            line_path.move_to(gpui::point(gpui::px(x_abs), gpui::px(y_abs)));
                            started = true;
                        } else {
                            line_path.line_to(gpui::point(gpui::px(x_abs), gpui::px(y_abs)));
                        }
                    }
                    if started {
                        if let Ok(p) = line_path.build() {
                            window.paint_path(p, *color);
                        }
                    }
                } else if *style == IndicatorStyle::Histogram {
                    let zero_y = scale_y(0.0).clamp(pane_y0, pane_y0 + pane_h);
                    let bar_w = (chart_w / visible.len() as f32 * 0.8).clamp(1.0, 10.0);
                    
                    for &(ts, val) in series {
                        let px_x = x_scale.scale(ts as f64);
                        if px_x < 0.0 || px_x > chart_w || val.is_nan() { continue; }
                        let y_abs = scale_y(val);
                        let x_abs = origin.x.as_f32() + px_x - bar_w / 2.0;

                        let bar_color = if val >= 0.0 { bullish_color.opacity(0.5) } else { bearish_color.opacity(0.5) };

                        let y_top = origin.y.as_f32() + y_abs.min(zero_y);
                        let y_bottom = origin.y.as_f32() + y_abs.max(zero_y).max(y_abs.min(zero_y) + 1.0);
                        
                        let bounds = gpui::Bounds::from_corners(
                            gpui::point(gpui::px(x_abs), gpui::px(y_top)),
                            gpui::point(gpui::px(x_abs + bar_w), gpui::px(y_bottom))
                        );
                        window.paint_quad(gpui::fill(bounds, bar_color));
                    }
                }
            }
        };

        let mut subpane_bounds: Vec<(f32, f32, f64, f64)> = Vec::new();
        if self.show_candles {
            if let Some(main_lines) = grouped_lines.get(&PaneTarget::Main) {
                draw_lines(window, 0.0, candle_h, main_lines, y_domain_min, y_domain_max);
            }
        }

        if has_subpanes {
            for i in 0..self.subpane_count {
                let mut pane_y0 = candle_h + VOLUME_GAP;
                for j in 0..i { pane_y0 += avail * subpane_ratios[j] + VOLUME_GAP; }
                let pane_h = avail * subpane_ratios[i];
                if let Some(lines) = grouped_lines.get(&PaneTarget::Subpane(i)) {
                    let mut p_min = f64::INFINITY;
                    let mut p_max = f64::NEG_INFINITY;
                    for (series, _, style, _) in lines {
                        for &(ts, val) in series {
                            let px_x = x_scale.scale(ts as f64);
                            if px_x >= 0.0 && px_x <= chart_w && !val.is_nan() {
                                p_min = p_min.min(val);
                                p_max = p_max.max(val);
                            }
                        }
                        if *style == IndicatorStyle::Histogram {
                            p_min = p_min.min(0.0);
                            p_max = p_max.max(0.0);
                        }
                    }
                    if p_min.is_infinite() { p_min = 0.0; p_max = 100.0; }
                    if (p_max - p_min).abs() < f64::EPSILON { p_min *= 0.9; p_max *= 1.1; }
                    let pad = (p_max - p_min) * 0.1;
                    draw_lines(window, pane_y0, pane_h, lines, p_min - pad, p_max + pad);
                    subpane_bounds.push((origin.y.as_f32() + pane_y0, pane_h, p_min - pad, p_max + pad));
                    
                    // Separator for every subpane
                    let sep_y = origin.y.as_f32() + pane_y0 - VOLUME_GAP / 2.;
                    if let Some(st) = &self.state {
                        st.separator_ys.borrow_mut().insert(i, sep_y);
                    }
                    let mut sep = gpui::PathBuilder::stroke(gpui::px(1.));
                    sep.move_to(gpui::point(gpui::px(origin.x.as_f32()), gpui::px(sep_y)));
                    sep.line_to(gpui::point(gpui::px(origin.x.as_f32() + chart_w), gpui::px(sep_y)));
                    if let Ok(p) = sep.build() { window.paint_path(p, cx.theme().border); }
                    if i == 0 {
                        if let Some(st) = &self.state {
                            st.separator_ys.borrow_mut().insert(0, sep_y);
                            st.chart_height.set(Some(full_h));
                        }
                    }
                }
            }
        }

        // If the chart is panned to the past, the latest candle in the data feed might not be in the visible window!, the latest candle in the data feed might not be in the visible window!
        // We must use the feed's absolute last candle for the countdown timer and live price, not the visible window's last candle.
        let absolute_last_ts = self.data.last().map(|c| ts_fn(c)).unwrap_or(0);
        let (last_close_f, last_candle_is_bullish) = if let Some(last_candle) = self.data.last() {
            let close_fn = self.close.as_ref().unwrap();
            let open_fn = self.open.as_ref().unwrap();
            let close_v = close_fn(last_candle);
            let open_v = open_fn(last_candle);
            let close_f = num_traits::ToPrimitive::to_f64(&close_v).unwrap_or(0.0);
            let open_f = num_traits::ToPrimitive::to_f64(&open_v).unwrap_or(0.0);
            (close_f, close_f >= open_f)
        } else {
            (0.0, true)
        };

        // Shared Y-axis badge width across live price, countdown, and crosshair price.
        let live_label_for_width = if last_close_f > 0.0 {
            self.price_str(last_close_f)
        } else {
            self.price_str(y_domain_max)
        };
        let last_ts_for_width = absolute_last_ts;
        let now_ms_for_width = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let next_open_for_width = if interval_ms > 0 && last_ts_for_width > 0 {
            if now_ms_for_width < last_ts_for_width + interval_ms {
                last_ts_for_width + interval_ms
            } else {
                let elapsed = now_ms_for_width - last_ts_for_width;
                last_ts_for_width + ((elapsed / interval_ms) + 1) * interval_ms
            }
        } else if interval_ms > 0 {
            ((now_ms_for_width / interval_ms) + 1) * interval_ms
        } else {
            last_ts_for_width + interval_ms
        };
        let remaining_ms_for_width = (next_open_for_width.saturating_sub(now_ms_for_width)).max(0);
        let countdown_label_for_width: SharedString =
            format_countdown_duration(remaining_ms_for_width / 1000).into();
        let shared_live_badge_w = self
            .pill_width_for_text(&live_label_for_width, y_axis_w, window)
            .max(self.pill_width_for_text(&countdown_label_for_width, y_axis_w, window));

        // ------------------------------------------------------------------ //
        // 11. Volume sub-pane
        // ------------------------------------------------------------------ //
        if self.show_volume_pane {
            if let Some(ref vol_fn) = self.volume.clone() {
                let vols: Vec<f64> = visible.iter().map(|d| vol_fn(d)).collect();
            let max_vol = vols.iter().cloned().fold(0.0_f64, f64::max);
            if max_vol > 0.0 {
                // Separator line.
                let sep_y = origin.y.as_f32() + (candle_h + VOLUME_GAP) - VOLUME_GAP / 2.;
                
                // Save it to state if present for hit-testing the resize handle
                if let Some(st) = &self.state {
                    st.separator_ys.borrow_mut().insert(0, sep_y);
                    st.chart_height.set(Some(full_h));
                }
                let mut sep = PathBuilder::stroke(px(1.));
                sep.move_to(gpui::point(px(origin.x.as_f32()), px(sep_y)));
                sep.line_to(gpui::point(px(origin.x.as_f32() + chart_w), px(sep_y)));
                if let Ok(p) = sep.build() {
                    window.paint_path(p, cx.theme().border);
                }

                for (d, (&ts, &vol)) in visible.iter().zip(timestamps.iter().zip(vols.iter())) {
                    let is_bull =
                        close_fn(d).to_f64().unwrap_or(0.0) >= open_fn(d).to_f64().unwrap_or(0.0);
                    let color: Hsla = if is_bull {
                        bullish_color.opacity(0.5)
                    } else {
                        bearish_color.opacity(0.5)
                    };
                    let center_x = x_scale.scale(ts as f64);
                    let pane_h = avail * subpane_ratios[0];
                    let bar_h = ((vol / max_vol) as f32 * pane_h).max(1.0);
                    let bx = center_x - body_w / 2.;
                    let pane_h = avail * subpane_ratios[0];
                let by = (candle_h + VOLUME_GAP) + (pane_h - bar_h);
                    window.paint_quad(fill(
                        Bounds::from_corners(
                            origin_point(px(bx), px(by), origin),
                            origin_point(px(bx + body_w), px(by + bar_h), origin),
                        ),
                        color,
                    ));
                }
            }
        }
        }

        // ------------------------------------------------------------------ //
        // ------------------------------------------------------------------ //
        // 11.5 Solid Axes Background Masks
        // ------------------------------------------------------------------ //
        {
            let bg = cx.theme().background;

            // Y-axis mask (right edge)
            if y_axis_w > 0.0 {
                let y_mask = Bounds::from_corners(
                    gpui::point(bounds.origin.x + px(chart_w), bounds.origin.y),
                    gpui::point(
                        bounds.origin.x + bounds.size.width,
                        bounds.origin.y + bounds.size.height,
                    ),
                );
                window.paint_quad(fill(y_mask, bg));
            }
        }

        // 11.6 Axis Border Lines
        // ------------------------------------------------------------------ //
        {
            let border_color = cx.theme().border;
            // Vertical line separating chart from Y-axis
            if y_axis_w > 0.0 {
                let mut v_line = PathBuilder::stroke(px(1.0));
                v_line.move_to(gpui::point(
                    px(bounds.origin.x.as_f32() + chart_w),
                    bounds.origin.y,
                ));
                v_line.line_to(gpui::point(
                    px(bounds.origin.x.as_f32() + chart_w),
                    px(bounds.origin.y.as_f32() + full_h),
                ));
                if let Ok(p) = v_line.build() {
                    window.paint_path(p, border_color);
                }
            }

            // Horizontal line separating chart from X-axis
            let mut h_line = PathBuilder::stroke(px(1.0));
            let h_y = bounds.origin.y.as_f32() + full_h - AXIS_GAP;
            h_line.move_to(gpui::point(bounds.origin.x, px(h_y)));
            h_line.line_to(gpui::point(px(bounds.origin.x.as_f32() + chart_w), px(h_y)));
            if let Ok(p) = h_line.build() {
                window.paint_path(p, border_color);
            }
        }

        // 6. X-axis tick labels (timestamp-based)
        // ------------------------------------------------------------------ //
        {
            let origin = bounds.origin;
            let axis_y = origin.y.as_f32() + full_h - AXIS_GAP;

            let mut last_drawn_ts: Option<i64> = None;
            for (i, (&ts, _)) in timestamps.iter().zip(visible.iter()).enumerate() {
                let current_bucket = ((ts as f64) / best_interval).floor() as i64;
                let prev_bucket = if i > 0 {
                    ((timestamps[i - 1] as f64) / best_interval).floor() as i64
                } else {
                    current_bucket
                };

                // Always draw the first candle, or when crossing a bucket boundary
                if i != 0 && current_bucket <= prev_bucket {
                    continue;
                }
                let px_x = x_scale.scale(ts as f64);
                let abs_x = origin.x.as_f32() + px_x;
                let label: SharedString = self
                    .format_timestamp_short
                    .as_ref()
                    .map(|formatter| formatter(ts, best_interval, last_drawn_ts))
                    .unwrap_or_else(|| format_timestamp_short(ts, best_interval, last_drawn_ts).into());
                let lw = label.len() as f32 * CHAR_WIDTH_EST;
                let lx = abs_x - lw / 2.0;
                let lbounds = Bounds::from_corners(
                    gpui::point(px(lx), px(axis_y + 2.0)),
                    gpui::point(px(lx + lw + 4.0), px(axis_y + 14.0)),
                );
                paint_text_line(
                    label,
                    gpui::point(px(lx), px(axis_y + 2.0)),
                    cx.theme().muted_foreground,
                    lbounds,
                    window,
                    cx,
                );
                last_drawn_ts = Some(ts);
            }
        }

        // ------------------------------------------------------------------ //
        // 7. Y-axis labels (right side, dynamic width)
        // ------------------------------------------------------------------ //
        {
            let origin = bounds.origin;
            for &price in &y_ticks {
                let py = y_scale.scale(price);
                let label = self.price_str(price);
                let lx = origin.x.as_f32() + chart_w + Y_AXIS_PAD;
                let ly = origin.y.as_f32() + py - TIP_FONT / 2.0;
                let lbounds = Bounds::from_corners(
                    gpui::point(px(lx), px(ly)),
                    gpui::point(px(lx + y_axis_w - Y_AXIS_PAD), px(ly + TIP_FONT + 2.0)),
                );
                paint_text_line(
                    label,
                    gpui::point(px(lx), px(ly)),
                    cx.theme().muted_foreground,
                    lbounds,
                    window,
                    cx,
                );
            }
        }
        
        // Subpane Y-axis labels
        if y_axis_w > 0.0 {
            for (pane_abs_y, pane_h, p_min, p_max) in &subpane_bounds {
                let (s_min, s_max) = (*p_min, *p_max);
                let s_range = s_max - s_min;
                let mut s_ticks = Vec::new();
                if s_range > 0.0 {
                    let max_ticks = ((*pane_h as f64) / 40.0).max(2.0_f64);
                    let exponent = (s_range / max_ticks).log10().floor();
                    let fraction = (s_range / max_ticks) / 10_f64.powf(exponent);
                    let tick_spacing = if fraction < 1.5 { 1.0 } else if fraction < 3.0 { 2.0 } else if fraction < 7.0 { 5.0 } else { 10.0 } * 10_f64.powf(exponent);
                    let mut first_tick = (s_min / tick_spacing).ceil() * tick_spacing;
                    while first_tick <= s_max {
                        s_ticks.push(first_tick);
                        first_tick += tick_spacing;
                    }
                } else {
                    s_ticks.push(s_min);
                }
                let s_scale = LinearScale::new(s_min, s_max, *pane_h, 0.0);
                for &price in &s_ticks {
                    let py = s_scale.scale(price);
                    let label = self.price_str(price);
                    let lx = bounds.origin.x.as_f32() + chart_w + Y_AXIS_PAD;
                    let ly = *pane_abs_y + py - TIP_FONT / 2.0;
                    let lbounds = Bounds::from_corners(
                        gpui::point(px(lx), px(ly)),
                        gpui::point(px(lx + y_axis_w - Y_AXIS_PAD), px(ly + TIP_FONT + 2.0)),
                    );
                    paint_text_line(
                        label,
                        gpui::point(px(lx), px(ly)),
                        cx.theme().muted_foreground,
                        lbounds,
                        window,
                        cx,
                    );
                }
            }
        }


        // ------------------------------------------------------------------ //
        // 9. Last-price dashed line + Y-axis price label
        // ------------------------------------------------------------------ //
        let last_close_px_y = y_scale.scale(last_close_f);
        if self.show_candles && last_close_f > 0.0 {
            let lcp_abs_y = origin.y.as_f32() + last_close_px_y;
            let label_color = if last_candle_is_bullish {
                bullish_color
            } else {
                bearish_color
            };
            let mut hairline = label_color;
            hairline.a = 0.8;
            let seg = 6.0_f32;
            let gap = 4.0_f32;
            let mut x_cur = origin.x.as_f32();
            let end_x = origin.x.as_f32() + chart_w;
            while x_cur < end_x {
                let x_end = (x_cur + seg).min(end_x);
                let mut pb = PathBuilder::stroke(px(1.5));
                pb.move_to(gpui::point(px(x_cur), px(lcp_abs_y)));
                pb.line_to(gpui::point(px(x_end), px(lcp_abs_y)));
                if let Ok(p) = pb.build() {
                    window.paint_path(p, hairline);
                }
                x_cur += seg + gap;
            }

            // Pill label on the right Y-axis edge (inside the Y-axis area).
            let label_txt = self.price_str(last_close_f);

            // Countdown timer: time until next candle opens.
            let countdown_str = countdown_label_for_width.clone();

            let badge_fg = gpui::white();
            self.paint_price_pill_with_width(
                label_txt,
                label_color,
                badge_fg,
                origin.x.as_f32() + chart_w,
                lcp_abs_y,
                shared_live_badge_w,
                y_axis_w,
                window,
                cx,
            );

            // Paint countdown pill stacked directly below the price pill (no gap).
            self.paint_price_pill_with_width(
                countdown_str,
                label_color,
                badge_fg,
                origin.x.as_f32() + chart_w,
                lcp_abs_y + 14.0, // 14px pill height, no gap
                shared_live_badge_w,
                y_axis_w,
                window,
                cx,
            );
        }

        // ------------------------------------------------------------------ //
        // 10. Period high / low markers
        // ------------------------------------------------------------------ //
        if self.show_candles && period_high_f.is_finite() {
            self.paint_period_marker(
                period_high_f,
                period_high_px,
                true,
                origin.x.as_f32(),
                origin.y.as_f32(),
                chart_w,
                candle_h,
                window,
                cx,
            );
        }
        if self.show_candles && period_low_f.is_finite() {
            self.paint_period_marker(
                period_low_f,
                period_low_px,
                false,
                origin.x.as_f32(),
                origin.y.as_f32(),
                chart_w,
                candle_h,
                window,
                cx,
            );
        }

        // ------------------------------------------------------------------ //
        // 11. Active positions & open orders overlays
        // ------------------------------------------------------------------ //
        if self.show_candles {
            // A. Active Positions
            for pos in &self.position_overlays {
                let side_color = if pos.is_long { bullish_color } else { bearish_color };
                let pnl = if pos.is_long {
                    (pos.current_price - pos.entry_price) * pos.quantity
                } else {
                    (pos.entry_price - pos.current_price) * pos.quantity
                };
                let roi = if pos.margin > 0.0 {
                    (pnl / pos.margin) * 100.0
                } else {
                    0.0
                };
                let pnl_sign = if pnl >= 0.0 { "+" } else { "" };
                let pnl_str = format!("{}${:.2} ({}{:.1}%)", pnl_sign, pnl, pnl_sign, roi);

                let entry_px_y = y_scale.scale(pos.entry_price);
                let entry_abs_y = origin.y.as_f32() + entry_px_y;

                if entry_px_y >= 0.0 && entry_px_y <= candle_h {
                    // 1. Horizontal dashed entry line across chart
                    let mut line_color = side_color;
                    line_color.a = 0.85;
                    let seg = 6.0_f32;
                    let gap = 3.0_f32;
                    let mut x_cur = origin.x.as_f32();
                    let end_x = origin.x.as_f32() + chart_w;
                    while x_cur < end_x {
                        let x_end = (x_cur + seg).min(end_x);
                        let mut pb = PathBuilder::stroke(px(1.5));
                        pb.move_to(gpui::point(px(x_cur), px(entry_abs_y)));
                        pb.line_to(gpui::point(px(x_end), px(entry_abs_y)));
                        if let Ok(p) = pb.build() {
                            window.paint_path(p, line_color);
                        }
                        x_cur += seg + gap;
                    }

                    // 2. Position HUD Badge
                    let side_label = if pos.is_long { "LONG" } else { "SHORT" };
                    let qty_str = if pos.quantity >= 1.0 {
                        format!("{:.2}", pos.quantity)
                    } else {
                        format!("{:.4}", pos.quantity)
                    };
                    let badge_text = format!(
                        "{} {}x {} @ {} | {}",
                        side_label,
                        pos.leverage,
                        qty_str,
                        self.price_str(pos.entry_price),
                        pnl_str
                    );

                    let badge_w = ((badge_text.len() as f32) * 6.5 + 40.0).clamp(160.0, 340.0);
                    let badge_h = 22.0_f32;
                    let badge_x = (origin.x.as_f32() + chart_w - badge_w - 8.0).max(origin.x.as_f32() + 4.0);
                    let badge_y = (entry_abs_y - badge_h / 2.0)
                        .clamp(origin.y.as_f32() + 2.0, origin.y.as_f32() + candle_h - badge_h - 2.0);

                    // Badge background
                    let badge_bounds = Bounds::from_corners(
                        gpui::point(px(badge_x), px(badge_y)),
                        gpui::point(px(badge_x + badge_w), px(badge_y + badge_h)),
                    );
                    window.paint_quad(fill(badge_bounds, rgba(0x131722f0)));

                    // Left accent strip indicating side
                    let strip_bounds = Bounds::from_corners(
                        gpui::point(px(badge_x), px(badge_y)),
                        gpui::point(px(badge_x + 3.0), px(badge_y + badge_h)),
                    );
                    window.paint_quad(fill(strip_bounds, side_color));

                    // Subtle border
                    let mut b_builder = PathBuilder::stroke(px(1.0));
                    b_builder.move_to(gpui::point(px(badge_x), px(badge_y)));
                    b_builder.line_to(gpui::point(px(badge_x + badge_w), px(badge_y)));
                    b_builder.line_to(gpui::point(px(badge_x + badge_w), px(badge_y + badge_h)));
                    b_builder.line_to(gpui::point(px(badge_x), px(badge_y + badge_h)));
                    b_builder.line_to(gpui::point(px(badge_x), px(badge_y)));
                    if let Ok(bp) = b_builder.build() {
                        let mut border_c = side_color;
                        border_c.a = 0.4;
                        window.paint_path(bp, border_c);
                    }

                    // Badge text
                    let text_bounds = Bounds::from_corners(
                        gpui::point(px(badge_x + 6.0), px(badge_y)),
                        gpui::point(px(badge_x + badge_w - 24.0), px(badge_y + badge_h)),
                    );
                    let pnl_color = if pnl >= 0.0 { bullish_color } else { bearish_color };
                    paint_text_line(
                        badge_text.into(),
                        gpui::point(px(badge_x + 6.0), px(badge_y + 5.0)),
                        pnl_color,
                        text_bounds,
                        window,
                        cx,
                    );

                    // Close button [×]
                    let close_size = 16.0_f32;
                    let close_x = badge_x + badge_w - close_size - 4.0;
                    let close_y = badge_y + (badge_h - close_size) / 2.0;
                    let close_bounds = Bounds::from_corners(
                        gpui::point(px(close_x), px(close_y)),
                        gpui::point(px(close_x + close_size), px(close_y + close_size)),
                    );
                    window.paint_quad(fill(close_bounds, rgba(0xffffff22)));
                    paint_text_line_centered(
                        "×".into(),
                        gpui::white(),
                        close_bounds,
                        window,
                        cx,
                    );

                    // Register close hitbox
                    if let Some(ref st) = self.state {
                        st.overlay_hitboxes.borrow_mut().push(ChartOverlayHitbox {
                            bounds: close_bounds,
                            action: ChartOverlayAction::ClosePosition {
                                position_id: pos.id.clone(),
                            },
                        });
                    }

                    // 3. Price pill on Y-axis
                    self.paint_price_pill_with_width(
                        self.price_str(pos.entry_price),
                        side_color,
                        gpui::white(),
                        origin.x.as_f32() + chart_w,
                        entry_abs_y,
                        shared_live_badge_w,
                        y_axis_w,
                        window,
                        cx,
                    );
                } else {
                    // Off-screen indicator
                    let (off_y, arrow) = if entry_px_y < 0.0 {
                        (origin.y.as_f32() + 4.0, "▲")
                    } else {
                        (origin.y.as_f32() + candle_h - 20.0, "▼")
                    };
                    let off_text = format!(
                        "{} {} @ {} ({})",
                        arrow,
                        if pos.is_long { "LONG" } else { "SHORT" },
                        self.price_str(pos.entry_price),
                        pnl_str
                    );
                    let off_w = ((off_text.len() as f32) * 6.5 + 16.0).clamp(120.0, 240.0);
                    let off_x = (origin.x.as_f32() + chart_w - off_w - 8.0).max(origin.x.as_f32() + 4.0);
                    let off_bounds = Bounds::from_corners(
                        gpui::point(px(off_x), px(off_y)),
                        gpui::point(px(off_x + off_w), px(off_y + 18.0)),
                    );
                    window.paint_quad(fill(off_bounds, rgba(0x131722e0)));
                    let text_b = Bounds::from_corners(
                        gpui::point(px(off_x + 4.0), px(off_y)),
                        gpui::point(px(off_x + off_w - 4.0), px(off_y + 18.0)),
                    );
                    paint_text_line(
                        off_text.into(),
                        gpui::point(px(off_x + 4.0), px(off_y + 3.0)),
                        side_color,
                        text_b,
                        window,
                        cx,
                    );
                }

                // 4. Liquidation line (if leveraged)
                if let Some(liq_price) = pos.liquidation_price {
                    if liq_price > 0.0 && pos.leverage > 1 {
                        let liq_px_y = y_scale.scale(liq_price);
                        let liq_abs_y = origin.y.as_f32() + liq_px_y;
                        if liq_px_y >= 0.0 && liq_px_y <= candle_h {
                            let warning_color: Hsla = rgb(0xf59e0b).into();
                            let mut liq_line_color = warning_color;
                            liq_line_color.a = 0.75;
                            let seg = 4.0_f32;
                            let gap = 4.0_f32;
                            let mut x_cur = origin.x.as_f32();
                            let end_x = origin.x.as_f32() + chart_w;
                            while x_cur < end_x {
                                let x_end = (x_cur + seg).min(end_x);
                                let mut pb = PathBuilder::stroke(px(1.0));
                                pb.move_to(gpui::point(px(x_cur), px(liq_abs_y)));
                                pb.line_to(gpui::point(px(x_end), px(liq_abs_y)));
                                if let Ok(p) = pb.build() {
                                    window.paint_path(p, liq_line_color);
                                }
                                x_cur += seg + gap;
                            }

                            // Liquidation pill on Y-axis
                            let liq_str: SharedString = format!("LIQ {}", self.price_str(liq_price)).into();
                            self.paint_price_pill_with_width(
                                liq_str,
                                warning_color,
                                gpui::white(),
                                origin.x.as_f32() + chart_w,
                                liq_abs_y,
                                shared_live_badge_w,
                                y_axis_w,
                                window,
                                cx,
                            );
                        }
                    }
                }
            }

            // B. Open Orders
            for ord in &self.order_overlays {
                let side_color = if ord.is_long { bullish_color } else { bearish_color };
                let ord_px_y = y_scale.scale(ord.price);
                let ord_abs_y = origin.y.as_f32() + ord_px_y;

                if ord_px_y >= 0.0 && ord_px_y <= candle_h {
                    // 1. Horizontal dashed order line across chart
                    let mut line_color = side_color;
                    line_color.a = 0.75;
                    let seg = 4.0_f32;
                    let gap = 4.0_f32;
                    let mut x_cur = origin.x.as_f32();
                    let end_x = origin.x.as_f32() + chart_w;
                    while x_cur < end_x {
                        let x_end = (x_cur + seg).min(end_x);
                        let mut pb = PathBuilder::stroke(px(1.2));
                        pb.move_to(gpui::point(px(x_cur), px(ord_abs_y)));
                        pb.line_to(gpui::point(px(x_end), px(ord_abs_y)));
                        if let Ok(p) = pb.build() {
                            window.paint_path(p, line_color);
                        }
                        x_cur += seg + gap;
                    }

                    // 2. Order HUD Badge
                    let ord_side_str = if ord.is_long { "BUY" } else { "SELL" };
                    let qty_str = if ord.quantity >= 1.0 {
                        format!("{:.2}", ord.quantity)
                    } else {
                        format!("{:.4}", ord.quantity)
                    };
                    let badge_text = format!(
                        "{} {} {} @ {}",
                        ord.order_type,
                        ord_side_str,
                        qty_str,
                        self.price_str(ord.price)
                    );

                    let badge_w = ((badge_text.len() as f32) * 6.5 + 36.0).clamp(140.0, 300.0);
                    let badge_h = 20.0_f32;
                    let badge_x = (origin.x.as_f32() + chart_w - badge_w - 8.0).max(origin.x.as_f32() + 4.0);
                    let badge_y = (ord_abs_y - badge_h / 2.0)
                        .clamp(origin.y.as_f32() + 2.0, origin.y.as_f32() + candle_h - badge_h - 2.0);

                    // Badge background
                    let badge_bounds = Bounds::from_corners(
                        gpui::point(px(badge_x), px(badge_y)),
                        gpui::point(px(badge_x + badge_w), px(badge_y + badge_h)),
                    );
                    window.paint_quad(fill(badge_bounds, rgba(0x131722e8)));

                    // Left accent strip
                    let strip_bounds = Bounds::from_corners(
                        gpui::point(px(badge_x), px(badge_y)),
                        gpui::point(px(badge_x + 3.0), px(badge_y + badge_h)),
                    );
                    window.paint_quad(fill(strip_bounds, side_color));

                    // Border
                    let mut b_builder = PathBuilder::stroke(px(1.0));
                    b_builder.move_to(gpui::point(px(badge_x), px(badge_y)));
                    b_builder.line_to(gpui::point(px(badge_x + badge_w), px(badge_y)));
                    b_builder.line_to(gpui::point(px(badge_x + badge_w), px(badge_y + badge_h)));
                    b_builder.line_to(gpui::point(px(badge_x), px(badge_y + badge_h)));
                    b_builder.line_to(gpui::point(px(badge_x), px(badge_y)));
                    if let Ok(bp) = b_builder.build() {
                        let mut border_c = side_color;
                        border_c.a = 0.35;
                        window.paint_path(bp, border_c);
                    }

                    // Text
                    let text_bounds = Bounds::from_corners(
                        gpui::point(px(badge_x + 6.0), px(badge_y)),
                        gpui::point(px(badge_x + badge_w - 22.0), px(badge_y + badge_h)),
                    );
                    paint_text_line(
                        badge_text.into(),
                        gpui::point(px(badge_x + 6.0), px(badge_y + 4.0)),
                        gpui::white(),
                        text_bounds,
                        window,
                        cx,
                    );

                    // Cancel button [×]
                    let cancel_size = 14.0_f32;
                    let cancel_x = badge_x + badge_w - cancel_size - 4.0;
                    let cancel_y = badge_y + (badge_h - cancel_size) / 2.0;
                    let cancel_bounds = Bounds::from_corners(
                        gpui::point(px(cancel_x), px(cancel_y)),
                        gpui::point(px(cancel_x + cancel_size), px(cancel_y + cancel_size)),
                    );
                    window.paint_quad(fill(cancel_bounds, rgba(0xffffff22)));
                    paint_text_line_centered(
                        "×".into(),
                        gpui::white(),
                        cancel_bounds,
                        window,
                        cx,
                    );

                    // Register cancel hitbox
                    if let Some(ref st) = self.state {
                        st.overlay_hitboxes.borrow_mut().push(ChartOverlayHitbox {
                            bounds: cancel_bounds,
                            action: ChartOverlayAction::CancelOrder {
                                order_id: ord.id.clone(),
                            },
                        });
                    }

                    // 3. Price pill on Y-axis
                    self.paint_price_pill_with_width(
                        self.price_str(ord.price),
                        side_color,
                        gpui::white(),
                        origin.x.as_f32() + chart_w,
                        ord_abs_y,
                        shared_live_badge_w,
                        y_axis_w,
                        window,
                        cx,
                    );
                } else {
                    // Off-screen indicator
                    let (off_y, arrow) = if ord_px_y < 0.0 {
                        (origin.y.as_f32() + 4.0, "▲")
                    } else {
                        (origin.y.as_f32() + candle_h - 20.0, "▼")
                    };
                    let ord_side_str = if ord.is_long { "BUY" } else { "SELL" };
                    let off_text = format!(
                        "{} {} {} @ {}",
                        arrow,
                        ord.order_type,
                        ord_side_str,
                        self.price_str(ord.price)
                    );
                    let off_w = ((off_text.len() as f32) * 6.5 + 16.0).clamp(110.0, 220.0);
                    let off_x = (origin.x.as_f32() + chart_w - off_w - 8.0).max(origin.x.as_f32() + 4.0);
                    let off_bounds = Bounds::from_corners(
                        gpui::point(px(off_x), px(off_y)),
                        gpui::point(px(off_x + off_w), px(off_y + 16.0)),
                    );
                    window.paint_quad(fill(off_bounds, rgba(0x131722d8)));
                    let text_b = Bounds::from_corners(
                        gpui::point(px(off_x + 4.0), px(off_y)),
                        gpui::point(px(off_x + off_w - 4.0), px(off_y + 16.0)),
                    );
                    paint_text_line(
                        off_text.into(),
                        gpui::point(px(off_x + 4.0), px(off_y + 2.0)),
                        side_color,
                        text_b,
                        window,
                        cx,
                    );
                }
            }
        }

        // ------------------------------------------------------------------ //
        // 12. Crosshair + inline OHLCV tooltip + X-axis timestamp pill
        // ------------------------------------------------------------------ //
        if let Some(ref st) = self.state {
            if let Some(cursor) = st.crosshair_pos {
                // cursor is window-absolute; convert to element-relative by subtracting bounds.origin
                let cursor_rel = cursor - origin;
                let cursor_x_in_chart = cursor_rel.x.as_f32();
                if cursor_x_in_chart >= 0.0 && cursor_x_in_chart <= chart_w {
                    // Find the nearest candle using the X scale for accurate alignment.
                    let cursor_ts = x_scale.inverse(cursor_x_in_chart);
                    let idx_opt = timestamps
                        .iter()
                        .enumerate()
                        .min_by_key(|&(_, ts)| (*ts as f64 - cursor_ts).abs() as i64);

                    if let Some((idx, _)) = idx_opt {
                        if idx < visible.len() {
                            let abs_idx = st.visible_range(self.data.len()).start + idx;
                            st.crosshair_index.set(Some(abs_idx));
                            
                            let d = visible[idx];
                            let ts = timestamps[idx];
                            let open_v = open_fn(d).to_f64().unwrap_or(0.0);
                            let _high_v = high_fn(d).to_f64().unwrap_or(0.0);
                            let _low_v = low_fn(d).to_f64().unwrap_or(0.0);
                            let close_v = close_fn(d).to_f64().unwrap_or(0.0);
                            let _vol_v = self.volume.as_ref().map(|f| f(d)).unwrap_or(0.0);
                            let is_bull = close_v >= open_v;
                            let _candle_col: Hsla = if is_bull {
                                bullish_color
                            } else {
                                bearish_color
                            };

                            // Snap X to the candle centre.
                            let cx_px =
                                centres_x.get(idx).copied().unwrap_or(cursor_rel.x.as_f32());

                            // Klinechart-style crosshair (dashed, neutral color)
                            let mut crosshair_color = crosshair_line;
                            // opacity is handled by the color itself in coinwatch, but we can set 0.8
                            crosshair_color.a = 0.8;
                            let dash_len = 6.0;
                            let gap_len = 4.0;

                            // Vertical dashed hairline
                            let vx = origin.x.as_f32() + cx_px;
                            let mut y_curr = origin.y.as_f32();
                            // Vertical line spans full chart (candles + volume), stops at the bottom axis gap.
                            let y_end = origin.y.as_f32() + full_h - AXIS_GAP;
                            let mut vl = PathBuilder::stroke(px(1.5));
                            while y_curr < y_end {
                                vl.move_to(gpui::point(px(vx), px(y_curr)));
                                let next_y = (y_curr + dash_len).min(y_end);
                                vl.line_to(gpui::point(px(vx), px(next_y)));
                                y_curr = next_y + gap_len;
                            }
                            if let Ok(p) = vl.build() {
                                window.paint_path(p, crosshair_color);
                            }

                            // Horizontal dashed hairline
                            let cy_rel = cursor_rel.y.as_f32();
                            if cy_rel >= 0.0 && cy_rel <= full_h - AXIS_GAP {
                                let hy = origin.y.as_f32() + cy_rel;
                                let mut x_curr = origin.x.as_f32();
                                let x_end = origin.x.as_f32() + chart_w;
                                let mut hl = PathBuilder::stroke(px(1.5));
                                while x_curr < x_end {
                                    hl.move_to(gpui::point(px(x_curr), px(hy)));
                                    let next_x = (x_curr + dash_len).min(x_end);
                                    hl.line_to(gpui::point(px(next_x), px(hy)));
                                    x_curr = next_x + gap_len;
                                }
                                if let Ok(p) = hl.build() {
                                    window.paint_path(p, crosshair_color);
                                }

                                // Crosshair pill on Y-axis
                                let cy_abs = origin.y.as_f32() + cy_rel;
                                let mut pl_txt: gpui::SharedString = "".into();
                                if cy_rel <= candle_h {
                                    let price_at_cursor = y_scale.inverse(cy_rel);
                                    pl_txt = self.price_str(price_at_cursor);
                                } else {
                                    // Find which subpane we are in
                                    for (y0, pane_h, p_min, p_max) in &subpane_bounds {
                                        let y0 = *y0;
                                        if cy_abs >= y0 && cy_abs <= y0 + pane_h {
                                            let rel = (cy_abs - y0) / pane_h;
                                            let v_val = p_max - (rel as f64) * (p_max - p_min);
                                            pl_txt = format!("{:.2}", v_val).into();
                                            break;
                                        }
                                    }
                                    
                                    // If not found in indicator bounds, maybe it's explicit volume
                                    if pl_txt.is_empty() && self.volume.is_some() {
                                        let v_y0 = origin.y.as_f32() + candle_h + VOLUME_GAP;
                                        let pane_h = avail * subpane_ratios[0];
                if cy_abs >= v_y0 && cy_abs <= v_y0 + pane_h {
                                            let vols: Vec<f64> = visible.iter().map(|d| self.volume.as_ref().unwrap()(d)).collect();
                                            let max_vol = vols.iter().cloned().fold(0.0_f64, f64::max);
                                            let vol_cy = cy_abs - v_y0;
                                            let v_val = max_vol - (vol_cy / pane_h) as f64 * max_vol;
                                            pl_txt = format_volume(v_val).into();
                                        }
                                    }
                                }
                                
                                if !pl_txt.is_empty() {
                                    self.paint_price_pill_with_width(
                                        pl_txt,
                                        crosshair_bg,
                                        gpui::white(),
                                        origin.x.as_f32() + chart_w,
                                        hy,
                                        shared_live_badge_w,
                                        y_axis_w,
                                        window,
                                        cx,
                                    );
                                }
                            }

                            // --- X-axis timestamp pill ---
                            {
                                let ts_label: SharedString = self
                                    .format_timestamp_short
                                    .as_ref()
                                    .map(|formatter| formatter(ts, best_interval, None))
                                    .unwrap_or_else(|| {
                                        format_timestamp_short(ts, best_interval, None).into()
                                    });
                                let pill_w =
                                    (ts_label.len() as f32 * CHAR_WIDTH_EST + 10.0).max(50.0);
                                let pill_h = 14.0_f32;
                                let axis_y = origin.y.as_f32() + full_h - AXIS_GAP;
                                let pill_x = (vx - pill_w / 2.0)
                                    .max(origin.x.as_f32())
                                    .min(origin.x.as_f32() + chart_w - pill_w);
                                let pill_bounds = Bounds::from_corners(
                                    gpui::point(px(pill_x), px(axis_y + 1.0)),
                                    gpui::point(px(pill_x + pill_w), px(axis_y + pill_h + 1.0)),
                                );
                                window.paint_quad(fill(pill_bounds, cx.theme().foreground));
                                paint_text_line(
                                    ts_label,
                                    gpui::point(px(pill_x + 3.0), px(axis_y + 2.0)),
                                    cx.theme().background,
                                    pill_bounds,
                                    window,
                                    cx,
                                );
                            }

                            // --- Inline OHLCV tooltip (top of chart, horizontal row) ---
                            {

                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers on CandlestickChart
// ---------------------------------------------------------------------------

impl<T, Y> CandlestickChart<T, Y>
where
    Y: Copy + PartialOrd + Num + ToPrimitive + Sealed + 'static,
{
    fn price_str(&self, v: f64) -> SharedString {
        if let Some(ref f) = self.format_price {
            f(v)
        } else {
            format_price_inline(v).into()
        }
    }

    fn pill_width_for_text(&self, text: &SharedString, y_axis_w: f32, window: &mut Window) -> f32 {
        let font_size = px(TIP_FONT);
        let mut font = window.text_style().font();
        font.weight = gpui::FontWeight::BOLD;
        let text_run = gpui::TextRun {
            len: text.len(),
            font,
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let measured_w = window
            .text_system()
            .shape_text(text.clone(), font_size, &[text_run], None, None)
            .ok()
            .and_then(|lines| lines.into_iter().next())
            .map(|line| line.size(font_size).width.as_f32())
            .unwrap_or(text.len() as f32 * CHAR_WIDTH_EST);

        (measured_w + Y_AXIS_PAD * 2.0)
            .max(40.0)
            .min(y_axis_w - 2.0)
    }

    fn paint_price_pill_with_width(
        &self,
        text: SharedString,
        bg: Hsla,
        fg: Hsla,
        left_edge_x: f32,
        centre_y: f32,
        _pill_w: f32,
        y_axis_w: f32,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pill_h = 14.0_f32;
        // Fill from side to side of the axis
        let pill_x = left_edge_x;
        let pill_y = centre_y - pill_h / 2.0;
        let pill_bounds = Bounds::from_corners(
            gpui::point(px(pill_x), px(pill_y)),
            gpui::point(px(pill_x + y_axis_w), px(pill_y + pill_h)),
        );
        let quad = gpui::fill(pill_bounds, bg);
        // Coinwatch natively has border: solid 1px transparent or matching bg
        window.paint_quad(quad);
        paint_text_line_centered(text, fg, pill_bounds, window, cx);
    }

    /// Paint a period high or low marker (tick + connector + price label).
    #[allow(clippy::too_many_arguments)]
    fn paint_period_marker(
        &self,
        price: f64,
        px_pos: (f32, f32), // (centre_x, y) relative to chart area origin
        is_high: bool,
        origin_x: f32,
        origin_y: f32,
        chart_w: f32,
        candle_h: f32,
        window: &mut Window,
        cx: &mut App,
    ) {
        let gold: Hsla = rgb(0xf0c060).into();
        let (cx_rel, cy_rel) = px_pos;
        let abs_x = origin_x + cx_rel;
        let abs_y = origin_y + cy_rel;

        // Small horizontal tick at the extreme.
        let mut tick = PathBuilder::stroke(px(1.));
        tick.move_to(gpui::point(px(abs_x - 4.), px(abs_y)));
        tick.line_to(gpui::point(px(abs_x + 4.), px(abs_y)));
        if let Ok(p) = tick.build() {
            window.paint_path(p, gold);
        }

        // Adaptive label direction: left half → label to the right, right half → label to the left.
        let label = self.price_str(price);
        let lw = (label.len() as f32 * CHAR_WIDTH_EST + 8.0).max(50.0);
        let lh = 12.0_f32;
        let label_offset = 13.0_f32;

        let (text_y, connector_end_y) = if is_high {
            let ty = (abs_y - label_offset).max(origin_y + 2.);
            (ty, ty)
        } else {
            let ty = (abs_y + label_offset).min(origin_y + candle_h - 2.);
            (ty, ty)
        };

        // Vertical connector.
        let mut conn = PathBuilder::stroke(px(1.));
        conn.move_to(gpui::point(px(abs_x), px(abs_y)));
        conn.line_to(gpui::point(px(abs_x), px(connector_end_y)));
        if let Ok(p) = conn.build() {
            window.paint_path(p, gold);
        }

        // Label position: right of candle if in left half, left of candle otherwise.
        let lx = if cx_rel < chart_w / 2.0 {
            abs_x + 6.0
        } else {
            (abs_x - lw - 6.0).max(origin_x)
        };
        let ly = if is_high {
            text_y - lh - 1.
        } else {
            text_y + 1.
        };
        let label_bounds = Bounds::from_corners(
            gpui::point(px(lx), px(ly)),
            gpui::point(px(lx + lw), px(ly + lh)),
        );
        paint_text_line(
            label,
            gpui::point(px(lx), px(ly)),
            gold,
            label_bounds,
            window,
            cx,
        );
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Draw a single line of text using the window text system.
fn paint_text_line(
    text: SharedString,
    origin: gpui::Point<Pixels>,
    color: Hsla,
    _clip_bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui::TextRun;
    let font_size = px(TIP_FONT);
    let text_run = TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    if let Ok(shaped) = window
        .text_system()
        .shape_text(text, font_size, &[text_run], None, None)
    {
        for line in shaped {
            let _ = line.paint(origin, font_size, TextAlign::Left, None, window, cx);
        }
    }
}

pub fn paint_text_line_centered(
    text: SharedString,
    color: Hsla,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let font_size = px(TIP_FONT);
    let mut font = window.text_style().font();
    font.weight = gpui::FontWeight::BOLD;
    let text_run = gpui::TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };

    if let Ok(shaped) = window
        .text_system()
        .shape_text(text, font_size, &[text_run], None, None)
    {
        let bottom_right = bounds.corner(Corner::BottomRight);
        let center_x = (bounds.origin.x + bottom_right.x).as_f32() / 2.0;
        let center_y = (bounds.origin.y + bottom_right.y).as_f32() / 2.0;

        for line in shaped {
            let line_size = line.size(font_size);
            let origin = gpui::point(
                px(center_x - line_size.width.as_f32() / 2.0),
                px(center_y - font_size.as_f32() / 2.0),
            );
            let _ = line.paint(origin, font_size, TextAlign::Left, None, window, cx);
        }
    }
}

/// Default price formatting (uses magnitude-based rules).
/// For exchange-specific precision, set a custom formatter via `.format_price()`.
fn format_price_inline(v: f64) -> String {
    // Magnitude-based formatting (fallback)
    let abs_v = v.abs();
    if abs_v >= 10_000.0 {
        format!("{:.2}", v)
    } else if abs_v >= 1_000.0 {
        format!("{:.2}", v)
    } else if abs_v >= 1.0 {
        format!("{:.4}", v)
    } else if abs_v >= 0.01 {
        format!("{:.6}", v)
    } else {
        format!("{:.8}", v)
    }
}

/// Compact volume string (e.g. 1_234_567 → "1.23M").
fn format_volume(v: f64) -> String {
    if v >= 1_000_000_000.0 {
        format!("{:.2}B", v / 1_000_000_000.0)
    } else if v >= 1_000_000.0 {
        format!("{:.2}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{:.0}", v)
    }
}

/// Human-readable countdown duration separated by colons.
/// Always includes all units from the highest active unit down to seconds.
/// Examples: 9m:12s, 3h:14m:05s, 1w:0d:0h:4m:05s.
fn format_countdown_duration(remaining_secs: i64) -> String {
    if remaining_secs <= 0 {
        return "0s".to_string();
    }

    let mut secs = remaining_secs;
    let weeks = secs / 604_800;
    secs %= 604_800;
    let days = secs / 86_400;
    secs %= 86_400;
    let hours = secs / 3_600;
    secs %= 3_600;
    let minutes = secs / 60;
    secs %= 60;
    let seconds = secs;

    if weeks > 0 {
        return format!(
            "{}w:{}d:{}h:{:02}m:{:02}s",
            weeks, days, hours, minutes, seconds
        );
    }
    if days > 0 {
        return format!("{}d:{}h:{:02}m:{:02}s", days, hours, minutes, seconds);
    }
    if hours > 0 {
        return format!("{}h:{:02}m:{:02}s", hours, minutes, seconds);
    }
    if minutes > 0 {
        return format!("{}m:{:02}s", minutes, seconds);
    }
    format!("{}s", seconds)
}

/// Short timestamp: HH:MM (suitable for intraday views).
fn format_timestamp_short(ts_ms: i64, _interval: f64, _prev: Option<i64>) -> String {
    let ts_secs = ts_ms / 1_000;
    let h = (ts_secs / 3600) % 24;
    let m = (ts_secs / 60) % 60;
    format!("{:02}:{:02}", h, m)
}

/// Long timestamp: MMM DD HH:MM (suitable for tooltip row).
#[allow(dead_code)]
fn format_timestamp_long(ts_ms: i64) -> String {
    let ts_secs = ts_ms / 1_000;
    let h = (ts_secs / 3600) % 24;
    let m = (ts_secs / 60) % 60;
    // Days since epoch (approximate).
    let day_of_year = (ts_secs / 86400) % 365;
    // Rough month/day (good enough for display, avoids pulling in chrono).
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month_days = [31i64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut rem = day_of_year;
    let mut month_idx = 0usize;
    for (i, &days) in month_days.iter().enumerate() {
        if rem < days {
            month_idx = i;
            break;
        }
        rem -= days;
    }
    format!("{} {:02} {:02}:{:02}", months[month_idx], rem + 1, h, m)
}
