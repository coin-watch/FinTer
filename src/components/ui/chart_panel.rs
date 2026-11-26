/// Chart panel — GPU-accelerated interactive candlestick chart.
///
/// Renders OHLCV candlestick data from a `CandleFeed` using gpui-component's
/// `CandlestickChart`. Supports:
///
/// - **Crosshair + OHLCV readout** – cursor position is tracked and passed to
///   the chart renderer, which draws hairlines and an axis price badge.
/// - **Horizontal scroll / pan** – click-drag moves the viewport through the
///   candle history; the drag is delta-based so the candle under the cursor
///   stays fixed.
/// - **Mouse-wheel zoom** – vertical scroll changes `visible_count` (zoom
///   level) centred on the current viewport.
/// - **Last-price line + label** – dashed horizontal line at the most recent
///   close price with a coloured pill label on the right Y-axis edge.
/// - **Period high / low markers** – gold tick + label at the highest high and
///   lowest low in the current visible window.
/// - **Volume sub-pane** – transparent bar chart below the candles.
/// - **Technical Indicators** – parameterized indicator instances with cached
///   computation keyed to candle feed version.
/// - **Drawing Tools** – full suite of charting tools (Trendline, Ray, Horizontal,
///   Vertical, Fibonacci, Channel, Long/Short Position, Text) with interactive
///   placement, selection, moving, deletion, and persistence.
/// - **Price Alert Levels** – saved alerts for the pane's exchange/symbol render
///   as dashed horizontal lines with Y-axis pills and off-screen edge badges.
/// - **Title-bar controls** – chart type, indicators, drawings, and view
///   actions live in the pane title bar; indicator management and drawing
///   colour open in floating windows rather than inside the chart panel.
///
/// # 60 FPS design
/// `render()` calls `window.request_animation_frame()` every frame. State
/// mutations (mouse events) mark the view dirty via `cx.notify()`, which
/// triggers the next frame immediately.
use gpui::{
    AnyElement, App, AppContext, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, PathBuilder, Pixels, Point, Render, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::PixelsExt;
use gpui_component::button::Button;
use gpui_component::chart::core::{
    AXIS_GAP, AxisHandle, Ohlcv, PriceScaleMode, drag_price_range, drag_time_span,
    timeframe_interval_ms, timestamp_at_buffer_index,
};
use gpui_component::chart::elements::{
    ChartEvent, ChartOptions, ChartSnapshot, ChartView, PriceAxisState,
};
use gpui_component::chart::theme::ChartPalette;
use gpui_component::chart::{
    AlertOverlaySpec, CandleType, CandlestickChart, ChartOverlayAction, ChartState, IndicatorStyle,
    MAX_VISIBLE, MIN_VISIBLE, OrderOverlaySpec, PaneTarget, PositionOverlaySpec,
};
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::theme::ActiveTheme;
use gpui_component::v_flex;

use crate::chart::drawings::{
    DrawingInstance, DrawingInteractionState, DrawingPoint, DrawingTool, hit_test_drawings,
};
use crate::chart::indicators::{IndicatorCache, validate_params};
use crate::chart::layout::ChartMetrics;
use crate::chart::model::{ChartLayout, IndicatorCategory, IndicatorInstance, IndicatorKind};
use crate::chart::persistence::{default_chart_dir, load_layout, save_layout};
use crate::data::alerts::AlertStore;
use crate::data::candle_feed::CandleFeed;
use crate::data::models::{OhlcvCandle, PrecisionInfo};
use crate::data::price_format::format_price;
use crate::klinechart;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Display constants
// ---------------------------------------------------------------------------

/// Zoom factor applied per mouse-wheel tick.
const ZOOM_FACTOR_IN: f32 = 0.85;
const ZOOM_FACTOR_OUT: f32 = 1.0 / ZOOM_FACTOR_IN;

// ---------------------------------------------------------------------------
// ChartPanel
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum LegendSettingsTarget {
    Main,
    Indicator(usize), // index in indicators list
    Volume,
}

pub type TechnicalIndicatorKind = IndicatorKind;
pub type TechnicalIndicatorCategory = IndicatorCategory;

/// Callback used by title-bar and legend controls to open the floating
/// indicator window without owning window creation.
pub type ChartWindowCallback = std::sync::Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>;

struct IndicatorLineSpec {
    instance_id: String,
    #[allow(dead_code)]
    kind: IndicatorKind,
    name: String,
    series: Vec<(i64, f64)>,
    color: gpui::Hsla,
    style: IndicatorStyle,
    pane: PaneTarget,
}

/// In-progress axis drag for the legacy `CandlestickChart` renderer.
///
/// Mirrors v2's `AxisDrag` but stores the values the legacy `ChartState`
/// needs (visible count / viewport offset) instead of `Viewport` slots.
#[derive(Clone, Copy, Debug, PartialEq)]
enum LegacyAxisDrag {
    Price {
        start_y: f32,
        start_range: (f64, f64),
        anchor_price: f64,
        moved: bool,
    },
    Time {
        start_x: f32,
        start_visible: f64,
        anchor_pos: f64,
        anchor_frac: f32,
        moved: bool,
    },
}

/// Hit-test the legacy chart's axis rails in window-space pixels.
///
/// `origin` is the candle plot origin and `plot_size` is `(chart_w, candle_h)`
/// captured during paint. The price rail starts 4 px left of the plot's right
/// edge; the time rail is the 4 px above the bottom axis strip. Pure so it can
/// be unit-tested without a window.
fn axis_at_point(
    origin: (f32, f32),
    plot_size: (f32, f32),
    bounds_bottom: f32,
    x: f32,
    y: f32,
) -> Option<AxisHandle> {
    let (origin_x, origin_y) = origin;
    let (chart_w, candle_h) = plot_size;
    if !origin_x.is_finite()
        || !origin_y.is_finite()
        || !chart_w.is_finite()
        || !candle_h.is_finite()
        || !bounds_bottom.is_finite()
        || !x.is_finite()
        || !y.is_finite()
        || chart_w <= 0.0
        || candle_h <= 0.0
    {
        return None;
    }

    if x >= origin_x + chart_w - 4.0 && y >= origin_y && y <= origin_y + candle_h {
        return Some(AxisHandle::Price);
    }
    if y >= bounds_bottom - AXIS_GAP - 4.0 && x >= origin_x - 4.0 && x <= origin_x + chart_w + 4.0 {
        return Some(AxisHandle::Time);
    }
    None
}

pub struct ChartPanel {
    feed: CandleFeed,
    /// Display title (e.g. "BTC/USDT 1m").
    title: String,
    /// Timeframe for display (e.g. "1m", "5m", "1h").
    timeframe: String,
    /// Exchange name for persistence context (e.g. "binance").
    exchange: String,
    /// Symbol name for persistence context (e.g. "BTC/USDT").
    symbol: String,
    /// Exchange-specific price precision metadata
    precision: Option<PrecisionInfo>,
    focus_handle: FocusHandle,
    /// Interactive viewport + crosshair state.
    chart_state: ChartState,
    /// Axis rail under the pointer, used for cursor + press handling.
    hover_axis: Option<AxisHandle>,
    /// Active price/time axis drag, if any.
    axis_drag: Option<LegacyAxisDrag>,
    /// Experimental scene-based chart (enabled with `FINTER_CHART_V2=1`).
    chart_v2: gpui::Entity<ChartView>,
    /// Cheap change key for the v2 chart snapshot/options.
    last_v2_key: String,
    /// Persisted v2 price scale mode and range.
    v2_price_scale_mode: String,
    manual_price_range: Option<(f64, f64)>,
    v2_percent_anchor: Option<f64>,
    pub on_load_more:
        Option<std::sync::Arc<dyn Fn(i64, std::sync::mpsc::Sender<()>) + Send + Sync>>,
    /// Called shortly after a new live candle appears so the feed can re-fetch
    /// the just-closed bar from the exchange (final OHLCV + volume).
    pub on_refresh_latest: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    /// Opens the floating indicator window. Set by the pane-grid renderer so
    /// the chart's `I` shortcut and legend controls do not own window creation.
    pub on_open_indicators: Option<ChartWindowCallback>,
    /// Timestamp of the newest candle seen on the previous frame, used to
    /// detect a period rollover.
    last_completed_candle_ts: i64,
    history_completion: Option<std::sync::mpsc::Receiver<()>>,
    history_retry_at: Option<std::time::Instant>,
    oldest_loaded_ts: i64,
    is_loading_more: bool,
    load_more_attempts: u8,
    show_main_series: bool,

    // Parameterized Indicators
    pub indicators: Vec<IndicatorInstance>,
    indicator_cache: IndicatorCache,
    indicator_hovered: Vec<usize>,

    // Legends & Quick Settings
    main_legend_hovered: bool,
    vol_legend_hovered: bool,
    active_legend_settings: Option<LegendSettingsTarget>,
    settings_panel_x: f32,
    settings_panel_y: f32,
    settings_panel_dragging: bool,
    settings_drag_start_x: f32,
    settings_drag_start_y: f32,
    settings_drag_origin_x: f32,
    settings_drag_origin_y: f32,

    // Drawing Tools
    pub drawings: Vec<DrawingInstance>,
    pub active_drawing_tool: Option<DrawingTool>,
    pub drawing_state: DrawingInteractionState,
    pub selected_drawing_id: Option<String>,
    pub drawing_color: String,
    pub snap_to_candle: bool,
    drawing_cursor_point: Option<DrawingPoint>,

    // Persistence & Debounce
    layout_dirty: bool,
    last_dirty_time: Instant,

    /// Pixel width of the chart area on the most recent frame (for pan math).
    last_chart_width: f32,
    /// Last known cursor X position (chart-relative pixels), for zoom anchoring.
    last_cursor_x: f32,
    /// Current chart rendering style
    candle_type: CandleType,
    /// Show/hide horizontal dashed guide lines in plot area.
    show_horizontal_grid: bool,
    /// IANA timezone used for chart labels and crosshair timestamps.
    timezone: String,
    pub pane_width: f32,
    pub pane_height: f32,
}

fn chart_v2_enabled() -> bool {
    std::env::var_os("FINTER_CHART_V2").is_some()
}

/// Duplicate indicators are allowed, so ids must never collide with an
/// existing instance even after one of several duplicates is removed.
fn unique_indicator_id(indicators: &[IndicatorInstance], kind: &IndicatorKind) -> String {
    let base = kind.label().to_lowercase();
    let mut suffix = indicators.len() + 1;
    let mut id = format!("{base}-{suffix}");
    while indicators.iter().any(|instance| instance.id == id) {
        suffix += 1;
        id = format!("{base}-{suffix}");
    }
    id
}

fn chart_v2_snapshot(feed: &CandleFeed) -> (ChartSnapshot, u64) {
    let (candles, version) = feed.snapshot_with_version();
    let mut bars = Vec::with_capacity(candles.len());
    let mut timestamps = Vec::with_capacity(candles.len());
    for candle in &candles {
        bars.push(Ohlcv::new(
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume,
        ));
        timestamps.push(candle.timestamp);
    }
    (
        ChartSnapshot::new(Arc::from(bars), Arc::from(timestamps), version),
        version,
    )
}

fn chart_v2_options(
    precision: &Option<PrecisionInfo>,
    timezone: &str,
    show_grid: bool,
) -> ChartOptions {
    let precision = precision.clone();
    let timezone = timezone.to_string();
    ChartOptions {
        show_grid,
        price_formatter: Some(Arc::new(move |value| {
            format_price(value, precision.as_ref()).into()
        })),
        time_formatter: Some(Arc::new(move |timestamp| {
            let utc = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp / 1_000, 0)
                .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
            let tz = timezone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC);
            utc.with_timezone(&tz)
                .format("%b %d %H:%M")
                .to_string()
                .into()
        })),
        ..Default::default()
    }
}

fn chart_v2_scale_mode(
    name: &str,
    manual: Option<(f64, f64)>,
    percent_anchor: Option<f64>,
) -> PriceScaleMode {
    match name {
        "log" => PriceScaleMode::Log,
        "percent" => {
            let anchor = percent_anchor
                .filter(|anchor| anchor.is_finite() && *anchor != 0.0)
                .or_else(|| {
                    manual
                        .filter(|(min, max)| min.is_finite() && max.is_finite() && max > min)
                        .map(|(min, max)| (min + max) * 0.5)
                })
                .unwrap_or(1.0);
            PriceScaleMode::percent(anchor)
        }
        "inverted" => PriceScaleMode::InvertedLinear,
        _ => PriceScaleMode::Linear,
    }
}

impl ChartPanel {
    fn sync_chart_v2(&mut self, cx: &mut Context<Self>) {
        let (snapshot, version) = chart_v2_snapshot(&self.feed);
        let mut options =
            chart_v2_options(&self.precision, &self.timezone, self.show_horizontal_grid);
        options.title = format!("{} · {}", self.symbol, self.timeframe).into();
        let palette = ChartPalette::from_theme(&cx.theme().colors);
        let mode = chart_v2_scale_mode(
            &self.v2_price_scale_mode,
            self.manual_price_range,
            self.v2_percent_anchor,
        );
        let axis_state = match self.manual_price_range {
            Some((min, max)) if min.is_finite() && max.is_finite() && max > min => {
                PriceAxisState::Manual { min, max }
            }
            _ => PriceAxisState::Auto,
        };
        self.chart_v2.update(cx, |view, cx| {
            view.set_snapshot(snapshot, cx);
            view.set_options(options, cx);
            view.set_palette_if_changed(palette, cx);
            if view.price_scale_mode() != mode {
                view.set_price_scale_mode(mode, cx);
            }
            if view.price_axis_state() != axis_state {
                view.set_price_axis_state(axis_state, cx);
            }
        });
        self.last_v2_key = format!(
            "{:?}|{}|{}|{}|{}|{:?}|{:?}",
            self.precision,
            self.timezone,
            self.show_horizontal_grid,
            version,
            self.v2_price_scale_mode,
            self.manual_price_range,
            self.v2_percent_anchor
        );
    }

    pub fn agent_snapshot(&self) -> serde_json::Value {
        let candles = self.feed.snapshot();
        serde_json::json!({
            "exchange": self.exchange, "symbol": self.symbol, "timeframe": self.timeframe,
            "candles": &candles[candles.len().saturating_sub(120)..],
            "last_update_ms": self.feed.last_update_ms(), "loading": candles.is_empty()
        })
    }

    pub fn new(
        feed: CandleFeed,
        title: impl Into<String>,
        timeframe: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let f = feed.clone();
        // The countdown is painted from wall-clock time, independent of feed
        // updates. Invalidate the retained panel once per second even on quiet
        // markets; keep coalescing feed updates at the existing 250 ms cadence.
        cx.spawn(async move |this, cx| {
            let mut last_ms = 0;
            let mut last_countdown_second = u64::MAX;
            let mut last_trade_ver = u64::MAX;
            let mut last_alert_ver = u64::MAX;
            let mut last_credit_rev = u64::MAX;
            loop {
                let ms = f.last_update_ms();
                // Use the absolute Unix second as the invalidation key. The
                // chart renderer derives the remaining time from the same wall
                // clock, so every displayed second changes exactly once.
                let countdown_second = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
                let trade_ver = crate::data::trade_store::TradeStore::global()
                    .lock()
                    .map(|s| s.version)
                    .unwrap_or(0);
                let alert_ver = AlertStore::global()
                    .lock()
                    .map(|s| s.version())
                    .unwrap_or(0);
                let credit_rev = crate::data::credits::CreditStore::revision();
                let feed_changed = ms != last_ms;
                let countdown_changed = countdown_second != last_countdown_second;
                let trade_changed = trade_ver != last_trade_ver;
                let alert_changed = alert_ver != last_alert_ver;
                let credit_changed = credit_rev != last_credit_rev;

                if feed_changed
                    || countdown_changed
                    || trade_changed
                    || alert_changed
                    || credit_changed
                {
                    last_ms = ms;
                    last_countdown_second = countdown_second;
                    last_trade_ver = trade_ver;
                    last_alert_ver = alert_ver;
                    last_credit_rev = credit_rev;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
            }
        })
        .detach();

        let title_str = title.into();
        let tf_str = timeframe.into();
        let parsed_symbol = title_str
            .split_whitespace()
            .next()
            .unwrap_or(&title_str)
            .to_string();

        let (v2_snapshot, _v2_version) = chart_v2_snapshot(&feed);
        let v2_palette = ChartPalette::from_theme(&cx.theme().colors);
        let chart_v2 =
            cx.new(|cx| ChartView::new(v2_snapshot, ChartOptions::default(), v2_palette, cx));
        let last_v2_key = String::new();
        let v2_price_scale_mode = "linear".to_string();
        let manual_price_range = None;
        let v2_percent_anchor = None;
        cx.subscribe(&chart_v2, |panel, _, event: &ChartEvent, cx| {
            if let ChartEvent::ScaleChanged { mode, manual } = event {
                let name = match mode {
                    PriceScaleMode::Linear => "linear",
                    PriceScaleMode::Log => "log",
                    PriceScaleMode::Percent { .. } => "percent",
                    PriceScaleMode::InvertedLinear => "inverted",
                };
                let anchor = mode.anchor();
                if panel.v2_price_scale_mode != name
                    || panel.manual_price_range != *manual
                    || panel.v2_percent_anchor != anchor
                {
                    panel.v2_price_scale_mode = name.to_string();
                    panel.manual_price_range = *manual;
                    panel.v2_percent_anchor = anchor;
                    panel.mark_dirty();
                    cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(650))
                            .await;
                        let _ = this.update(cx, |panel, cx| {
                            panel.check_debounced_save();
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            }
        })
        .detach();

        let mut panel = Self {
            feed,
            title: title_str,
            timeframe: tf_str,
            exchange: "binance".to_string(),
            symbol: parsed_symbol,
            precision: None,
            focus_handle: cx.focus_handle(),
            chart_state: ChartState::new(),
            hover_axis: None,
            axis_drag: None,
            chart_v2,
            last_v2_key,
            v2_price_scale_mode,
            manual_price_range,
            v2_percent_anchor,
            last_chart_width: 800.0,
            last_cursor_x: 400.0,
            candle_type: CandleType::Solid,
            show_horizontal_grid: false,
            timezone: "UTC".into(),
            pane_width: 800.0,
            pane_height: 500.0,
            on_load_more: None,
            on_refresh_latest: None,
            on_open_indicators: None,
            last_completed_candle_ts: 0,
            history_completion: None,
            history_retry_at: None,
            oldest_loaded_ts: 0,
            is_loading_more: false,
            load_more_attempts: 0,
            show_main_series: true,
            // Parameterized indicators & cache
            indicators: Vec::new(),
            indicator_cache: IndicatorCache::new(),
            indicator_hovered: Vec::new(),

            main_legend_hovered: false,
            vol_legend_hovered: false,
            active_legend_settings: None,
            settings_panel_x: 240.0,
            settings_panel_y: 48.0,
            settings_panel_dragging: false,
            settings_drag_start_x: 0.0,
            settings_drag_start_y: 0.0,
            settings_drag_origin_x: 0.0,
            settings_drag_origin_y: 0.0,

            // Drawing tools
            drawings: Vec::new(),
            active_drawing_tool: None,
            drawing_state: DrawingInteractionState::Idle,
            selected_drawing_id: None,
            drawing_color: "#C4E855".to_string(),
            snap_to_candle: true,
            drawing_cursor_point: None,

            // Persistence
            layout_dirty: false,
            last_dirty_time: Instant::now(),
        };

        panel.load_layout();
        panel.sync_chart_v2(cx);
        panel
    }

    fn chart_metrics(&self) -> ChartMetrics {
        let (plot_width, plot_height) =
            self.chart_state.candle_plot_size.get().unwrap_or_else(|| {
                (
                    (self.pane_width - 64.0).max(80.0),
                    self.pane_height.max(80.0),
                )
            });
        ChartMetrics::new(self.pane_width, self.pane_height, plot_width, plot_height)
    }

    /// Which axis rail (if any) is under a window-space point.
    fn axis_at(&self, x: f32, y: f32) -> Option<AxisHandle> {
        let origin = self.chart_state.candle_plot_origin.get()?;
        let size = self.chart_state.candle_plot_size.get()?;
        let bounds = self.chart_state.chart_bounds.get()?;
        axis_at_point(origin, size, bounds.bottom().as_f32(), x, y)
    }

    fn settings_panel_width(&self) -> f32 {
        260.0_f32.min((self.pane_width - 24.0).max(180.0))
    }

    /// Set exchange and symbol context for layout persistence.
    pub fn set_context(&mut self, exchange: impl Into<String>, symbol: impl Into<String>) {
        self.exchange = exchange.into();
        self.symbol = symbol.into();
        self.load_layout();
    }

    pub fn with_context(mut self, exchange: impl Into<String>, symbol: impl Into<String>) -> Self {
        self.set_context(exchange, symbol);
        self
    }

    pub fn set_timezone(&mut self, tz: impl Into<String>) {
        self.timezone = tz.into();
    }

    pub fn set_candle_type(&mut self, candle_type: CandleType) {
        self.candle_type = candle_type;
    }

    pub fn candle_type_config(&self) -> &'static str {
        crate::components::ui::pane_grid::chart_config::candle_type_to_config(self.candle_type)
    }

    /// Exchange, symbol and timeframe for title-bar/floating-window context.
    pub fn market_context(&self) -> (&str, &str, &str) {
        (&self.exchange, &self.symbol, &self.timeframe)
    }

    pub fn indicator_count(&self) -> usize {
        self.indicators.len()
    }

    pub fn indicator_instances(&self) -> &[IndicatorInstance] {
        &self.indicators
    }

    pub fn set_drawing_color_hex(&mut self, hex: &str) {
        self.drawing_color = hex.to_string();
        self.mark_dirty();
    }

    pub fn drawing_color_hex(&self) -> &str {
        &self.drawing_color
    }

    pub fn snap_to_candle(&self) -> bool {
        self.snap_to_candle
    }

    pub fn set_snap_to_candle(&mut self, snap: bool) {
        self.snap_to_candle = snap;
        self.mark_dirty();
    }

    pub fn horizontal_grid_visible(&self) -> bool {
        self.show_horizontal_grid
    }

    pub fn toggle_horizontal_grid(&mut self) {
        self.show_horizontal_grid = !self.show_horizontal_grid;
        self.mark_dirty();
    }

    pub fn reset_view_to_latest(&mut self) {
        let total = self.feed.len();
        self.chart_state.reset_view(total);
    }

    /// Reset both the legacy viewport and the opt-in scene-based chart.
    pub fn reset_view_all(&mut self, cx: &mut Context<Self>) {
        self.reset_view_to_latest();
        self.chart_v2.update(cx, |view, cx| view.reset_view(cx));
    }

    pub fn go_to_latest(&mut self) {
        let total = self.feed.len();
        self.chart_state.scroll_to_latest(total);
        self.chart_state.pinned_to_latest = true;
    }

    /// Jump to latest on both the legacy viewport and the scene-based chart.
    pub fn go_to_latest_all(&mut self, cx: &mut Context<Self>) {
        self.go_to_latest();
        self.chart_v2
            .update(cx, |view, cx| view.scroll_to_latest(cx));
    }

    pub fn active_drawing_tool(&self) -> Option<DrawingTool> {
        self.active_drawing_tool
    }

    pub fn chart_focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn handle_chart_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "escape" => {
                let cleared_tool = if self.active_drawing_tool.is_some() {
                    self.active_drawing_tool = None;
                    self.drawing_state = DrawingInteractionState::Idle;
                    true
                } else if self.selected_drawing_id.is_some() {
                    self.selected_drawing_id = None;
                    false
                } else {
                    false
                };
                cx.notify();
                if cleared_tool {
                    // The title-bar Drawings trigger renders the armed tool
                    // label, so refresh the window that owns that title bar.
                    window.refresh();
                }
            }
            "backspace" | "delete" => {
                self.delete_selected_drawing();
                cx.notify();
            }
            "i" => {
                if let Some(open) = self.on_open_indicators.clone() {
                    open(window, cx);
                }
            }
            "g" => {
                self.toggle_horizontal_grid();
                cx.notify();
            }
            "l" => {
                self.go_to_latest_all(cx);
                cx.notify();
            }
            "f" => {
                self.reset_view_all(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    pub fn with_precision(mut self, precision: PrecisionInfo) -> Self {
        self.precision = Some(precision);
        self
    }

    pub fn with_show_horizontal_grid(mut self, show: bool) -> Self {
        self.show_horizontal_grid = show;
        self
    }

    pub fn set_show_horizontal_grid(&mut self, show: bool) {
        self.show_horizontal_grid = show;
        self.mark_dirty();
    }

    pub fn set_size(&mut self, width: f32, height: f32) -> bool {
        if (self.pane_width - width).abs() < 0.5 && (self.pane_height - height).abs() < 0.5 {
            return false;
        }
        self.pane_width = width;
        self.pane_height = height;
        // last_chart_width belongs to the measured chart canvas, not the pane.
        // Overwriting it here makes toolbar breakpoints oscillate on live updates.
        true
    }

    /// Resize from the pane grid and redraw overlays at the new size.
    pub fn resize(&mut self, width: f32, height: f32, cx: &mut Context<Self>) {
        if self.set_size(width, height) {
            cx.notify();
        }
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.pane_width - width).abs() < 0.5 && (self.pane_height - height).abs() < 0.5
    }

    // -----------------------------------------------------------------------
    // Persistence & Debounce
    // -----------------------------------------------------------------------

    pub fn mark_dirty(&mut self) {
        self.layout_dirty = true;
        self.last_dirty_time = Instant::now();
    }

    pub fn check_debounced_save(&mut self) {
        if self.layout_dirty && self.last_dirty_time.elapsed() >= Duration::from_millis(500) {
            self.save_layout();
            self.layout_dirty = false;
        }
    }

    pub fn is_over_modal(&self, x: f32, y: f32) -> bool {
        if self.settings_panel_dragging {
            return true;
        }
        if self.active_legend_settings.is_some() {
            if x >= self.settings_panel_x
                && x <= self.settings_panel_x + self.settings_panel_width()
                && y >= self.settings_panel_y
                && y <= self.settings_panel_y + 240.0
            {
                return true;
            }
        }
        false
    }

    pub fn load_layout(&mut self) {
        let root = default_chart_dir();
        let layout = load_layout(&root, &self.exchange, &self.symbol, &self.timeframe);
        if !layout.indicators.is_empty() {
            self.indicators = layout.indicators;
        }
        self.drawings = layout.drawings;
        self.show_horizontal_grid = layout.show_grid;
        self.v2_price_scale_mode = layout.price_scale_mode;
        self.manual_price_range = layout.manual_price_range;
        self.v2_percent_anchor = layout.percent_anchor;
    }

    pub fn save_layout(&self) {
        let root = default_chart_dir();
        let mut layout = ChartLayout::new(&self.exchange, &self.symbol, &self.timeframe);
        layout.candle_type = match self.candle_type {
            CandleType::Solid => "solid".into(),
            CandleType::Stroke => "stroke".into(),
            CandleType::UpStroke => "up_stroke".into(),
            CandleType::DownStroke => "down_stroke".into(),
            CandleType::Ohlc => "ohlc".into(),
            CandleType::Area => "area".into(),
            CandleType::HeikinAshi => "heikin_ashi".into(),
        };
        layout.show_grid = self.show_horizontal_grid;
        layout.timezone = self.timezone.clone();
        layout.indicators = self.indicators.clone();
        layout.drawings = self.drawings.clone();
        layout.price_scale_mode = self.v2_price_scale_mode.clone();
        layout.manual_price_range = self.manual_price_range;
        layout.percent_anchor = self.v2_percent_anchor;
        // The in-chart indicator library is gone; keep the field parseable for
        // old layout files but stop writing a new geometry.
        layout.library_geometry = None;
        let _ = save_layout(&root, &layout);
    }

    // -----------------------------------------------------------------------
    // Indicator Instances
    // -----------------------------------------------------------------------

    pub fn add_indicator_instance(&mut self, kind: IndicatorKind) -> String {
        let id = unique_indicator_id(&self.indicators, &kind);
        let instance = IndicatorInstance::new(&id, kind);
        self.indicators.push(instance);
        self.mark_dirty();
        id
    }

    pub fn remove_indicator_instance(&mut self, id: &str) {
        self.indicators.retain(|ind| ind.id != id);
        self.mark_dirty();
    }

    pub fn toggle_indicator_instance(&mut self, id: &str) {
        if let Some(ind) = self.indicators.iter_mut().find(|i| i.id == id) {
            ind.visible = !ind.visible;
            self.mark_dirty();
        }
    }

    pub fn update_indicator_params(
        &mut self,
        id: &str,
        params: BTreeMap<String, f64>,
    ) -> Result<(), String> {
        let ind = self
            .indicators
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| "Indicator not found".to_string())?;

        validate_params(&ind.kind, &params)?;
        ind.params = params;
        self.mark_dirty();
        Ok(())
    }

    // Backwards-compatible indicator helper methods
    pub fn toggle_indicator(&mut self, kind: IndicatorKind) {
        if let Some(pos) = self.indicators.iter().position(|i| i.kind == kind) {
            self.indicators[pos].visible = !self.indicators[pos].visible;
        } else {
            self.add_indicator_instance(kind);
        }
        self.mark_dirty();
    }

    pub fn remove_indicator(&mut self, kind: IndicatorKind) {
        self.indicators.retain(|i| i.kind != kind);
        self.mark_dirty();
    }

    pub fn indicator_enabled(&self, kind: IndicatorKind) -> bool {
        self.indicators.iter().any(|i| i.kind == kind && i.visible)
    }

    // -----------------------------------------------------------------------
    // Drawing Tools
    // -----------------------------------------------------------------------

    pub fn set_active_drawing_tool(&mut self, tool: Option<DrawingTool>) {
        self.active_drawing_tool = tool;
        self.drawing_state = DrawingInteractionState::Idle;
        self.selected_drawing_id = None;
    }

    pub fn clear_all_drawings(&mut self) {
        self.drawings.clear();
        self.selected_drawing_id = None;
        self.mark_dirty();
    }

    pub fn delete_selected_drawing(&mut self) {
        if let Some(id) = self.selected_drawing_id.take() {
            self.drawings.retain(|d| d.id != id);
            self.mark_dirty();
        }
    }

    // -----------------------------------------------------------------------
    // Indicator Calculation & Colors
    // -----------------------------------------------------------------------

    fn indicator_color(kind: IndicatorKind, index: usize) -> gpui::Hsla {
        let palette: &[u32] = match kind {
            IndicatorKind::Ma => &[0xf4a261ff, 0x2a9d8fff, 0xe76f51ff],
            IndicatorKind::Ema => &[0x3a86ffff, 0x8338ecff, 0xff006eff],
            IndicatorKind::Boll => &[0xe0aaffff, 0xc77dffff, 0x9d4eddff],
            IndicatorKind::Macd => &[0x06d6a0ff, 0xffd166ff, 0xef476fff],
            IndicatorKind::Kdj => &[0xf72585ff, 0x7209b7ff, 0x4cc9f0ff],
            IndicatorKind::Rsi => &[0xffb703ff],
            IndicatorKind::Vol => &[0xb185d2ff],
            _ => &[0xc4e855ff, 0x00f0ffff, 0xffd600ff, 0xff0055ff],
        };
        gpui::rgba(palette[index % palette.len()]).into()
    }

    fn compute_indicators_for_chart(
        &mut self,
        candles: &[OhlcvCandle],
        feed_version: u64,
    ) -> (Vec<IndicatorLineSpec>, usize) {
        let mut lines = Vec::new();
        let has_vol = self
            .indicators
            .iter()
            .any(|i| i.kind == IndicatorKind::Vol && i.visible);
        let mut subpane_idx = if has_vol { 1 } else { 0 };

        for (ind_idx, ind) in self.indicators.iter().enumerate() {
            if !ind.visible {
                continue;
            }

            // Volume is rendered natively by CandlestickChart via show_volume_pane(true)
            if ind.kind == IndicatorKind::Vol {
                continue;
            }

            let computed = self
                .indicator_cache
                .get_or_compute(feed_version, ind, candles);

            let is_subpane = ind.kind.default_pane() == "subpane" || ind.pane == "subpane";
            let target_pane = if is_subpane {
                let p = PaneTarget::Subpane(subpane_idx);
                subpane_idx += 1;
                p
            } else {
                PaneTarget::Main
            };

            for spec in computed.iter() {
                let color = if ind.style.color != "#C4E855" && !ind.style.color.is_empty() {
                    crate::chart::color::hex_to_hsla(&ind.style.color).unwrap_or_else(|| {
                        Self::indicator_color(ind.kind.clone(), spec.color_offset + ind_idx)
                    })
                } else {
                    Self::indicator_color(ind.kind.clone(), spec.color_offset + ind_idx)
                };

                let style = if spec.is_histogram {
                    IndicatorStyle::Histogram
                } else {
                    IndicatorStyle::Line
                };

                let pane = if spec.is_main_pane {
                    PaneTarget::Main
                } else {
                    target_pane
                };

                lines.push(IndicatorLineSpec {
                    instance_id: ind.id.clone(),
                    kind: ind.kind.clone(),
                    name: spec.name.clone(),
                    series: spec.series.clone(),
                    color,
                    style,
                    pane,
                });
            }
        }

        (lines, subpane_idx)
    }

    // -----------------------------------------------------------------------
    // Helper Methods
    // -----------------------------------------------------------------------

    fn format_timestamp(timestamp_ms: i64, timezone: &str, format: &str) -> String {
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp_ms / 1_000, 0)
            .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
        let rendered = timezone
            .parse::<chrono_tz::Tz>()
            .map(|tz| timestamp.with_timezone(&tz).format(format).to_string());
        rendered.unwrap_or_else(|_| timestamp.format(format).to_string())
    }

    #[allow(dead_code)]
    fn format_time(timestamp_ms: i64) -> String {
        Self::format_timestamp(timestamp_ms, "UTC", "%H:%M")
    }

    fn render_empty(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let is_configured = crate::data::client::is_api_url_configured();
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(div().w(px(40.0)).h(px(2.0)).bg(if is_configured { colors.primary } else { colors.muted_foreground.opacity(0.4) }))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if is_configured { colors.muted_foreground } else { colors.warning })
                    .child(if is_configured { "LOADING MARKET DATA" } else { "NO DATA FETCHED" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(if is_configured {
                        format!("{} · {}", self.title, self.timeframe)
                    } else {
                        "Guest session: no API service URL configured. Configure in Settings to stream candles.".to_string()
                    }),
            )
    }

    fn render_error(&self, error: &str, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(div().w(px(40.0)).h(px(2.0)).bg(colors.bearish))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors.bearish)
                    .child("CONNECTION ERROR"),
            )
            .child(
                div()
                    .max_w(px(300.0))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(error.to_string()),
            )
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for ChartPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let credit_error = self
            .feed
            .credit_error()
            .or_else(|| crate::data::credits::CreditStore::module_error("Chart"));

        if let Some(ref ce) = credit_error {
            return v_flex()
                .size_full()
                .bg(cx.theme().colors.background)
                .child(
                    crate::components::ui::credit_alert::render_credits_exhausted_empty(
                        "Chart Data",
                        Some(ce),
                        false,
                        cx.theme(),
                    ),
                )
                .into_any_element();
        }

        if chart_v2_enabled() {
            self.check_debounced_save();
            let key = format!(
                "{:?}|{}|{}|{}|{}|{:?}|{:?}",
                self.precision,
                self.timezone,
                self.show_horizontal_grid,
                self.feed.version(),
                self.v2_price_scale_mode,
                self.manual_price_range,
                self.v2_percent_anchor
            );
            if key != self.last_v2_key {
                self.sync_chart_v2(cx);
            }
            let palette = ChartPalette::from_theme(&cx.theme().colors);
            self.chart_v2
                .update(cx, |view, cx| view.set_palette_if_changed(palette, cx));
            let root = v_flex()
                .size_full()
                .bg(cx.theme().colors.background)
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    this.handle_chart_key(ev, window, cx);
                }))
                .child(div().flex_1().min_h_0().child(self.chart_v2.clone()));
            return root.into_any_element();
        }

        // The feed watcher and interaction handlers notify this entity. Avoid
        // scheduling an unconditional 60 FPS render loop when the chart is idle.

        // Track chart width for dynamic toolbar
        if let Some((w, _)) = self.chart_state.candle_plot_size.get() {
            if w > 0.0 {
                self.last_chart_width = w;
            }
        }

        // Check debounced persistence
        self.check_debounced_save();

        let (candles_snap, feed_version) = self.feed.snapshot_with_version();
        let metrics = self.chart_metrics();
        let current_oldest = candles_snap.first().map(|c| c.timestamp).unwrap_or(0);
        if current_oldest > 0 && current_oldest != self.oldest_loaded_ts {
            if self.oldest_loaded_ts > 0 {
                let prepended_count = candles_snap
                    .iter()
                    .take_while(|c| c.timestamp < self.oldest_loaded_ts)
                    .count();
                if prepended_count > 0 {
                    self.chart_state.viewport_offset += prepended_count as f64;
                }
            }
            self.oldest_loaded_ts = current_oldest;
            self.is_loading_more = false;
            self.history_completion = None;
            self.history_retry_at = None;
            self.load_more_attempts = 0;
        }

        if let Some(completion) = &self.history_completion {
            if !matches!(
                completion.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ) {
                self.is_loading_more = false;
                self.history_completion = None;
                self.history_retry_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(15));
            }
        }

        if !self.is_loading_more
            && self
                .history_retry_at
                .is_none_or(|at| std::time::Instant::now() >= at)
            && self.load_more_attempts < 3
            && current_oldest > 0
            && self.chart_state.viewport_offset <= 15.0
        {
            if let Some(on_load_more) = &self.on_load_more {
                let (tx, rx) = std::sync::mpsc::channel();
                self.is_loading_more = true;
                self.history_completion = Some(rx);
                self.load_more_attempts = self.load_more_attempts.saturating_add(1);
                on_load_more(current_oldest - 1, tx);
            }
        }

        let mut candles: Vec<OhlcvCandle> = candles_snap
            .into_iter()
            .filter_map(|mut c| {
                if c.timestamp <= 0
                    || !c.open.is_finite()
                    || !c.high.is_finite()
                    || !c.low.is_finite()
                    || !c.close.is_finite()
                {
                    return None;
                }
                c.high = c.high.max(c.open).max(c.close);
                c.low = c.low.min(c.open).min(c.close);
                if !c.volume.is_finite() {
                    c.volume = 0.0;
                }
                Some(c)
            })
            .collect();

        // A new candle appeared → the previous one just closed. Ask upstream to
        // re-fetch after a short delay so the exchange has finalized the bar.
        let latest_ts = candles.last().map(|c| c.timestamp).unwrap_or(0);
        if latest_ts > 0 {
            if self.last_completed_candle_ts > 0 && latest_ts > self.last_completed_candle_ts {
                if let Some(refresh) = &self.on_refresh_latest {
                    let refresh = refresh.clone();
                    cx.background_executor()
                        .spawn(async move {
                            // Wait 1.5s for upstream exchange to finalize the closed bar
                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                            refresh();
                        })
                        .detach();
                }
            }
            self.last_completed_candle_ts = latest_ts;
        }

        // NOTE: The candle series from `self.feed.snapshot_with_version()` is
        // authoritative. `CandleFeed` already subscribes to and maintains the
        // live forming candle for `(self.exchange, self.symbol)`. Do not
        // overwrite it with `TradeStore::get_mark_price(&self.symbol)` — that
        // lookup is venue-agnostic, so a ticker from another exchange could
        // corrupt this chart, and ratcheting `last.low`/`last.high` on every
        // render frame permanently stretches the wicks.

        if matches!(self.candle_type, CandleType::HeikinAshi) {
            let mut ha_candles = Vec::with_capacity(candles.len());
            let mut prev_ha_open = candles.first().map(|c| c.open).unwrap_or(0.0);
            let mut prev_ha_close = candles.first().map(|c| c.close).unwrap_or(0.0);

            for c in candles.iter() {
                let ha_close = (c.open + c.high + c.low + c.close) / 4.0;
                let ha_open = if ha_candles.is_empty() {
                    (c.open + c.close) / 2.0
                } else {
                    (prev_ha_open + prev_ha_close) / 2.0
                };
                let ha_high = c.high.max(ha_open).max(ha_close);
                let ha_low = c.low.min(ha_open).min(ha_close);

                let mut ha_candle = c.clone();
                ha_candle.open = ha_open;
                ha_candle.high = ha_high;
                ha_candle.low = ha_low;
                ha_candle.close = ha_close;

                ha_candles.push(ha_candle);

                prev_ha_open = ha_open;
                prev_ha_close = ha_close;
            }
            candles = ha_candles;
        }

        let error = self.feed.last_error();
        if let Some(ref err) = error {
            if candles.is_empty() {
                return v_flex()
                    .size_full()
                    .bg(cx.theme().colors.background)
                    .child(self.render_error(err, cx))
                    .into_any_element();
            }
        }

        if candles.is_empty() {
            return v_flex()
                .size_full()
                .bg(cx.theme().colors.background)
                .child(self.render_empty(cx))
                .into_any_element();
        }

        let total = candles.len();
        if self.chart_state.pinned_to_latest {
            self.chart_state.scroll_to_latest(total);
        } else {
            self.chart_state.clamp_offset(total);
        }

        let chart_st = self.chart_state.clone();
        let candle_type = self.candle_type;
        let precision = self.precision.clone();

        let active_candle = if let Some(idx) = self.chart_state.crosshair_index.get() {
            candles.get(idx)
        } else {
            candles.last()
        };

        // Compute parameterized indicators with the versioned cache.
        let (indicator_lines, subpane_count) =
            self.compute_indicators_for_chart(&candles, feed_version);

        let full_h = self.chart_state.chart_height.get().unwrap_or(0.0);
        let has_subpanes = subpane_count > 0;
        let mut total_subpane_ratio = 0.0;
        let default_subpane_ratio = if metrics.plot_height < 280.0 {
            0.26
        } else {
            0.18
        };
        let mut subpane_ratios = vec![default_subpane_ratio; subpane_count];
        {
            let ratios = self.chart_state.subpane_ratios.borrow();
            for i in 0..subpane_count {
                subpane_ratios[i] = *ratios.get(&i).unwrap_or(&default_subpane_ratio);
                total_subpane_ratio += subpane_ratios[i];
            }
        }
        if total_subpane_ratio > 0.8 {
            let scale = 0.8 / total_subpane_ratio;
            for i in 0..subpane_count {
                subpane_ratios[i] *= scale;
            }
            total_subpane_ratio = 0.8;
        }

        let subpane_total_gap = if has_subpanes {
            2.0 * subpane_count as f32
        } else {
            0.0
        };
        let avail = full_h - 18.0;
        let candle_h = if has_subpanes {
            (avail * (1.0 - total_subpane_ratio) - subpane_total_gap).max(10.0)
        } else {
            avail.max(10.0)
        };

        let mut subpane_legends = Vec::new();
        let legend_ui = if let Some(c) = active_candle {
            let is_bull = c.close >= c.open;
            // Match the chart candles so the crosshair OHLCV readout follows
            // the active theme's bullish/bearish colors in light and dark mode.
            let c_color: gpui::Hsla = if is_bull {
                cx.theme().bullish
            } else {
                cx.theme().bearish
            };
            let prec = precision.clone();
            let fmt_price = move |p: f64| format_price(p, prec.as_ref());

            let change_val = c.close - c.open;
            let change_pct = (change_val / c.open.max(0.00000001)) * 100.0;
            let change_sign = if change_val > 0.0 { "+" } else { "" };
            let change_str = format!(
                "{}{:.2} ({}{:.2}%)",
                change_sign, change_val, change_sign, change_pct
            );

            let mut legend = div()
                .absolute()
                .top(px(8.0))
                .left(px(12.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(metrics.legend_text_size))
                        .id("main_legend_row")
                        .bg(gpui::transparent_black())
                        .on_hover(cx.listener(|this, hovered: &bool, _win, cx| {
                            if this.main_legend_hovered != *hovered {
                                this.main_legend_hovered = *hovered;
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .text_color(cx.theme().colors.muted_foreground)
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(self.title.clone()),
                                )
                                .children(if self.main_legend_hovered {
                                    Some(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .ml(px(4.0))
                                            .child(
                                                div()
                                                    .id("main_legend_eye")
                                                    .cursor_pointer()
                                                    .text_color(cx.theme().colors.muted_foreground)
                                                    .hover(|s| s.text_color(cx.theme().colors.foreground))
                                                    .on_mouse_down(
                                                        gpui::MouseButton::Left,
                                                        cx.listener(|this, _ev, _win, cx| {
                                                            cx.stop_propagation();
                                                            this.show_main_series =
                                                                !this.show_main_series;
                                                            cx.notify();
                                                        }),
                                                    )
                                                    .child(
                                                        gpui_component::Icon::default()
                                                            .path(if self.show_main_series {
                                                                "icons/eye.svg"
                                                            } else {
                                                                "icons/eye-off.svg"
                                                            })
                                                            .size(px(14.0)),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .id("main_legend_settings")
                                                    .cursor_pointer()
                                                    .text_color(cx.theme().colors.muted_foreground)
                                                    .hover(|s| s.text_color(cx.theme().colors.foreground))
                                                    .on_mouse_down(
                                                        gpui::MouseButton::Left,
                                                        cx.listener(
                                                            |this, ev: &MouseDownEvent, _win, cx| {
                                                                cx.stop_propagation();
                                                                this.active_legend_settings =
                                                                    Some(LegendSettingsTarget::Main);
                                                                this.settings_panel_x =
                                                                    ev.position.x.as_f32() + 12.0;
                                                                this.settings_panel_y =
                                                                    ev.position.y.as_f32() + 12.0;
                                                                cx.notify();
                                                            },
                                                        ),
                                                    )
                                                    .child(
                                                        gpui_component::Icon::default()
                                                            .path("icons/settings.svg")
                                                            .size(px(14.0)),
                                                    ),
                                            ),
                                    )
                                } else {
                                    None
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .child(div().text_color(cx.theme().colors.muted_foreground).child("O"))
                                .child(div().text_color(c_color).child(fmt_price(c.open))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .child(div().text_color(cx.theme().colors.muted_foreground).child("H"))
                                .child(div().text_color(c_color).child(fmt_price(c.high))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .child(div().text_color(cx.theme().colors.muted_foreground).child("L"))
                                .child(div().text_color(c_color).child(fmt_price(c.low))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .child(div().text_color(cx.theme().colors.muted_foreground).child("C"))
                                .child(div().text_color(c_color).child(fmt_price(c.close))),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .child(div().text_color(c_color).child(change_str)),
                        )
                );

            let active_timestamp = c.timestamp;
            let mut subpane_legends_map: std::collections::BTreeMap<usize, (f32, Vec<AnyElement>)> =
                std::collections::BTreeMap::new();

            for (ind_idx, ind) in self.indicators.iter().enumerate() {
                if !ind.visible {
                    continue;
                }
                if ind.kind == IndicatorKind::Vol {
                    continue;
                }
                let lines_for_ind: Vec<&IndicatorLineSpec> = indicator_lines
                    .iter()
                    .filter(|l| l.instance_id == ind.id)
                    .collect();
                if lines_for_ind.is_empty() {
                    continue;
                }

                let is_hovered = self.indicator_hovered.contains(&ind_idx);
                let ind_id_str = ind.id.clone();
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_xs()
                    .id(SharedString::from(format!("legend_row_{}", ind.id)))
                    .bg(gpui::transparent_black())
                    .on_hover(cx.listener(move |this, hovered: &bool, _win, cx| {
                        if *hovered {
                            if !this.indicator_hovered.contains(&ind_idx) {
                                this.indicator_hovered.push(ind_idx);
                                cx.notify();
                            }
                        } else {
                            this.indicator_hovered.retain(|&i| i != ind_idx);
                            cx.notify();
                        }
                    }));

                let title_col = div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(cx.theme().colors.muted_foreground)
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(ind.display_label()),
                    )
                    .children(if is_hovered {
                        Some(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .ml(px(4.0))
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .text_color(cx.theme().colors.muted_foreground)
                                        .hover(|s| s.text_color(cx.theme().colors.foreground))
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener({
                                                let id = ind_id_str.clone();
                                                move |this, _ev, _win, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_indicator_instance(&id);
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(
                                            gpui_component::Icon::default()
                                                .path(if ind.visible {
                                                    "icons/eye.svg"
                                                } else {
                                                    "icons/eye-off.svg"
                                                })
                                                .size(px(14.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .text_color(cx.theme().colors.muted_foreground)
                                        .hover(|s| s.text_color(cx.theme().colors.foreground))
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener(|this, _ev, window, cx| {
                                                cx.stop_propagation();
                                                if let Some(open) = this.on_open_indicators.clone()
                                                {
                                                    open(window, cx);
                                                }
                                            }),
                                        )
                                        .child(
                                            gpui_component::Icon::default()
                                                .path("icons/settings.svg")
                                                .size(px(14.0)),
                                        ),
                                )
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .text_color(cx.theme().colors.bearish)
                                        .hover(|s| {
                                            s.text_color(cx.theme().colors.bearish.opacity(0.8))
                                        })
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener({
                                                let id = ind_id_str.clone();
                                                move |this, _ev, _win, cx| {
                                                    cx.stop_propagation();
                                                    this.remove_indicator_instance(&id);
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                        .child(
                                            gpui_component::Icon::default()
                                                .path("icons/close.svg")
                                                .size(px(14.0)),
                                        ),
                                ),
                        )
                    } else {
                        None
                    });

                row = row.child(title_col);

                for l in &lines_for_ind {
                    let val = match l
                        .series
                        .binary_search_by_key(&active_timestamp, |&(ts, _)| ts)
                    {
                        Ok(idx) => l.series[idx].1,
                        Err(_) => f64::NAN,
                    };
                    let val_str = if val.is_nan() {
                        "--".to_string()
                    } else {
                        format!("{:.4}", val)
                    };
                    row = row.child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(div().text_color(l.color).child(format!("{}:", l.name)))
                            .child(div().text_color(l.color).child(val_str)),
                    );
                }

                let pane_target = lines_for_ind[0].pane;
                match pane_target {
                    PaneTarget::Subpane(idx) => {
                        let mut y0 = candle_h + 2.0;
                        for j in 0..idx {
                            y0 += avail * subpane_ratios[j] + 2.0;
                        }
                        y0 += 8.0;
                        let y0 = if full_h > 0.0 {
                            y0
                        } else {
                            200.0 + idx as f32 * 50.0
                        };
                        subpane_legends_map
                            .entry(idx)
                            .or_insert_with(|| (y0, Vec::new()))
                            .1
                            .push(row.into_any_element());
                    }
                    _ => {
                        legend = legend.child(row);
                    }
                }
            }

            for (_idx, (y0, rows)) in subpane_legends_map {
                let container = gpui::div()
                    .absolute()
                    .top(gpui::px(y0))
                    .left(gpui::px(12.0))
                    .flex()
                    .flex_col()
                    .gap(gpui::px(4.0))
                    .children(rows);
                subpane_legends.push(container.into_any_element());
            }

            legend
        } else {
            div()
        };

        // Volume legend
        let vol_y = if full_h > 0.0 {
            candle_h + 2.0 + 8.0
        } else {
            200.0
        };

        let has_vol = self
            .indicators
            .iter()
            .any(|i| i.kind == IndicatorKind::Vol && i.visible);
        let legend_vol_ui: AnyElement = if !has_vol {
            div().into_any_element()
        } else if let Some(c) = active_candle {
            let format_vol = |v: f64| {
                if v >= 1_000_000_000.0 {
                    format!("{:.2}B", v / 1_000_000_000.0)
                } else if v >= 1_000_000.0 {
                    format!("{:.2}M", v / 1_000_000.0)
                } else if v >= 1_000.0 {
                    format!("{:.2}K", v / 1_000.0)
                } else {
                    format!("{:.2}", v)
                }
            };
            div()
                .absolute()
                .top(px(vol_y))
                .left(px(12.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_sm()
                .id("vol_legend_row")
                .bg(gpui::transparent_black())
                .on_hover(cx.listener(|this, hovered: &bool, _win, cx| {
                    if this.vol_legend_hovered != *hovered {
                        this.vol_legend_hovered = *hovered;
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_color(cx.theme().colors.muted_foreground)
                                .font_weight(gpui::FontWeight::BOLD)
                                .child("Vol"),
                        )
                        .children(if self.vol_legend_hovered {
                            Some(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .ml(px(4.0))
                                    .child(
                                        div()
                                            .id("vol_legend_eye")
                                            .cursor_pointer()
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .hover(|s| s.text_color(cx.theme().colors.foreground))
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                cx.listener(|this, _ev, _win, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_indicator(IndicatorKind::Vol);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(
                                                gpui_component::Icon::default()
                                                    .path("icons/eye.svg")
                                                    .size(px(14.0)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("vol_legend_close")
                                            .cursor_pointer()
                                            .text_color(cx.theme().colors.muted_foreground)
                                            .hover(|s| s.text_color(cx.theme().colors.bearish))
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                cx.listener(|this, _ev, _win, cx| {
                                                    cx.stop_propagation();
                                                    this.remove_indicator(IndicatorKind::Vol);
                                                    cx.notify();
                                                }),
                                            )
                                            .child(
                                                gpui_component::Icon::default()
                                                    .path("icons/close.svg")
                                                    .size(px(14.0)),
                                            ),
                                    ),
                            )
                        } else {
                            None
                        }),
                )
                .child({
                    let vol_color: gpui::Hsla = gpui::rgba(0xb185d2ff).into();
                    div().text_color(vol_color).child(format_vol(c.volume))
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // Quick settings modal
        let settings_panel: AnyElement = if let Some(target) = self.active_legend_settings {
            let is_main = target == LegendSettingsTarget::Main;
            let title = match target {
                LegendSettingsTarget::Main => self.title.clone(),
                LegendSettingsTarget::Volume => "Vol".to_string(),
                LegendSettingsTarget::Indicator(idx) => self
                    .indicators
                    .get(idx)
                    .map(|i| i.display_label())
                    .unwrap_or_default(),
            };
            div()
                .absolute()
                .top(px(self.settings_panel_y))
                .left(px(self.settings_panel_x))
                .w(px(self.settings_panel_width()))
                .p(px(10.0))
                .border_1()
                .border_color(cx.theme().colors.border)
                .bg(cx.theme().colors.background)
                .shadow_md()
                .text_sm()
                .cursor_default()
                .on_mouse_move(cx.listener(|this, _ev: &MouseMoveEvent, _win, cx| {
                    cx.stop_propagation();
                    if this.chart_state.crosshair_pos.is_some() {
                        this.chart_state.crosshair_pos = None;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|_this, _ev, _win, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(|_this, _ev, _win, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|_this, _ev, _win, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_scroll_wheel(cx.listener(|_this, _ev, _win, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .cursor_grab()
                        .pb(px(8.0))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _win, cx| {
                                cx.stop_propagation();
                                this.settings_panel_dragging = true;
                                this.settings_drag_start_x = ev.position.x.as_f32();
                                this.settings_drag_start_y = ev.position.y.as_f32();
                                this.settings_drag_origin_x = this.settings_panel_x;
                                this.settings_drag_origin_y = this.settings_panel_y;
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(cx.theme().colors.foreground)
                                .child(format!("{} Settings", title)),
                        )
                        .child(
                            div()
                                .cursor_pointer()
                                .text_color(cx.theme().colors.muted_foreground)
                                .hover(|s| s.text_color(cx.theme().colors.foreground))
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, _ev, _win, cx| {
                                        cx.stop_propagation();
                                        this.active_legend_settings = None;
                                        cx.notify();
                                    }),
                                )
                                .child("×"),
                        ),
                )
                .child(
                    div()
                        .pt(px(6.0))
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(if is_main {
                            "Component: main price series"
                        } else {
                            "Indicator element"
                        }),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // Candlestick chart construction
        let current_mark = candles.last().map(|c| c.close).unwrap_or(0.0);
        let show_candles = self.show_main_series;
        let mut chart = CandlestickChart::new(candles)
            .show_candles(show_candles)
            .subpane_count(subpane_count)
            .show_volume_pane(has_vol)
            .timestamp_ms(|c: &OhlcvCandle| c.timestamp)
            .open(|c: &OhlcvCandle| c.open)
            .high(|c: &OhlcvCandle| c.high)
            .low(|c: &OhlcvCandle| c.low)
            .close(|c: &OhlcvCandle| c.close)
            .body_width_ratio(0.7)
            .timeframe(self.timeframe.clone())
            .candle_type(candle_type)
            .show_horizontal_grid(self.show_horizontal_grid)
            .chart_state(chart_st.clone())
            .manual_price_range(self.manual_price_range)
            .volume(|c: &OhlcvCandle| c.volume)
            .format_timestamp_short({
                let timezone = self.timezone.clone();
                move |timestamp, best_interval, prev_ts_opt| {
                    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp / 1_000, 0)
                        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
                    let tz = timezone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC);
                    let local_dt = dt.with_timezone(&tz);

                    let is_new_day = prev_ts_opt
                        .map(|p| {
                            let prev_dt =
                                chrono::DateTime::<chrono::Utc>::from_timestamp(p / 1_000, 0)
                                    .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
                                    .with_timezone(&tz);
                            prev_dt.date_naive() != local_dt.date_naive()
                        })
                        .unwrap_or(false);

                    let fmt = if best_interval >= 30.0 * 86400_000.0 {
                        "%Y-%m"
                    } else if best_interval >= 86400_000.0 || is_new_day {
                        "%m-%d"
                    } else if prev_ts_opt.is_none() {
                        "%m-%d %H:%M"
                    } else {
                        "%H:%M"
                    };
                    local_dt.format(fmt).to_string().into()
                }
            })
            .format_timestamp_long({
                let timezone = self.timezone.clone();
                move |timestamp| Self::format_timestamp(timestamp, &timezone, "%b %d %H:%M").into()
            })
            .format_price(move |price: f64| format_price(price, precision.as_ref()).into());

        for line in &indicator_lines {
            chart = chart.indicator_line(line.series.clone(), line.color, line.style, line.pane);
        }

        // Active positions and open orders overlays
        // NOTE: the chart only publishes a fallback mark price for symbols without
        // a live feed. Overwriting here would fight the tick-feed writer and make
        // PnL flicker between the candle close and the live traded price.
        if current_mark > 0.0 {
            let mut store = crate::data::trade_store::TradeStore::global()
                .lock()
                .unwrap();
            store.sync_chart_mark_price(&self.symbol, current_mark);
        }

        let (pos_overlays, ord_overlays) = {
            let store = crate::data::trade_store::TradeStore::global()
                .lock()
                .unwrap();
            let chart_mark = if current_mark > 0.0 {
                current_mark
            } else {
                store.get_mark_price(&self.symbol).unwrap_or(0.0)
            };

            let pos: Vec<PositionOverlaySpec> = store
                .positions
                .iter()
                .filter(|p| crate::data::trade_store::symbol_matches(&p.symbol, &self.symbol))
                .map(|p| {
                    let liq = if p.leverage > 1 {
                        let factor = 0.9 / (p.leverage as f64);
                        match p.side {
                            crate::data::trade_store::TradeSide::Long => {
                                Some((p.entry_price * (1.0 - factor)).max(0.0))
                            }
                            crate::data::trade_store::TradeSide::Short => {
                                Some(p.entry_price * (1.0 + factor))
                            }
                        }
                    } else {
                        None
                    };
                    // Use the position's own mark price so the chart badge matches
                    // the trade desk / positions panel exactly.
                    let pos_mark = store
                        .get_mark_price(&p.symbol)
                        .filter(|m| m.is_finite() && *m > 0.0)
                        .or_else(|| {
                            if chart_mark.is_finite() && chart_mark > 0.0 {
                                Some(chart_mark)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(p.entry_price);
                    PositionOverlaySpec {
                        id: p.id.clone(),
                        symbol: p.symbol.clone(),
                        is_long: p.side == crate::data::trade_store::TradeSide::Long,
                        entry_price: p.entry_price,
                        quantity: p.quantity,
                        margin: p.margin,
                        leverage: p.leverage,
                        current_price: pos_mark,
                        liquidation_price: liq,
                        is_live: p.mode == crate::data::trade_store::TradingMode::Live,
                    }
                })
                .collect();

            let ord: Vec<OrderOverlaySpec> = store
                .orders
                .iter()
                .filter(|o| {
                    o.status == "OPEN"
                        && crate::data::trade_store::symbol_matches(&o.symbol, &self.symbol)
                })
                .map(|o| OrderOverlaySpec {
                    id: o.id.clone(),
                    symbol: o.symbol.clone(),
                    is_long: o.side == crate::data::trade_store::TradeSide::Long,
                    order_type: match o.order_type {
                        crate::data::trade_store::TradeOrderType::Market => "MARKET".to_string(),
                        crate::data::trade_store::TradeOrderType::Limit => "LIMIT".to_string(),
                    },
                    price: o.price,
                    quantity: o.quantity,
                    status: o.status.to_string(),
                    is_live: o.mode == crate::data::trade_store::TradingMode::Live,
                })
                .collect();

            (pos, ord)
        };

        chart = chart
            .position_overlays(pos_overlays)
            .order_overlays(ord_overlays);

        // Saved price alerts for this pane's exchange/symbol. The store is
        // populated by the alerts sidebar and survives panes created before it.
        let alert_overlays: Vec<AlertOverlaySpec> = {
            let colors = cx.theme().colors;
            AlertStore::global()
                .lock()
                .map(|store| {
                    store
                        .alerts_for(&self.exchange, &self.symbol)
                        .into_iter()
                        .filter(|alert| !alert.is_expired())
                        .map(|alert| {
                            let color = if alert.is_bullish() {
                                colors.bullish
                            } else if alert.is_bearish() {
                                colors.bearish
                            } else {
                                colors.info
                            };
                            AlertOverlaySpec {
                                id: alert.id.clone(),
                                price: alert.price,
                                color,
                                label: format!(
                                    "{} {}",
                                    alert.glyph(),
                                    format_price(alert.price, self.precision.as_ref())
                                )
                                .into(),
                                is_active: alert.is_active(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        chart = chart.alert_overlays(alert_overlays);

        // Render drawing overlay
        let drawing_overlay = {
            let drawings = self.drawings.clone();
            let selected_id = self.selected_drawing_id.clone();
            let chart_st = self.chart_state.clone();

            let preview_drawing = match &self.drawing_state {
                DrawingInteractionState::Placing { tool, points } => {
                    if let Some(cursor_pt) = self.drawing_cursor_point {
                        let mut preview_pts = points.clone();
                        preview_pts.push(cursor_pt);
                        let mut inst = DrawingInstance::new("preview", *tool);
                        inst.points = preview_pts;
                        inst.style.color = self.drawing_color.clone();
                        inst.style.dashed = true;
                        Some(inst)
                    } else {
                        None
                    }
                }
                _ => None,
            };

            div().absolute().size_full().child(gpui::canvas(
                move |_, _, _| {},
                move |_, _, window, _cx| {
                    let origin = chart_st.candle_plot_origin.get();
                    let size = chart_st.candle_plot_size.get();
                    let ts_range = chart_st.candle_ts_range.get();
                    let price_range = chart_st.candle_price_range.get();

                    if let (Some(orig), Some(sz), Some((ts_min, ts_max)), Some((p_min, p_max))) =
                        (origin, size, ts_range, price_range)
                    {
                        let to_screen = |pt: &DrawingPoint| -> (f32, f32) {
                            let x = if ts_max > ts_min {
                                orig.0
                                    + ((pt.timestamp as f64 - ts_min) / (ts_max - ts_min)) as f32
                                        * sz.0
                            } else {
                                orig.0
                            };
                            let y = if p_max > p_min {
                                orig.1 + ((p_max - pt.price) / (p_max - p_min)) as f32 * sz.1
                            } else {
                                orig.1
                            };
                            (x, y)
                        };

                        for d in drawings.iter().chain(preview_drawing.as_ref()) {
                            let is_selected = selected_id.as_deref() == Some(&d.id);
                            let color = crate::chart::color::hex_to_hsla(&d.style.color)
                                .unwrap_or(gpui::rgba(0xc4e855ff).into());

                            match d.tool {
                                DrawingTool::Trendline | DrawingTool::Segment => {
                                    if d.points.len() >= 2 {
                                        let (x0, y0) = to_screen(&d.points[0]);
                                        let (x1, y1) = to_screen(&d.points[1]);
                                        let mut b = PathBuilder::stroke(px(d.style.width as f32));
                                        if d.style.dashed {
                                            b = b.dash_array(&[px(4.0), px(2.0)]);
                                        }
                                        b.move_to(gpui::point(px(x0), px(y0)));
                                        b.line_to(gpui::point(px(x1), px(y1)));
                                        if let Ok(path) = b.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                }
                                DrawingTool::HorizontalLine => {
                                    if let Some(pt) = d.points.first() {
                                        let (_, y0) = to_screen(pt);
                                        let mut b = PathBuilder::stroke(px(d.style.width as f32));
                                        if d.style.dashed {
                                            b = b.dash_array(&[px(4.0), px(2.0)]);
                                        }
                                        b.move_to(gpui::point(px(orig.0), px(y0)));
                                        b.line_to(gpui::point(px(orig.0 + sz.0), px(y0)));
                                        if let Ok(path) = b.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                }
                                DrawingTool::VerticalLine => {
                                    if let Some(pt) = d.points.first() {
                                        let (x0, _) = to_screen(pt);
                                        let mut b = PathBuilder::stroke(px(d.style.width as f32));
                                        if d.style.dashed {
                                            b = b.dash_array(&[px(4.0), px(2.0)]);
                                        }
                                        b.move_to(gpui::point(px(x0), px(orig.1)));
                                        b.line_to(gpui::point(px(x0), px(orig.1 + sz.1)));
                                        if let Ok(path) = b.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                }
                                DrawingTool::Ray => {
                                    if d.points.len() >= 2 {
                                        let (x0, y0) = to_screen(&d.points[0]);
                                        let (x1, y1) = to_screen(&d.points[1]);
                                        let dx = x1 - x0;
                                        let dy = y1 - y0;
                                        let (end_x, end_y) = if dx.abs() > 1e-4 {
                                            let target_x = orig.0 + sz.0;
                                            let scale = (target_x - x0) / dx;
                                            (target_x, y0 + dy * scale)
                                        } else {
                                            (x0, orig.1 + sz.1)
                                        };
                                        let mut b = PathBuilder::stroke(px(d.style.width as f32));
                                        b.move_to(gpui::point(px(x0), px(y0)));
                                        b.line_to(gpui::point(px(end_x), px(end_y)));
                                        if let Ok(path) = b.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                }
                                DrawingTool::Fibonacci => {
                                    if d.points.len() >= 2 {
                                        let (x0, _) = to_screen(&d.points[0]);
                                        let (x1, _) = to_screen(&d.points[1]);
                                        let min_x = x0.min(x1);
                                        let max_x = (x0.max(x1)).max(orig.0 + sz.0);
                                        let levels = klinechart::fibonacci_levels(
                                            d.points[0].price,
                                            d.points[1].price,
                                        );
                                        for (_, price) in levels {
                                            let pt = DrawingPoint {
                                                timestamp: 0,
                                                price,
                                            };
                                            let (_, y) = to_screen(&pt);
                                            let mut b = PathBuilder::stroke(px(1.0));
                                            b.move_to(gpui::point(px(min_x), px(y)));
                                            b.line_to(gpui::point(px(max_x), px(y)));
                                            if let Ok(path) = b.build() {
                                                window.paint_path(path, color);
                                            }
                                        }
                                    }
                                }
                                DrawingTool::PriceChannel => {
                                    if d.points.len() >= 2 {
                                        let (x0, y0) = to_screen(&d.points[0]);
                                        let (x1, y1) = to_screen(&d.points[1]);
                                        let mut b = PathBuilder::stroke(px(d.style.width as f32));
                                        b.move_to(gpui::point(px(x0), px(y0)));
                                        b.line_to(gpui::point(px(x1), px(y1)));
                                        if let Ok(path) = b.build() {
                                            window.paint_path(path, color);
                                        }
                                        if d.points.len() >= 3 {
                                            let (_, y2) = to_screen(&d.points[2]);
                                            let offset_y = y2 - y0;
                                            let mut b2 =
                                                PathBuilder::stroke(px(d.style.width as f32));
                                            b2.move_to(gpui::point(px(x0), px(y0 + offset_y)));
                                            b2.line_to(gpui::point(px(x1), px(y1 + offset_y)));
                                            if let Ok(path) = b2.build() {
                                                window.paint_path(path, color);
                                            }
                                        }
                                    }
                                }
                                DrawingTool::LongPosition | DrawingTool::ShortPosition => {
                                    if d.points.len() >= 2 {
                                        let (x0, y_entry) = to_screen(&d.points[0]);
                                        let (x1, y_target) = to_screen(&d.points[1]);
                                        let min_x = x0.min(x1);
                                        let max_x = x0.max(x1).max(min_x + 60.0);

                                        let green_bounds = gpui::Bounds::from_corners(
                                            gpui::point(px(min_x), px(y_entry.min(y_target))),
                                            gpui::point(px(max_x), px(y_entry.max(y_target))),
                                        );
                                        window.paint_quad(gpui::fill(
                                            green_bounds,
                                            gpui::rgba(0x2ebd8533),
                                        ));

                                        let stop_y = y_entry + (y_entry - y_target) * 0.5;
                                        let red_bounds = gpui::Bounds::from_corners(
                                            gpui::point(px(min_x), px(y_entry.min(stop_y))),
                                            gpui::point(px(max_x), px(y_entry.max(stop_y))),
                                        );
                                        window.paint_quad(gpui::fill(
                                            red_bounds,
                                            gpui::rgba(0xf6465d33),
                                        ));

                                        let mut b = PathBuilder::stroke(px(1.5));
                                        b.move_to(gpui::point(px(min_x), px(y_entry)));
                                        b.line_to(gpui::point(px(max_x), px(y_entry)));
                                        if let Ok(path) = b.build() {
                                            window.paint_path(path, color);
                                        }
                                    }
                                }
                                DrawingTool::Text => {
                                    if let Some(pt) = d.points.first() {
                                        let (x0, y0) = to_screen(pt);
                                        let rect = gpui::Bounds::from_corners(
                                            gpui::point(px(x0), px(y0)),
                                            gpui::point(px(x0 + 80.0), px(y0 + 20.0)),
                                        );
                                        window.paint_quad(gpui::fill(rect, gpui::rgba(0x1e222dcc)));
                                    }
                                }
                            }

                            if is_selected {
                                for pt in &d.points {
                                    let (px_x, px_y) = to_screen(pt);
                                    let handle = gpui::Bounds::from_corners(
                                        gpui::point(px(px_x - 4.0), px(px_y - 4.0)),
                                        gpui::point(px(px_x + 4.0), px(px_y + 4.0)),
                                    );
                                    window.paint_quad(gpui::fill(handle, gpui::rgba(0xffffffff)));
                                }
                            }
                        }
                    }
                },
            ))
        };

        let is_near_separator = {
            let mut near = false;
            if let Some(pos) = self.chart_state.crosshair_pos {
                let sep_ys = self.chart_state.separator_ys.borrow();
                for (_, &sep_y) in sep_ys.iter() {
                    if (pos.y.as_f32() - sep_y).abs() < 10.0 {
                        near = true;
                        break;
                    }
                }
            }
            near || self.chart_state.is_dragging_separator.is_some()
        };

        let active_axis = match self.axis_drag {
            Some(LegacyAxisDrag::Price { .. }) => Some(AxisHandle::Price),
            Some(LegacyAxisDrag::Time { .. }) => Some(AxisHandle::Time),
            None => self.hover_axis,
        };

        let mut chart_area = div()
            .id("chart_interactive_area")
            .flex_1()
            .w_full()
            .min_h_0()
            .relative();

        if active_axis == Some(AxisHandle::Price) {
            chart_area = chart_area.cursor_row_resize();
        } else if active_axis == Some(AxisHandle::Time) {
            chart_area = chart_area.cursor_col_resize();
        } else if is_near_separator {
            chart_area = chart_area.cursor_row_resize();
        } else {
            chart_area = chart_area.cursor_crosshair();
        }

        let chart_area = chart_area
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.hover_axis = None;
                    if this.chart_state.crosshair_pos.is_some()
                        || this.drawing_cursor_point.is_some()
                    {
                        this.chart_state.crosshair_pos = None;
                        this.drawing_cursor_point = None;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _window, cx| {
                let pos: Point<Pixels> = ev.position;
                if this.settings_panel_dragging {
                    this.chart_state.crosshair_pos = None;
                    this.settings_panel_x =
                        this.settings_drag_origin_x + pos.x.as_f32() - this.settings_drag_start_x;
                    this.settings_panel_y =
                        this.settings_drag_origin_y + pos.y.as_f32() - this.settings_drag_start_y;
                    this.mark_dirty();
                    cx.notify();
                    return;
                }

                // If cursor is over the legend settings window, stop interacting with chart module
                if this.is_over_modal(pos.x.as_f32(), pos.y.as_f32()) {
                    if this.chart_state.crosshair_pos.is_some() {
                        this.chart_state.crosshair_pos = None;
                        cx.notify();
                    }
                    return;
                }

                // Axis rails take priority over separators / panning.
                if let Some(mut drag) = this.axis_drag {
                    let x = pos.x.as_f32();
                    let y = pos.y.as_f32();
                    let total = this.feed.len();
                    match &mut drag {
                        LegacyAxisDrag::Price {
                            start_y,
                            start_range,
                            anchor_price,
                            moved,
                        } => {
                            let dy = y - *start_y;
                            if *moved || dy.abs() >= 2.0 {
                                *moved = true;
                                let range = drag_price_range(
                                    PriceScaleMode::Linear,
                                    *start_range,
                                    *anchor_price,
                                    dy,
                                );
                                if range.0.is_finite() && range.1.is_finite() && range.1 > range.0 {
                                    this.manual_price_range = Some(range);
                                }
                            }
                        }
                        LegacyAxisDrag::Time {
                            start_x,
                            start_visible,
                            anchor_pos,
                            anchor_frac,
                            moved,
                        } => {
                            let dx = x - *start_x;
                            if *moved || dx.abs() >= 2.0 {
                                *moved = true;
                                let new_span = drag_time_span(
                                    *start_visible,
                                    dx,
                                    MIN_VISIBLE as f64,
                                    MAX_VISIBLE as f64,
                                );
                                this.chart_state.resize_span(new_span, total, *anchor_frac);
                                // Keep the original pointer anchor fixed even if the
                                // buffer-edge clamp moved the viewport last frame.
                                this.chart_state.viewport_offset = *anchor_pos
                                    - (this.chart_state.visible_count as f64 - 1.0)
                                        * *anchor_frac as f64;
                                this.chart_state.clamp_offset(total);
                            }
                        }
                    }
                    this.axis_drag = Some(drag);
                    this.chart_state.crosshair_pos = None;
                    this.drawing_cursor_point = None;
                    cx.notify();
                    return;
                }

                // Handle subpane separator dragging
                if let Some(idx) = this.chart_state.is_dragging_separator {
                    let total_h = if let Some(h) = this.chart_state.chart_height.get() {
                        h
                    } else {
                        return;
                    };
                    if total_h > 0.0 {
                        let mut ratios = this.chart_state.subpane_ratios.borrow_mut();
                        let original_ratio = this.chart_state.drag_start_vol_ratio;
                        let dy = pos.y.as_f32() - this.chart_state.drag_start_y;
                        let dr = dy / total_h;
                        let new_ratio = (original_ratio - dr).clamp(0.05, 0.5);
                        ratios.insert(idx, new_ratio);
                    }
                    cx.notify();
                    return;
                }

                // Handle chart panning
                if this.chart_state.is_dragging {
                    let dx = pos.x.as_f32() - this.chart_state.drag_start_x;
                    let candle_width =
                        if this.chart_state.visible_count > 0 && this.last_chart_width > 0.0 {
                            this.last_chart_width as f64 / this.chart_state.visible_count as f64
                        } else {
                            8.0
                        };
                    let candle_delta = -(dx as f64 / candle_width);
                    let total = this.feed.len();
                    this.chart_state.viewport_offset =
                        this.chart_state.drag_start_offset + candle_delta;
                    this.chart_state.clamp_offset(total);
                    this.chart_state.pinned_to_latest = false;
                    cx.notify();
                    return;
                }

                // Track which axis rail the pointer is over. Clear the
                // crosshair while over a rail so axis drags feel distinct.
                let hover = this.axis_at(pos.x.as_f32(), pos.y.as_f32());
                let hover_changed = this.hover_axis != hover;
                this.hover_axis = hover;
                if hover.is_some() {
                    this.chart_state.crosshair_pos = None;
                    this.drawing_cursor_point = None;
                    // Preserve wheel-zoom anchoring over the rails: the wheel
                    // handler still reads the last cursor fraction.
                    this.last_cursor_x = pos.x.as_f32();
                    if let Some(w) = this.chart_state.chart_height.get() {
                        if w > 0.0 && this.last_chart_width > 0.0 {
                            this.chart_state
                                .last_cursor_frac
                                .set((pos.x.as_f32() / this.last_chart_width).clamp(0.0, 1.0));
                        }
                    }
                    if hover_changed {
                        cx.notify();
                    }
                    return;
                }
                if hover_changed {
                    cx.notify();
                }

                // Compute cursor data point (timestamp, price)
                let origin = this.chart_state.candle_plot_origin.get();
                let size = this.chart_state.candle_plot_size.get();
                let ts_range = this.chart_state.candle_ts_range.get();
                let price_range = this.chart_state.candle_price_range.get();

                if let (Some(orig), Some(sz), Some((ts_min, ts_max)), Some((p_min, p_max))) =
                    (origin, size, ts_range, price_range)
                {
                    let mouse_x = pos.x.as_f32();
                    let mouse_y = pos.y.as_f32();
                    let frac_y = ((mouse_y - orig.1) / sz.1.max(1.0)).clamp(0.0, 1.0) as f64;
                    let p = p_max - frac_y * (p_max - p_min);

                    // Slot-accurate timestamp: the chart lays candles out on
                    // discrete slots (see `candlestick_chart.rs`), so snap to
                    // the slot under the cursor. Blank past/future slots use
                    // the same timeframe extrapolation as the crosshair pill.
                    let visible = this.feed.visible_slice(&this.chart_state);
                    let chart_w = sz.0;
                    let ts = if !visible.is_empty() && chart_w > 0.0 {
                        let visible_count_f = this.chart_state.visible_count.max(1) as f32;
                        let slot_w = (chart_w / visible_count_f).max(1.0);
                        let offset = this.chart_state.viewport_offset;
                        let vis_start =
                            this.chart_state.visible_range(this.feed.len()).start as i64;
                        let slot = ((((mouse_x - orig.0) as f64) / (slot_w as f64)) + offset)
                            .floor() as i64;
                        let rel = slot - vis_start;
                        let interval = timeframe_interval_ms(&this.timeframe).unwrap_or(60_000);
                        if rel >= 0 && (rel as usize) < visible.len() {
                            visible[rel as usize].timestamp
                        } else if rel < 0 {
                            let first = visible.first().map(|c| c.timestamp).unwrap_or(0);
                            timestamp_at_buffer_index(
                                &[first],
                                rel as f64,
                                &this.timeframe,
                                interval,
                            )
                            .unwrap_or(first)
                        } else {
                            let last = visible.last().map(|c| c.timestamp).unwrap_or(0);
                            let future = rel - (visible.len() as i64 - 1);
                            timestamp_at_buffer_index(
                                &[last],
                                future as f64,
                                &this.timeframe,
                                interval,
                            )
                            .unwrap_or(last)
                        }
                    } else {
                        let frac_x = ((mouse_x - orig.0) / sz.0.max(1.0)).clamp(0.0, 1.0) as f64;
                        (ts_min + frac_x * (ts_max - ts_min)).round() as i64
                    };

                    this.drawing_cursor_point = Some(DrawingPoint {
                        timestamp: ts,
                        price: p,
                    });
                }

                this.chart_state.crosshair_pos = Some(pos);
                this.last_cursor_x = pos.x.as_f32();
                if let Some(w) = this.chart_state.chart_height.get() {
                    if w > 0.0 && this.last_chart_width > 0.0 {
                        this.chart_state
                            .last_cursor_frac
                            .set((pos.x.as_f32() / this.last_chart_width).clamp(0.0, 1.0));
                    }
                }
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                    let pos: Point<Pixels> = ev.position;
                    if this.is_over_modal(pos.x.as_f32(), pos.y.as_f32()) {
                        return;
                    }

                    // Check if clicking on an overlay badge button (close position / cancel order)
                    let hit_action = {
                        let hitboxes = this.chart_state.overlay_hitboxes.borrow();
                        hitboxes
                            .iter()
                            .find(|hb| hb.bounds.contains(&pos))
                            .map(|hb| hb.action.clone())
                    };
                    if let Some(action) = hit_action {
                        cx.stop_propagation();
                        match action {
                            ChartOverlayAction::ClosePosition { position_id } => {
                                let last_price = this.feed.snapshot().last().map(|c| c.close);
                                let _ = crate::data::trade_store::TradeStore::global()
                                    .lock()
                                    .unwrap()
                                    .close_position(&position_id, last_price);
                            }
                            ChartOverlayAction::CancelOrder { order_id } => {
                                let _ = crate::data::trade_store::TradeStore::global()
                                    .lock()
                                    .unwrap()
                                    .cancel_order(&order_id);
                            }
                        }
                        cx.notify();
                        return;
                    }

                    // Axis rails: double-click resets, single press starts a drag.
                    if let Some(axis) = this.axis_at(pos.x.as_f32(), pos.y.as_f32()) {
                        if ev.click_count >= 2 {
                            match axis {
                                AxisHandle::Price => {
                                    this.manual_price_range = None;
                                    this.mark_dirty();
                                }
                                AxisHandle::Time => {
                                    let total = this.feed.len();
                                    this.chart_state.reset_view(total);
                                }
                            }
                            this.axis_drag = None;
                            this.chart_state.crosshair_pos = None;
                            this.drawing_cursor_point = None;
                            cx.notify();
                            return;
                        }

                        let origin = this.chart_state.candle_plot_origin.get();
                        let size = this.chart_state.candle_plot_size.get();
                        match axis {
                            AxisHandle::Price => {
                                if let Some(range) = this.chart_state.candle_price_range.get() {
                                    if let Some(anchor_price) =
                                        this.chart_state.y_to_price(pos.y.as_f32())
                                    {
                                        this.axis_drag = Some(LegacyAxisDrag::Price {
                                            start_y: pos.y.as_f32(),
                                            start_range: range,
                                            anchor_price,
                                            moved: false,
                                        });
                                    }
                                }
                            }
                            AxisHandle::Time => {
                                if let (Some((ox, _oy)), Some((chart_w, _h))) = (origin, size) {
                                    let visible_count = this.chart_state.visible_count.max(1);
                                    let anchor_frac = (((pos.x.as_f32() - ox) / chart_w.max(1.0))
                                        .clamp(0.0, 1.0))
                                        as f32;
                                    let anchor_pos = this.chart_state.viewport_offset
                                        + (visible_count as f64 - 1.0) * anchor_frac as f64;
                                    this.axis_drag = Some(LegacyAxisDrag::Time {
                                        start_x: pos.x.as_f32(),
                                        start_visible: visible_count as f64,
                                        anchor_pos,
                                        anchor_frac,
                                        moved: false,
                                    });
                                }
                            }
                        }
                        this.hover_axis = Some(axis);
                        this.chart_state.crosshair_pos = None;
                        this.drawing_cursor_point = None;
                        cx.notify();
                        return;
                    }

                    // 1. Check if clicking near any subpane separator first
                    let hit_sep = {
                        let sep_ys = this.chart_state.separator_ys.borrow();
                        sep_ys.iter().find_map(|(&idx, &sep_y)| {
                            if (pos.y.as_f32() - sep_y).abs() < 10.0 {
                                Some(idx)
                            } else {
                                None
                            }
                        })
                    };
                    if let Some(idx) = hit_sep {
                        this.chart_state.is_dragging_separator = Some(idx);
                        this.chart_state.drag_start_y = pos.y.as_f32();
                        let ratios = this.chart_state.subpane_ratios.borrow();
                        this.chart_state.drag_start_vol_ratio = *ratios.get(&idx).unwrap_or(&0.18);
                        cx.notify();
                        return;
                    }

                    // 2. If a drawing tool is active, place points
                    if let Some(tool) = this.active_drawing_tool {
                        if let Some(mut cur_pt) = this.drawing_cursor_point {
                            // Snap to closest candle if enabled
                            if this.snap_to_candle {
                                let snap_feed = this.feed.snapshot();
                                if let Some(closest) = snap_feed
                                    .iter()
                                    .min_by_key(|c| (c.timestamp - cur_pt.timestamp).abs())
                                {
                                    let prices =
                                        [closest.open, closest.high, closest.low, closest.close];
                                    if let Some(&best_price) = prices.iter().min_by(|a, b| {
                                        (*a - cur_pt.price)
                                            .abs()
                                            .partial_cmp(&(*b - cur_pt.price).abs())
                                            .unwrap()
                                    }) {
                                        cur_pt.price = best_price;
                                        cur_pt.timestamp = closest.timestamp;
                                    }
                                }
                            }

                            if tool.points_required() == 1 {
                                let mut inst = DrawingInstance::new(
                                    &format!("draw-{}", this.drawings.len() + 1),
                                    tool,
                                );
                                inst.points = vec![cur_pt];
                                inst.style.color = this.drawing_color.clone();
                                this.drawings.push(inst);
                                this.active_drawing_tool = None;
                                this.drawing_state = DrawingInteractionState::Idle;
                                this.mark_dirty();
                                cx.notify();
                                return;
                            } else {
                                match &mut this.drawing_state {
                                    DrawingInteractionState::Idle => {
                                        this.drawing_state = DrawingInteractionState::Placing {
                                            tool,
                                            points: vec![cur_pt],
                                        };
                                    }
                                    DrawingInteractionState::Placing { tool: _, points } => {
                                        points.push(cur_pt);
                                        if points.len() >= tool.points_required() {
                                            let mut inst = DrawingInstance::new(
                                                &format!("draw-{}", this.drawings.len() + 1),
                                                tool,
                                            );
                                            inst.points = points.clone();
                                            inst.style.color = this.drawing_color.clone();
                                            this.drawings.push(inst);
                                            this.active_drawing_tool = None;
                                            this.drawing_state = DrawingInteractionState::Idle;
                                            this.mark_dirty();
                                        }
                                    }
                                    _ => {}
                                }
                                cx.notify();
                                return;
                            }
                        }
                    }

                    // 4. Otherwise hit-test existing drawings
                    let origin = this.chart_state.candle_plot_origin.get();
                    let size = this.chart_state.candle_plot_size.get();
                    let ts_range = this.chart_state.candle_ts_range.get();
                    let price_range = this.chart_state.candle_price_range.get();

                    if let (Some(orig), Some(sz), Some((ts_min, ts_max)), Some((p_min, p_max))) =
                        (origin, size, ts_range, price_range)
                    {
                        let to_screen = |ts: i64, price: f64| -> Option<(f64, f64)> {
                            let x = if ts_max > ts_min {
                                orig.0 + ((ts as f64 - ts_min) / (ts_max - ts_min)) as f32 * sz.0
                            } else {
                                orig.0
                            };
                            let y = if p_max > p_min {
                                orig.1 + ((p_max - price) / (p_max - p_min)) as f32 * sz.1
                            } else {
                                orig.1
                            };
                            Some((x as f64, y as f64))
                        };

                        if let Some(hit_id) = hit_test_drawings(
                            pos.x.as_f32() as f64,
                            pos.y.as_f32() as f64,
                            &this.drawings,
                            8.0,
                            to_screen,
                        ) {
                            this.selected_drawing_id = Some(hit_id.clone());
                            this.drawing_state =
                                DrawingInteractionState::Selected { drawing_id: hit_id };
                            cx.notify();
                            return;
                        } else {
                            this.selected_drawing_id = None;
                            this.drawing_state = DrawingInteractionState::Idle;
                        }
                    }

                    // 4. Normal chart pan drag
                    this.chart_state.is_dragging = true;
                    this.chart_state.drag_start_x = pos.x.as_f32();
                    this.chart_state.drag_start_offset = this.chart_state.viewport_offset;
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseUpEvent, _window, cx| {
                    this.settings_panel_dragging = false;
                    this.chart_state.is_dragging = false;
                    this.chart_state.is_dragging_separator = None;
                    if let Some(drag) = this.axis_drag.take() {
                        if matches!(drag, LegacyAxisDrag::Price { moved: true, .. }) {
                            this.mark_dirty();
                        }
                    }
                    let pos: Point<Pixels> = ev.position;
                    if this.is_over_modal(pos.x.as_f32(), pos.y.as_f32()) {
                        cx.notify();
                        return;
                    }
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                    this.settings_panel_dragging = false;
                    this.chart_state.is_dragging = false;
                    this.chart_state.is_dragging_separator = None;
                    if let Some(drag) = this.axis_drag.take() {
                        if matches!(drag, LegacyAxisDrag::Price { moved: true, .. }) {
                            this.mark_dirty();
                        }
                    }
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _window, cx| {
                let pos: Point<Pixels> = ev.position;
                if this.is_over_modal(pos.x.as_f32(), pos.y.as_f32()) {
                    return;
                }
                let total = this.feed.len();
                let delta_y = ev.delta.pixel_delta(px(1.0)).y.as_f32();
                let cursor_frac = this.chart_state.last_cursor_frac.get().clamp(0.0, 1.0);
                if delta_y > 0.0 {
                    this.chart_state.zoom(ZOOM_FACTOR_IN, total, cursor_frac);
                } else if delta_y < 0.0 {
                    this.chart_state.zoom(ZOOM_FACTOR_OUT, total, cursor_frac);
                }

                let delta_x = ev.delta.pixel_delta(px(1.0)).x.as_f32();
                if delta_x.abs() > 1.0 {
                    let candle_width =
                        if this.chart_state.visible_count > 0 && this.last_chart_width > 0.0 {
                            this.last_chart_width as f64 / this.chart_state.visible_count as f64
                        } else {
                            8.0
                        };
                    let candle_delta = -(delta_x as f64 / candle_width);
                    this.chart_state.scroll_by(candle_delta, total);
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.handle_chart_key(ev, window, cx);
            }))
            .overflow_hidden()
            .child(chart)
            .child(drawing_overlay)
            .child(legend_ui)
            .child(legend_vol_ui)
            .children(subpane_legends)
            .child(gpui::deferred(settings_panel));

        let root = v_flex()
            .id("chart_panel_root")
            .size_full()
            .bg(cx.theme().colors.background)
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.hover_axis = None;
                    if this.chart_state.crosshair_pos.is_some()
                        || this.drawing_cursor_point.is_some()
                    {
                        this.chart_state.crosshair_pos = None;
                        this.drawing_cursor_point = None;
                        cx.notify();
                    }
                }
            }));

        root.child(chart_area).into_any_element()
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

impl Panel for ChartPanel {
    fn panel_name(&self) -> &'static str {
        "ChartPanel"
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(format!("{} {}", self.title, self.timeframe))
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        None
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }

    fn closable(&self, _: &App) -> bool {
        true
    }
}

impl Focusable for ChartPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ChartPanel {}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_time_works() {
        let ts = 12 * 3600 * 1000 + 30 * 60 * 1000;
        assert_eq!(ChartPanel::format_time(ts), "12:30");
    }

    #[test]
    fn format_time_handles_midnight() {
        assert_eq!(ChartPanel::format_time(0), "00:00");
    }

    #[test]
    fn format_time_handles_end_of_day() {
        let ts = 23 * 3600 * 1000 + 59 * 60 * 1000;
        assert_eq!(ChartPanel::format_time(ts), "23:59");
    }

    #[test]
    fn indicator_instances_parameter_validation() {
        let mut params = BTreeMap::new();
        params.insert("period".to_string(), 20.0);
        assert!(validate_params(&IndicatorKind::Ma, &params).is_ok());

        params.insert("period".to_string(), 0.0);
        assert!(validate_params(&IndicatorKind::Ma, &params).is_err());
    }

    #[test]
    fn drawing_instance_creation() {
        let mut d = DrawingInstance::new("test-1", DrawingTool::Trendline);
        d.points.push(DrawingPoint {
            timestamp: 100,
            price: 50.0,
        });
        d.points.push(DrawingPoint {
            timestamp: 200,
            price: 60.0,
        });
        assert_eq!(d.points.len(), 2);
    }

    #[test]
    fn test_candle_style_cycling_and_labels() {
        for candle_type in [
            CandleType::Solid,
            CandleType::HeikinAshi,
            CandleType::Stroke,
            CandleType::UpStroke,
            CandleType::DownStroke,
            CandleType::Ohlc,
            CandleType::Area,
        ] {
            let label = match candle_type {
                CandleType::Solid => "Candles",
                CandleType::HeikinAshi => "Heikin Ashi",
                CandleType::Stroke => "Stroke",
                CandleType::UpStroke => "Up stroke",
                CandleType::DownStroke => "Down stroke",
                CandleType::Ohlc => "OHLC",
                CandleType::Area => "Area",
            };
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn test_specialist_drawing_tools_all_have_labels() {
        for tool in [
            DrawingTool::VerticalLine,
            DrawingTool::Ray,
            DrawingTool::Segment,
            DrawingTool::Fibonacci,
            DrawingTool::PriceChannel,
            DrawingTool::Text,
        ] {
            assert!(!tool.label().is_empty());
        }
    }

    #[test]
    fn test_new_chart_has_no_default_indicators() {
        let layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        assert!(layout.indicators.is_empty());
    }

    #[test]
    fn duplicate_indicator_ids_do_not_collide_after_removal() {
        let mut indicators = vec![IndicatorInstance::new("ema-1", IndicatorKind::Ema)];
        let next = unique_indicator_id(&indicators, &IndicatorKind::Ema);
        assert_eq!(next, "ema-2");
        indicators.push(IndicatorInstance::new(next, IndicatorKind::Ema));
        indicators.retain(|instance| instance.id != "ema-1");
        let next = unique_indicator_id(&indicators, &IndicatorKind::Ema);
        assert_eq!(next, "ema-3");
    }

    #[test]
    fn axis_at_point_detects_price_and_time_rails() {
        let origin = (0.0, 0.0);
        let size = (800.0, 400.0);
        let bounds_bottom = 500.0;

        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 798.0, 200.0),
            Some(AxisHandle::Price)
        );
        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 300.0, 490.0),
            Some(AxisHandle::Time)
        );
        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 300.0, 200.0),
            None
        );
    }

    #[test]
    fn axis_at_point_respects_the_rail_edges() {
        let origin = (10.0, 20.0);
        let size = (800.0, 400.0);
        let bounds_bottom = 500.0;

        // Price rail starts 4 px left of the plot's right edge.
        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 806.0, 20.0),
            Some(AxisHandle::Price)
        );
        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 805.9, 20.0),
            None
        );
        // Time rail starts 4 px above the bottom axis strip.
        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 300.0, 478.0),
            Some(AxisHandle::Time)
        );
        assert_eq!(
            axis_at_point(origin, size, bounds_bottom, 300.0, 477.9),
            None
        );
    }

    #[test]
    fn axis_at_point_rejects_non_finite_input() {
        let origin = (0.0, 0.0);
        let size = (800.0, 400.0);
        assert_eq!(axis_at_point(origin, size, 500.0, f32::NAN, 200.0), None);
        assert_eq!(
            axis_at_point(origin, (f32::NAN, 400.0), 500.0, 300.0, 200.0),
            None
        );
    }

    #[test]
    fn price_axis_anchor_uses_the_rendered_scale_inset() {
        // Reproduce the state captured during paint: a 10 px top inset maps
        // the top pixel slightly above the price range's max.
        let mut state = ChartState::new();
        state.candle_plot_origin.set(Some((10.0, 20.0)));
        state.candle_plot_size.set(Some((800.0, 400.0)));
        state.candle_price_range.set(Some((100.0, 200.0)));

        let top = state.y_to_price(20.0).expect("anchored price");
        // The naive (y - origin) / candle_h fraction would return exactly the
        // range max here; the real scale reaches above it because of the inset.
        assert!(top > 200.0);

        let bottom = state.y_to_price(20.0 + 400.0).expect("anchored price");
        assert!((bottom - 100.0).abs() < 1e-9);

        let mid = state.y_to_price(20.0 + 200.0).expect("anchored price");
        let expected = 100.0 + ((200.0 - 400.0) / (10.0 - 400.0)) * 100.0;
        assert!((mid - expected).abs() < 1e-4);
        // The inset means the midpoint is not the naive 150.0.
        assert!((mid - 150.0).abs() > 1.0);
    }
}
