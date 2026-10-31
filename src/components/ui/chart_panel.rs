/// Chart panel — GPU-accelerated interactive candlestick chart.
///
/// Renders OHLCV candlestick data from a `CandleFeed` using gpui-component's
/// `CandlestickChart`. Supports:
///
/// - **Crosshair + OHLCV tooltip** – cursor position is tracked and passed to
///   the chart renderer, which draws hairlines and a floating data box.
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
/// - **Indicator Library Modal** – draggable, resizable, searchable modal with
///   active indicator management and live parameter validation.
///
/// # 60 FPS design
/// `render()` calls `window.request_animation_frame()` every frame. State
/// mutations (mouse events) mark the view dirty via `cx.notify()`, which
/// triggers the next frame immediately.
use gpui::{
    App, Context, Corner, ElementId, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, AnyElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, KeyDownEvent, ParentElement, Pixels, Point,
    Render, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
    PathBuilder, prelude::FluentBuilder,
};
use gpui_component::PixelsExt;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::Selectable;
use gpui_component::chart::{
    CandleType, CandlestickChart, ChartOverlayAction, ChartState, IndicatorStyle, OrderOverlaySpec,
    PaneTarget, PositionOverlaySpec,
};
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::popover::Popover;
use gpui_component::scroll::ScrollableElement;
use gpui_component::theme::ActiveTheme;
use gpui_component::v_flex;

use crate::chart::model::{
    ChartLayout, IndicatorCategory, IndicatorInstance, IndicatorKind, WindowGeometry,
};
use crate::chart::indicators::{validate_params, IndicatorCache};
use crate::chart::drawings::{
    DrawingInstance, DrawingPoint, DrawingTool, DrawingInteractionState, hit_test_drawings,
};
use crate::chart::persistence::{default_chart_dir, load_layout, save_layout};
use crate::data::candle_feed::CandleFeed;
use crate::data::models::{OhlcvCandle, PrecisionInfo};
use crate::data::price_format::format_price;
use crate::klinechart;

use std::collections::BTreeMap;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorPickerDrag {
    Square,
    Hue,
}

pub type TechnicalIndicatorKind = IndicatorKind;
pub type TechnicalIndicatorCategory = IndicatorCategory;

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
    pub on_load_more: Option<std::sync::Arc<dyn Fn(i64, std::sync::mpsc::Sender<()>) + Send + Sync>>,
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

    // Free-floating Indicator Library Modal
    pub show_indicator_library: bool,
    pub library_x: f32,
    pub library_y: f32,
    pub library_w: f32,
    pub library_h: f32,
    library_dragging: bool,
    library_resizing: bool,
    library_drag_start_x: f32,
    library_drag_start_y: f32,
    library_drag_origin_x: f32,
    library_drag_origin_y: f32,
    pub indicator_library_search: String,
    pub indicator_library_category: Option<IndicatorCategory>,
    pub editing_indicator_id: Option<String>,
    pub editing_param_inputs: BTreeMap<String, String>,
    pub editing_param_error: Option<String>,

    // Drawing Tools
    pub drawings: Vec<DrawingInstance>,
    pub active_drawing_tool: Option<DrawingTool>,
    pub drawing_state: DrawingInteractionState,
    pub selected_drawing_id: Option<String>,
    pub drawing_color: String,
    pub snap_to_candle: bool,
    drawing_cursor_point: Option<DrawingPoint>,
    color_picker_drag: Option<ColorPickerDrag>,
    color_picker_hue: f32,
    color_picker_square_bounds: gpui::Bounds<Pixels>,
    color_picker_hue_bounds: gpui::Bounds<Pixels>,

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

impl ChartPanel {
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
                let credit_rev = crate::data::credits::CreditStore::revision();
                let feed_changed = ms != last_ms;
                let countdown_changed = countdown_second != last_countdown_second;
                let trade_changed = trade_ver != last_trade_ver;
                let credit_changed = credit_rev != last_credit_rev;

                if feed_changed || countdown_changed || trade_changed || credit_changed {
                    last_ms = ms;
                    last_countdown_second = countdown_second;
                    last_trade_ver = trade_ver;
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
        let parsed_symbol = title_str.split_whitespace().next().unwrap_or(&title_str).to_string();

        let mut panel = Self {
            feed,
            title: title_str,
            timeframe: tf_str,
            exchange: "binance".to_string(),
            symbol: parsed_symbol,
            precision: None,
            focus_handle: cx.focus_handle(),
            chart_state: ChartState::new(),
            last_chart_width: 800.0,
            last_cursor_x: 400.0,
            candle_type: CandleType::Solid,
            show_horizontal_grid: false,
            timezone: "UTC".into(),
            pane_width: 800.0,
            pane_height: 500.0,
            on_load_more: None,
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

            // Floating Indicator Library Modal
            show_indicator_library: false,
            library_x: 180.0,
            library_y: 60.0,
            library_w: 520.0,
            library_h: 460.0,
            library_dragging: false,
            library_resizing: false,
            library_drag_start_x: 0.0,
            library_drag_start_y: 0.0,
            library_drag_origin_x: 0.0,
            library_drag_origin_y: 0.0,
            indicator_library_search: String::new(),
            indicator_library_category: None,
            editing_indicator_id: None,
            editing_param_inputs: BTreeMap::new(),
            editing_param_error: None,

            // Drawing tools
            drawings: Vec::new(),
            active_drawing_tool: None,
            drawing_state: DrawingInteractionState::Idle,
            selected_drawing_id: None,
            drawing_color: "#C4E855".to_string(),
            snap_to_candle: true,
            drawing_cursor_point: None,
            color_picker_drag: None,
            color_picker_hue: 0.205,
            color_picker_square_bounds: gpui::Bounds::default(),
            color_picker_hue_bounds: gpui::Bounds::default(),

            // Persistence
            layout_dirty: false,
            last_dirty_time: Instant::now(),
        };

        panel.load_layout();
        panel
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

    pub fn set_size(&mut self, width: f32, height: f32) {
        if (self.pane_width - width).abs() < 0.5 && (self.pane_height - height).abs() < 0.5 {
            return;
        }
        self.pane_width = width;
        self.pane_height = height;
        // last_chart_width belongs to the measured chart canvas, not the pane.
        // Overwriting it here makes toolbar breakpoints oscillate on live updates.
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

    pub fn open_main_settings(&mut self, cx: &mut Context<Self>) {
        self.show_indicator_library = true;
        cx.notify();
    }

    pub fn is_over_modal(&self, x: f32, y: f32) -> bool {
        if self.library_dragging || self.library_resizing || self.settings_panel_dragging {
            return true;
        }
        if self.show_indicator_library {
            if x >= self.library_x
                && x <= self.library_x + self.library_w
                && y >= self.library_y
                && y <= self.library_y + self.library_h
            {
                return true;
            }
        }
        if self.active_legend_settings.is_some() {
            if x >= self.settings_panel_x
                && x <= self.settings_panel_x + 280.0
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
        if let Some(geom) = layout.library_geometry {
            self.library_x = geom.x;
            self.library_y = geom.y;
            self.library_w = geom.width;
            self.library_h = geom.height;
        }
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
        layout.library_geometry = Some(WindowGeometry {
            x: self.library_x,
            y: self.library_y,
            width: self.library_w,
            height: self.library_h,
        });
        let _ = save_layout(&root, &layout);
    }

    // -----------------------------------------------------------------------
    // Indicator Instances
    // -----------------------------------------------------------------------

    pub fn add_indicator_instance(&mut self, kind: IndicatorKind) -> String {
        let id = format!("{}-{}", kind.label().to_lowercase(), self.indicators.len() + 1);
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

    fn parse_hex_color(hex: &str) -> Option<gpui::Hsla> {
        let h = hex.trim_start_matches('#');
        if h.len() == 6 {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            Some(gpui::rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | 0xff).into())
        } else {
            None
        }
    }

    fn drawing_color_hsv(&self) -> (f32, f32, f32) {
        let color = Self::parse_hex_color(&self.drawing_color)
            .unwrap_or_else(|| gpui::Hsla::from(gpui::rgba(0xc4e855ff)));
        let rgb = color.to_rgb();
        let max = rgb.r.max(rgb.g.max(rgb.b));
        let min = rgb.r.min(rgb.g.min(rgb.b));
        let delta = max - min;
        let hue = if delta <= f32::EPSILON {
            self.color_picker_hue
        } else if max == rgb.r {
            ((rgb.g - rgb.b) / delta).rem_euclid(6.0) / 6.0
        } else if max == rgb.g {
            (((rgb.b - rgb.r) / delta) + 2.0) / 6.0
        } else {
            (((rgb.r - rgb.g) / delta) + 4.0) / 6.0
        };
        let saturation = if max <= f32::EPSILON { 0.0 } else { delta / max };
        (hue, saturation, max)
    }

    fn hsv_color(hue: f32, saturation: f32, value: f32) -> gpui::Hsla {
        let hue = hue.rem_euclid(1.0);
        let saturation = saturation.clamp(0.0, 1.0);
        let value = value.clamp(0.0, 1.0);
        let chroma = value * saturation;
        let sector = hue * 6.0;
        let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
        let (r, g, b) = match sector.floor() as i32 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        let m = value - chroma;
        gpui::Hsla::from(gpui::Rgba {
            r: r + m,
            g: g + m,
            b: b + m,
            a: 1.0,
        })
    }

    fn set_drawing_color_hsv(&mut self, hue: f32, saturation: f32, value: f32) {
        self.color_picker_hue = hue.clamp(0.0, 1.0);
        let rgb = Self::hsv_color(hue, saturation, value).to_rgb();
        self.drawing_color = format!(
            "#{:02X}{:02X}{:02X}",
            (rgb.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (rgb.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        );
    }

    fn update_color_picker(&mut self, position: Point<Pixels>, target: ColorPickerDrag) {
        let (hue, saturation, value) = self.drawing_color_hsv();
        match target {
            ColorPickerDrag::Square => {
                let bounds = &self.color_picker_square_bounds;
                if bounds.size.width.as_f32() <= 0.0 || bounds.size.height.as_f32() <= 0.0 {
                    return;
                }
                let x = ((position.x - bounds.origin.x).as_f32() / bounds.size.width.as_f32())
                    .clamp(0.0, 1.0);
                let y = ((position.y - bounds.origin.y).as_f32() / bounds.size.height.as_f32())
                    .clamp(0.0, 1.0);
                self.set_drawing_color_hsv(hue, x, 1.0 - y);
            }
            ColorPickerDrag::Hue => {
                let bounds = &self.color_picker_hue_bounds;
                if bounds.size.width.as_f32() <= 0.0 {
                    return;
                }
                let x = ((position.x - bounds.origin.x).as_f32() / bounds.size.width.as_f32())
                    .clamp(0.0, 1.0);
                self.set_drawing_color_hsv(x, saturation, value);
            }
        }
    }

    fn indicator_color(kind: IndicatorKind, index: usize) -> gpui::Hsla {
        let palette: &[u32] = match kind {
            IndicatorKind::Ma => &[0xf4a261ff, 0x2a9d8fff, 0xe76f51ff],
            IndicatorKind::Ema => &[0x3a86ffff, 0x8338ecff, 0xff006eff],
            IndicatorKind::Boll => &[0xe0aaffff, 0xc77dffff, 0x9d4eddff],
            IndicatorKind::Macd => &[0x06d6a0ff, 0xffd166ff, 0xef476fff],
            IndicatorKind::Kdj => &[0xf72585ff, 0x7209b7ff, 0x4cc9f0ff],
            IndicatorKind::Rsi => &[0xffb703ff],
            IndicatorKind::Vol => &[0xb185d2ff],
            _ => &[0xC4E855ff, 0x00F0FFff, 0xFFD600ff, 0xFF0055ff],
        };
        gpui::rgba(palette[index % palette.len()]).into()
    }

    fn compute_indicators_for_chart(
        &mut self,
        candles: &[OhlcvCandle],
        feed_version: u64,
    ) -> (Vec<IndicatorLineSpec>, usize) {
        let mut lines = Vec::new();
        let has_vol = self.indicators.iter().any(|i| i.kind == IndicatorKind::Vol && i.visible);
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
                    Self::parse_hex_color(&ind.style.color)
                        .unwrap_or_else(|| Self::indicator_color(ind.kind.clone(), spec.color_offset + ind_idx))
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
        let credit_error = self.feed.credit_error().or_else(|| {
            crate::data::credits::CreditStore::module_error("Chart")
        });

        if let Some(ref ce) = credit_error {
            return v_flex()
                .size_full()
                .bg(cx.theme().colors.background)
                .child(crate::components::ui::credit_alert::render_credits_exhausted_empty(
                    "Chart Data",
                    Some(ce),
                    false,
                    cx.theme(),
                ))
                .into_any_element();
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
            if !matches!(completion.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                self.is_loading_more = false;
                self.history_completion = None;
                self.history_retry_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(15));
            }
        }

        if !self.is_loading_more
            && self.history_retry_at.is_none_or(|at| std::time::Instant::now() >= at)
            && self.load_more_attempts < 3
            && current_oldest > 0
            && self.chart_state.viewport_offset <= 0.0
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

        if let Some(mark) = crate::data::trade_store::TradeStore::global()
            .lock()
            .ok()
            .and_then(|ts| ts.get_mark_price(&self.symbol))
        {
            if mark > 0.0 && mark.is_finite() {
                if let Some(last) = candles.last_mut() {
                    last.close = mark;
                    last.high = last.high.max(mark);
                    last.low = last.low.min(mark);
                }
            }
        }

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

        // Compute parameterized indicators with versioned cache
        let (indicator_lines, subpane_count) =
            self.compute_indicators_for_chart(&candles, feed_version);

        let full_h = self.chart_state.chart_height.get().unwrap_or(0.0);
        let has_subpanes = subpane_count > 0;
        let mut total_subpane_ratio = 0.0;
        let mut subpane_ratios = vec![0.18; subpane_count];
        {
            let ratios = self.chart_state.subpane_ratios.borrow();
            for i in 0..subpane_count {
                subpane_ratios[i] = *ratios.get(&i).unwrap_or(&0.18);
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
            let c_color: gpui::Hsla = if is_bull {
                gpui::rgba(0x2ebd85ff).into()
            } else {
                gpui::rgba(0xf6465dff).into()
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
                        .text_sm()
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
                        ),
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
                let lines_for_ind: Vec<&IndicatorLineSpec> =
                    indicator_lines.iter().filter(|l| l.instance_id == ind.id).collect();
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
                                            cx.listener({
                                                let id = ind_id_str.clone();
                                                move |this, _ev, _win, cx| {
                                                    cx.stop_propagation();
                                                    this.editing_indicator_id = Some(id.clone());
                                                    this.show_indicator_library = true;
                                                    cx.notify();
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
                                        .hover(|s| s.text_color(cx.theme().colors.bearish.opacity(0.8)))
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

        let has_vol = self.indicators.iter().any(|i| i.kind == IndicatorKind::Vol && i.visible);
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
                LegendSettingsTarget::Indicator(idx) => {
                    self.indicators.get(idx).map(|i| i.display_label()).unwrap_or_default()
                }
            };
            div()
                .absolute()
                .top(px(self.settings_panel_y))
                .left(px(self.settings_panel_x))
                .w(px(260.0))
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
        let mut chart = CandlestickChart::new(candles)
            .show_candles(self.show_main_series)
            .subpane_count(subpane_count)
            .show_volume_pane(has_vol)
            .timestamp_ms(|c: &OhlcvCandle| c.timestamp)
            .open(|c: &OhlcvCandle| c.open)
            .high(|c: &OhlcvCandle| c.high)
            .low(|c: &OhlcvCandle| c.low)
            .close(|c: &OhlcvCandle| c.close)
            .body_width_ratio(0.7)
            .candle_type(candle_type)
            .show_horizontal_grid(self.show_horizontal_grid)
            .chart_state(chart_st.clone())
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
            let mut store = crate::data::trade_store::TradeStore::global().lock().unwrap();
            store.sync_chart_mark_price(&self.symbol, current_mark);
        }

        let (pos_overlays, ord_overlays) = {
            let store = crate::data::trade_store::TradeStore::global().lock().unwrap();
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
                .filter(|o| o.status == "OPEN" && crate::data::trade_store::symbol_matches(&o.symbol, &self.symbol))
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

        chart = chart.position_overlays(pos_overlays).order_overlays(ord_overlays);

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

            div()
                .absolute()
                .size_full()
                .child(gpui::canvas(
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
                                        + ((pt.timestamp as f64 - ts_min) / (ts_max - ts_min))
                                            as f32
                                            * sz.0
                                } else {
                                    orig.0
                                };
                                let y = if p_max > p_min {
                                    orig.1
                                        + ((p_max - pt.price) / (p_max - p_min)) as f32 * sz.1
                                } else {
                                    orig.1
                                };
                                (x, y)
                            };

                            for d in drawings.iter().chain(preview_drawing.as_ref()) {
                                let is_selected = selected_id.as_deref() == Some(&d.id);
                                let color = Self::parse_hex_color(&d.style.color)
                                    .unwrap_or(gpui::rgba(0xc4e855ff).into());

                                match d.tool {
                                    DrawingTool::Trendline | DrawingTool::Segment => {
                                        if d.points.len() >= 2 {
                                            let (x0, y0) = to_screen(&d.points[0]);
                                            let (x1, y1) = to_screen(&d.points[1]);
                                            let mut b =
                                                PathBuilder::stroke(px(d.style.width as f32));
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
                                            let mut b =
                                                PathBuilder::stroke(px(d.style.width as f32));
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
                                            let mut b =
                                                PathBuilder::stroke(px(d.style.width as f32));
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
                                            let mut b =
                                                PathBuilder::stroke(px(d.style.width as f32));
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
                                            let mut b =
                                                PathBuilder::stroke(px(d.style.width as f32));
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
                                            window.paint_quad(gpui::fill(
                                                rect,
                                                gpui::rgba(0x1e222dcc),
                                            ));
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
                                        window.paint_quad(gpui::fill(
                                            handle,
                                            gpui::rgba(0xffffffff),
                                        ));
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

        let mut chart_area = div()
            .id("chart_interactive_area")
            .flex_1()
            .w_full()
            .min_h_0()
            .relative();

        if is_near_separator {
            chart_area = chart_area.cursor_row_resize();
        } else {
            chart_area = chart_area.cursor_crosshair();
        }

        let chart_area = chart_area
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    if this.chart_state.crosshair_pos.is_some() || this.drawing_cursor_point.is_some() {
                        this.chart_state.crosshair_pos = None;
                        this.drawing_cursor_point = None;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _window, cx| {
                let pos: Point<Pixels> = ev.position;
                if this.library_dragging {
                    this.chart_state.crosshair_pos = None;
                    this.library_x = this.library_drag_origin_x + pos.x.as_f32()
                        - this.library_drag_start_x;
                    this.library_y = this.library_drag_origin_y + pos.y.as_f32()
                        - this.library_drag_start_y;
                    this.mark_dirty();
                    cx.notify();
                    return;
                }
                if this.settings_panel_dragging {
                    this.chart_state.crosshair_pos = None;
                    this.settings_panel_x = this.settings_drag_origin_x + pos.x.as_f32()
                        - this.settings_drag_start_x;
                    this.settings_panel_y = this.settings_drag_origin_y + pos.y.as_f32()
                        - this.settings_drag_start_y;
                    this.mark_dirty();
                    cx.notify();
                    return;
                }
                if this.library_resizing {
                    this.chart_state.crosshair_pos = None;
                    this.library_w = (pos.x.as_f32() - this.library_x).max(360.0);
                    this.library_h = (pos.y.as_f32() - this.library_y).max(280.0);
                    this.mark_dirty();
                    cx.notify();
                    return;
                }

                // If cursor is over indicators & strategies window or settings window, stop interacting with chart module
                if this.is_over_modal(pos.x.as_f32(), pos.y.as_f32()) {
                    if this.chart_state.crosshair_pos.is_some() {
                        this.chart_state.crosshair_pos = None;
                        cx.notify();
                    }
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
                    let candle_width = if this.chart_state.visible_count > 0 && this.last_chart_width > 0.0 {
                        this.last_chart_width as f64 / this.chart_state.visible_count as f64
                    } else {
                        8.0
                    };
                    let candle_delta = -(dx as f64 / candle_width);
                    let total = this.feed.len();
                    this.chart_state.viewport_offset = this.chart_state.drag_start_offset + candle_delta;
                    this.chart_state.clamp_offset(total);
                    this.chart_state.pinned_to_latest = false;
                    cx.notify();
                    return;
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
                    let frac_x = ((mouse_x - orig.0) / sz.0.max(1.0)).clamp(0.0, 1.0) as f64;
                    let ts = (ts_min + frac_x * (ts_max - ts_min)).round() as i64;
                    let frac_y = ((mouse_y - orig.1) / sz.1.max(1.0)).clamp(0.0, 1.0) as f64;
                    let p = p_max - frac_y * (p_max - p_min);

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
                                if let Some(closest) = snap_feed.iter().min_by_key(|c| {
                                    (c.timestamp - cur_pt.timestamp).abs()
                                }) {
                                    let prices = [closest.open, closest.high, closest.low, closest.close];
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
                                    DrawingInteractionState::Placing {
                                        tool: _,
                                        points,
                                    } => {
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

                    // 3. Otherwise hit-test existing drawings
                    let origin = this.chart_state.candle_plot_origin.get();
                    let size = this.chart_state.candle_plot_size.get();
                    let ts_range = this.chart_state.candle_ts_range.get();
                    let price_range = this.chart_state.candle_price_range.get();

                    if let (Some(orig), Some(sz), Some((ts_min, ts_max)), Some((p_min, p_max))) =
                        (origin, size, ts_range, price_range)
                    {
                        let to_screen = |ts: i64, price: f64| -> Option<(f64, f64)> {
                            let x = if ts_max > ts_min {
                                orig.0
                                    + ((ts as f64 - ts_min) / (ts_max - ts_min)) as f32 * sz.0
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

                        if let Some(hit_id) =
                            hit_test_drawings(pos.x.as_f32() as f64, pos.y.as_f32() as f64, &this.drawings, 8.0, to_screen)
                        {
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
                    this.library_dragging = false;
                    this.library_resizing = false;
                    this.settings_panel_dragging = false;
                    this.chart_state.is_dragging = false;
                    this.chart_state.is_dragging_separator = None;
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
                    this.library_dragging = false;
                    this.library_resizing = false;
                    this.settings_panel_dragging = false;
                    this.chart_state.is_dragging = false;
                    this.chart_state.is_dragging_separator = None;
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
                    let candle_width = if this.chart_state.visible_count > 0
                        && this.last_chart_width > 0.0
                    {
                        this.last_chart_width as f64 / this.chart_state.visible_count as f64
                    } else {
                        8.0
                    };
                    let candle_delta = -(delta_x as f64 / candle_width);
                    this.chart_state.scroll_by(candle_delta, total);
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => {
                        if this.show_indicator_library {
                            this.show_indicator_library = false;
                        } else if this.active_drawing_tool.is_some() {
                            this.active_drawing_tool = None;
                            this.drawing_state = DrawingInteractionState::Idle;
                        } else if this.selected_drawing_id.is_some() {
                            this.selected_drawing_id = None;
                        }
                        cx.notify();
                    }
                    "backspace" | "delete" => {
                        this.delete_selected_drawing();
                        cx.notify();
                    }
                    "i" => {
                        this.show_indicator_library = !this.show_indicator_library;
                        cx.notify();
                    }
                    "g" => {
                        this.show_horizontal_grid = !this.show_horizontal_grid;
                        this.mark_dirty();
                        cx.notify();
                    }
                    "l" => {
                        let total = this.feed.len();
                        this.chart_state.scroll_to_latest(total);
                        this.chart_state.pinned_to_latest = true;
                        cx.notify();
                    }
                    "f" => {
                        let total = this.feed.len();
                        this.chart_state.reset_view(total);
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .overflow_hidden()
            .child(chart)
            .child(drawing_overlay)
            .child(legend_ui)
            .child(legend_vol_ui)
            .children(subpane_legends)
            .child(gpui::deferred(settings_panel))
            .child(gpui::deferred(self.render_indicator_library(cx)));

        let root = v_flex()
            .id("chart_panel_root")
            .size_full()
            .bg(cx.theme().colors.background)
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    if this.chart_state.crosshair_pos.is_some() || this.drawing_cursor_point.is_some() {
                        this.chart_state.crosshair_pos = None;
                        this.drawing_cursor_point = None;
                        cx.notify();
                    }
                }
            }))
            .child(self.render_chart_toolbar(cx));

        root.child(chart_area).into_any_element()
    }
}

// ---------------------------------------------------------------------------
// Toolbar & Modals
// ---------------------------------------------------------------------------

impl ChartPanel {
    fn render_chart_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let w = if self.last_chart_width > 0.0 {
            self.last_chart_width
        } else {
            self.chart_state
                .candle_plot_size
                .get()
                .map(|(w, _)| w)
                .unwrap_or(800.0)
        };

        // Breakpoints for dynamic responsiveness
        let is_wide = w >= 780.0;
        let is_medium = w >= 520.0 && w < 780.0;
        let _is_compact = w >= 360.0 && w < 520.0;
        let is_ultra_compact = w < 360.0;
        let is_short = self.pane_height > 0.0 && self.pane_height < 140.0;
        let toolbar_h = if is_short { px(28.0) } else { px(34.0) };
        let icon = |path: &'static str| {
            gpui_component::Icon::default().path(path).size(px(15.0))
        };
        let active_bg = colors.primary.opacity(0.14);
        let active_fg = colors.primary;

        let toolbar_button = |id: &'static str, path: &'static str, label: &'static str| {
            Button::new(id)
                .icon(icon(path))
                .compact()
                .ghost()
                .tooltip(label)
        };

        let drawing_button = |tool: Option<DrawingTool>, id: &'static str, path: &'static str, label: &'static str| {
            let active = self.active_drawing_tool == tool;
            toolbar_button(id, path, label)
                .selected(active)
                .when(active, |button| button.bg(active_bg).text_color(active_fg))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.set_active_drawing_tool(tool);
                    cx.notify();
                }))
        };

        let selected_tool = self.active_drawing_tool;
        let drawing_view = cx.entity().downgrade();
        let more_drawings = toolbar_button(
            "toolbar_more_drawings",
            "icons/trending-up.svg",
            selected_tool.map(|tool| tool.label()).unwrap_or("Drawings"),
        )
        .selected(selected_tool.is_some())
        .when(selected_tool.is_some(), |button| button.bg(active_bg).text_color(active_fg))
        .tooltip(selected_tool.map(|tool| tool.label()).unwrap_or("Drawing tools"))
        .dropdown_caret(true)
        .dropdown_menu(move |mut menu, _, _| {
            for tool in std::iter::once(None).chain(DrawingTool::all().into_iter().map(Some)) {
                let view = drawing_view.clone();
                menu = menu.item(
                    PopupMenuItem::new(tool.map(|tool| tool.label()).unwrap_or("Pointer"))
                        .checked(selected_tool == tool)
                        .on_click(move |_, window, cx| {
                            let view = view.clone();
                            window.defer(cx, move |window, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.set_active_drawing_tool(tool);
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                });
                            });
                        }),
                );
            }
            menu
        });

        let separator = || div().w(px(1.0)).h(px(18.0)).bg(colors.border);

        let indicators = toolbar_button("toolbar_btn_indicators", "icons/chart-pie.svg", "Indicators")
            .selected(self.show_indicator_library)
            .when(self.show_indicator_library, |button| button.bg(active_bg).text_color(active_fg))
            .tooltip("Indicators (I)")
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.show_indicator_library = !this.show_indicator_library;
                cx.notify();
            }));

        let current_hex = self.drawing_color.clone();
        let current_hsv = self.drawing_color_hsv();
        let parsed_color = Self::parse_hex_color(&current_hex)
            .unwrap_or(gpui::rgba(0xc4e855ff).into());
        let color_view = cx.entity().downgrade();

        let color_trigger = Button::new("toolbar_drawing_color")
            .icon(icon("icons/palette.svg"))
            .compact()
            .ghost()
            .tooltip(format!("Drawing color ({})", self.drawing_color))
            .child(
                div()
                    .w(px(10.0))
                    .h(px(10.0))
                    .bg(parsed_color)
                    .border_1()
                    .border_color(colors.foreground.opacity(0.7)),
            );

        let color_picker = Popover::new(ElementId::Name("toolbar-color-popover".into()))
            .anchor(Corner::TopLeft)
            .trigger(color_trigger)
            .content(move |_state, window, cx| {
                let view = color_view.clone();
                let selected_hex = current_hex.clone();
                let theme = &cx.theme().colors;
                let selected_color = ChartPanel::parse_hex_color(&selected_hex)
                    .unwrap_or(gpui::rgba(0xc4e855ff).into());
                let (selected_hue, selected_saturation, selected_value) = current_hsv;
                let square_marker_x = selected_saturation * 204.0;
                let square_marker_y = (1.0 - selected_value) * 154.0;
                let hue_marker_x = selected_hue * 204.0;
                let square_drag_view = view.clone();
                let hue_drag_view = view.clone();
                let palettes: &[(&str, &[&str])] = &[
                    ("Quick picks", &["#C4E855", "#00F0FF", "#FFD600", "#FF0055", "#FFFFFF"]),
                    ("Greens", &["#00E676", "#2EBD85", "#C4E855", "#AEEA00", "#76FF03"]),
                    ("Blues & violet", &["#00F0FF", "#00B0FF", "#2979FF", "#651FFF", "#3D5AFE"]),
                    ("Warm accents", &["#FFEA00", "#FFD600", "#FF9100", "#FF6D00", "#FF3D00"]),
                    ("Pink & red", &["#FF0055", "#F6465D", "#FF1744", "#F50057", "#D500F9"]),
                    ("Neutral", &["#FFFFFF", "#ECEFF1", "#90A4AE", "#546E7A", "#263238"]),
                ];

                div()
                    .w(px(238.0))
                    .max_h((window.viewport_size().height - px(64.0)).max(px(80.0)))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .text_xs()
                    .on_mouse_move({
                        let view = view.clone();
                        move |event: &MouseMoveEvent, _window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if let Some(target) = this.color_picker_drag {
                                    cx.stop_propagation();
                                    this.update_color_picker(event.position, target);
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .on_mouse_up(MouseButton::Left, {
                        let view = view.clone();
                        move |_event, _window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                if this.color_picker_drag.is_some() {
                                    cx.stop_propagation();
                                    this.color_picker_drag = None;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .pb(px(10.0))
                            .border_b_1()
                            .border_color(theme.border.opacity(0.55))
                            .child(
                                div()
                                    .w(px(38.0))
                                    .h(px(38.0))
                                    .flex_shrink_0()
                                    .bg(selected_color)
                                    .border_1()
                                    .border_color(theme.foreground.opacity(0.45)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme.foreground)
                                            .child("Drawing color"),
                                    )
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.primary)
                                            .child(selected_hex.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme.muted_foreground)
                                            .child("Used for new drawings"),
                                    ),
                            ),
                    )
                    .child({
                        let hue_color = ChartPanel::hsv_color(selected_hue, 1.0, 1.0);
                        let marker_x = square_marker_x - 6.0;
                        let marker_y = square_marker_y - 6.0;
                        let field_view = view.clone();
                        div()
                            .id("drawing-color-square")
                            .relative()
                            .w(px(204.0))
                            .h(px(154.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.border)
                            .cursor_crosshair()
                            .on_mouse_down(MouseButton::Left, {
                                let view = square_drag_view.clone();
                                move |event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let _ = view.update(cx, |this, cx| {
                                        this.color_picker_drag = Some(ColorPickerDrag::Square);
                                        this.update_color_picker(event.position, ColorPickerDrag::Square);
                                        cx.notify();
                                    });
                                }
                            })
                            .on_mouse_move({
                                let view = square_drag_view.clone();
                                move |event: &MouseMoveEvent, _window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if this.color_picker_drag == Some(ColorPickerDrag::Square) {
                                            cx.stop_propagation();
                                            this.update_color_picker(event.position, ColorPickerDrag::Square);
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .on_mouse_up(MouseButton::Left, {
                                let view = square_drag_view.clone();
                                move |_event, _window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if this.color_picker_drag == Some(ColorPickerDrag::Square) {
                                            cx.stop_propagation();
                                            this.color_picker_drag = None;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .child(
                                gpui::canvas(
                                    move |bounds, _, cx| {
                                        let _ = field_view.update(cx, |this, _| {
                                            this.color_picker_square_bounds = bounds;
                                        });
                                    },
                                    move |bounds, _, window, _cx| {
                                        const COLUMNS: usize = 34;
                                        const ROWS: usize = 26;
                                        for row in 0..ROWS {
                                            for column in 0..COLUMNS {
                                                let saturation = column as f32 / (COLUMNS - 1) as f32;
                                                let value = 1.0 - row as f32 / (ROWS - 1) as f32;
                                                let color = ChartPanel::hsv_color(selected_hue, saturation, value);
                                                let x0 = bounds.origin.x + bounds.size.width * (column as f32 / COLUMNS as f32);
                                                let y0 = bounds.origin.y + bounds.size.height * (row as f32 / ROWS as f32);
                                                let x1 = bounds.origin.x + bounds.size.width * ((column + 1) as f32 / COLUMNS as f32);
                                                let y1 = bounds.origin.y + bounds.size.height * ((row + 1) as f32 / ROWS as f32);
                                                window.paint_quad(gpui::fill(
                                                    gpui::Bounds::from_corners(
                                                        gpui::point(x0, y0),
                                                        gpui::point(x1, y1),
                                                    ),
                                                    color,
                                                ));
                                            }
                                        }
                                    },
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(marker_x))
                                    .top(px(marker_y))
                                    .w(px(12.0))
                                    .h(px(12.0))
                                    .border_2()
                                    .border_color(gpui::Hsla::white())
                                    .bg(hue_color.opacity(0.2))
                                    .shadow_sm(),
                            )
                    })
                    .child({
                        let hue_view = view.clone();
                        div()
                            .id("drawing-color-hue")
                            .relative()
                            .w(px(204.0))
                            .h(px(16.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.border)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let view = hue_drag_view.clone();
                                move |event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let _ = view.update(cx, |this, cx| {
                                        this.color_picker_drag = Some(ColorPickerDrag::Hue);
                                        this.update_color_picker(event.position, ColorPickerDrag::Hue);
                                        cx.notify();
                                    });
                                }
                            })
                            .on_mouse_move({
                                let view = hue_drag_view.clone();
                                move |event: &MouseMoveEvent, _window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if this.color_picker_drag == Some(ColorPickerDrag::Hue) {
                                            cx.stop_propagation();
                                            this.update_color_picker(event.position, ColorPickerDrag::Hue);
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .on_mouse_up(MouseButton::Left, {
                                let view = hue_drag_view.clone();
                                move |_event, _window, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        if this.color_picker_drag == Some(ColorPickerDrag::Hue) {
                                            cx.stop_propagation();
                                            this.color_picker_drag = None;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .child(
                                gpui::canvas(
                                    move |bounds, _, cx| {
                                        let _ = hue_view.update(cx, |this, _| {
                                            this.color_picker_hue_bounds = bounds;
                                        });
                                    },
                                    move |bounds, _, window, _cx| {
                                        const STEPS: usize = 36;
                                        for step in 0..STEPS {
                                            let hue = step as f32 / STEPS as f32;
                                            let x0 = bounds.origin.x + bounds.size.width * hue;
                                            let x1 = bounds.origin.x + bounds.size.width * ((step + 1) as f32 / STEPS as f32);
                                            window.paint_quad(gpui::fill(
                                                gpui::Bounds::from_corners(
                                                    gpui::point(x0, bounds.origin.y),
                                                    gpui::point(x1, bounds.origin.y + bounds.size.height),
                                                ),
                                                ChartPanel::hsv_color(hue, 1.0, 1.0),
                                            ));
                                        }
                                    },
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(hue_marker_x - 2.0))
                                    .top(px(1.0))
                                    .w(px(4.0))
                                    .h(px(14.0))
                                    .border_1()
                                    .border_color(gpui::Hsla::white())
                                    .bg(gpui::Hsla::transparent_black()),
                            )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .pt(px(2.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.muted_foreground)
                                    .child("Quick picks"),
                            )
                            .children(palettes[0].1.iter().enumerate().map(|(index, &hex)| {
                                let view = view.clone();
                                let hex_str = hex.to_string();
                                let is_active = selected_hex.eq_ignore_ascii_case(hex);
                                let swatch_color = ChartPanel::parse_hex_color(hex)
                                    .unwrap_or(gpui::rgba(0xffffffff).into());
                                div()
                                    .id(ElementId::Name(
                                        format!("drawing-color-quick-{}", index).into(),
                                    ))
                                    .cursor_pointer()
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .bg(swatch_color)
                                    .border_1()
                                    .border_color(if is_active {
                                        theme.foreground
                                    } else {
                                        theme.border.opacity(0.65)
                                    })
                                    .when(is_active, |this| this.border_2())
                                    .child(
                                        div()
                                            .w_full()
                                            .h_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(is_active, |this| {
                                                this.child(
                                                    gpui_component::Icon::default()
                                                        .path("icons/check.svg")
                                                        .size(px(12.0))
                                                        .text_color(if hex == "#FFFFFF" {
                                                            gpui::Hsla::from(gpui::rgba(0x263238ff))
                                                        } else {
                                                            gpui::Hsla::from(gpui::rgba(0xffffffff))
                                                        }),
                                                )
                                            }),
                                    )
                                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                        let view = view.clone();
                                        let hex_str = hex_str.clone();
                                        window.defer(cx, move |window, cx| {
                                            let _ = view.update(cx, |this, cx| {
                                                this.drawing_color = hex_str;
                                                this.color_picker_drag = None;
                                                window.focus(&this.focus_handle);
                                                cx.notify();
                                            });
                                        });
                                    })
                            })),
                    )
                    .into_any_element()
            });

        let snap = toolbar_button("toolbar_snap", "icons/magnet.svg", "Snap")
            .selected(self.snap_to_candle)
            .when(self.snap_to_candle, |button| button.bg(active_bg).text_color(active_fg))
            .tooltip("Snap drawings to candles")
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.snap_to_candle = !this.snap_to_candle;
                cx.notify();
            }));

        let reset = toolbar_button("toolbar_reset_view", "icons/undo-2.svg", "Reset")
            .tooltip("Reset view (F)")
            .on_click(cx.listener(|this, _event, _window, cx| {
                let total = this.feed.len();
                this.chart_state.reset_view(total);
                cx.notify();
            }));

        let grid = toolbar_button("toolbar_grid", "icons/crosshair.svg", "Grid")
            .selected(self.show_horizontal_grid)
            .when(self.show_horizontal_grid, |button| button.bg(colors.muted.opacity(0.3)).text_color(colors.foreground))
            .tooltip("Horizontal grid (G)")
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.show_horizontal_grid = !this.show_horizontal_grid;
                this.mark_dirty();
                cx.notify();
            }));

        let clear = toolbar_button("toolbar_clear_drawings", "icons/delete.svg", "Clear")
            .tooltip("Clear all drawings")
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.clear_all_drawings();
                cx.notify();
            }));

        div()
            .h(toolbar_h)
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .overflow_hidden()
            .gap(px(2.0))
            .px(if is_wide { px(8.0) } else { px(4.0) })
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.background)
            .text_xs()
            .on_mouse_move(cx.listener(|this, _ev: &MouseMoveEvent, _win, cx| {
                if this.chart_state.crosshair_pos.is_some() || this.drawing_cursor_point.is_some() {
                    this.chart_state.crosshair_pos = None;
                    this.drawing_cursor_point = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .flex_shrink_0()
                    .child(indicators)
                    .when(!is_ultra_compact, |this| this.child(separator()))
                    .child(drawing_button(None, "toolbar_pointer", "icons/mouse-pointer.svg", "Pointer"))
                    .when(!is_ultra_compact, |this| {
                        this.child(drawing_button(Some(DrawingTool::Trendline), "toolbar_trendline", "icons/trending-up.svg", "Trend"))
                    })
                    .when(is_wide || is_medium, |this| {
                        this.child(drawing_button(Some(DrawingTool::HorizontalLine), "toolbar_horizontal", "icons/minus.svg", "Horizontal"))
                    })
                    .when(is_wide, |this| {
                        this.child(drawing_button(Some(DrawingTool::VerticalLine), "toolbar_vertical", "icons/arrow-up.svg", "Vertical"))
                            .child(drawing_button(Some(DrawingTool::Ray), "toolbar_ray", "icons/arrow-right.svg", "Ray"))
                            .child(drawing_button(Some(DrawingTool::Fibonacci), "toolbar_fibonacci", "icons/sort-ascending.svg", "Fib"))
                            .child(drawing_button(Some(DrawingTool::PriceChannel), "toolbar_channel", "icons/frame.svg", "Channel"))
                            .child(drawing_button(Some(DrawingTool::Text), "toolbar_text", "icons/a-large-small.svg", "Text"))
                    })
                    .when(!is_wide, |this| {
                        this.child(more_drawings)
                    })
                    .when(is_wide || is_medium, |this| {
                        this.child(drawing_button(Some(DrawingTool::LongPosition), "toolbar_long", "icons/arrow-up.svg", "Long"))
                            .child(drawing_button(Some(DrawingTool::ShortPosition), "toolbar_short", "icons/arrow-down.svg", "Short"))
                    })
                    .when(!is_ultra_compact, |this| this.child(separator()))
                    .child(color_picker)
                    .when(!is_ultra_compact, |this| this.child(snap))
                    .when(!is_ultra_compact, |this| this.child(separator()))
                    .child(reset)
                    .when(is_wide || is_medium, |this| this.child(grid))
                    .when(!is_ultra_compact || w >= 260.0, |this| this.child(clear)),
            )
    }

    fn render_indicator_library(&self, cx: &Context<Self>) -> AnyElement {
        if !self.show_indicator_library {
            return div().into_any_element();
        }

        let colors = &cx.theme().colors;
        let categories = [
            (None, "All"),
            (Some(IndicatorCategory::Trend), "Trend"),
            (Some(IndicatorCategory::Momentum), "Momentum"),
            (Some(IndicatorCategory::Volatility), "Volatility"),
            (Some(IndicatorCategory::Volume), "Volume"),
        ];

        let available_kinds: Vec<IndicatorKind> = IndicatorKind::all()
            .into_iter()
            .filter(|k| {
                if let Some(cat) = self.indicator_library_category {
                    if k.category() != cat {
                        return false;
                    }
                }
                if !self.indicator_library_search.is_empty() {
                    let q = self.indicator_library_search.to_lowercase();
                    return k.label().to_lowercase().contains(&q)
                        || k.category().label().to_lowercase().contains(&q);
                }
                true
            })
            .collect();

        div()
            .absolute()
            .top(px(self.library_y))
            .left(px(self.library_x))
            .w(px(self.library_w))
            .h(px(self.library_h))
            .border_1()
            .border_color(colors.border)
            .bg(colors.background)
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
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
            // Draggable Modal Header
            .child(
                div()
                    .h(px(36.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_grab()
                    .bg(colors.muted.opacity(0.15))
                    .border_b_1()
                    .border_color(colors.border)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, ev: &MouseDownEvent, _win, cx| {
                            cx.stop_propagation();
                            this.library_dragging = true;
                            this.library_drag_start_x = ev.position.x.as_f32();
                            this.library_drag_start_y = ev.position.y.as_f32();
                            this.library_drag_origin_x = this.library_x;
                            this.library_drag_origin_y = this.library_y;
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child("INDICATORS & STRATEGIES"),
                    )
                    .child(
                        div()
                            .id("indicator_modal_close_btn")
                            .cursor_pointer()
                            .px(px(6.0))
                            .py(px(2.0))
                            .hover(|s| s.bg(colors.muted.opacity(0.2)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _win, cx| {
                                    cx.stop_propagation();
                                    this.show_indicator_library = false;
                                    cx.notify();
                                }),
                            )
                            .child("✕"),
                    ),
            )
            // Category Tabs
            .child(
                div()
                    .h(px(32.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .children(categories.into_iter().map(|(cat, label)| {
                        let is_active = self.indicator_library_category == cat;
                        div()
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(2.0))
                            .bg(if is_active {
                                colors.primary.opacity(0.14)
                            } else {
                                gpui::transparent_black()
                            })
                            .text_color(if is_active {
                                colors.primary
                            } else {
                                colors.muted_foreground
                            })
                            .hover(|s| {
                                s.bg(colors.muted.opacity(0.2))
                                    .text_color(colors.foreground)
                            })
                            .font_weight(if is_active {
                                gpui::FontWeight::BOLD
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev, _win, cx| {
                                    cx.stop_propagation();
                                    this.indicator_library_category = cat;
                                    cx.notify();
                                }),
                            )
                            .child(label)
                    })),
            )
            // Content Body
            .child(
                div()
                    .id("library-scroll")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .p(px(12.0))
                    .gap(px(10.0))
                    .overflow_y_scroll()
                    // Active on Chart section
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(format!(
                                        "ACTIVE ON CHART ({})",
                                        self.indicators.len()
                                    )),
                            )
                            .children(self.indicators.iter().map(|ind| {
                                let ind_id = ind.id.clone();
                                let is_editing = self.editing_indicator_id.as_deref() == Some(&ind_id);

                                div()
                                    .flex()
                                    .flex_col()
                                    .p(px(6.0))
                                    .bg(colors.muted.opacity(0.15))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .child(ind.display_label()),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.0))
                                                    // Toggle Visibility
                                                    .child(
                                                        div()
                                                            .cursor_pointer()
                                                            .text_color(colors.muted_foreground)
                                                            .hover(|s| s.text_color(colors.foreground))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener({
                                                                    let id = ind_id.clone();
                                                                    move |this, _ev, _win, cx| {
                                                                        cx.stop_propagation();
                                                                        this.toggle_indicator_instance(&id);
                                                                        cx.notify();
                                                                    }
                                                                }),
                                                            )
                                                            .child(if ind.visible { "👁" } else { "👁‍🗨" }),
                                                    )
                                                    // Settings Expander
                                                    .child(
                                                        div()
                                                            .cursor_pointer()
                                                            .text_color(colors.muted_foreground)
                                                            .hover(|s| s.text_color(colors.foreground))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener({
                                                                    let id = ind_id.clone();
                                                                    move |this, _ev, _win, cx| {
                                                                        cx.stop_propagation();
                                                                        if this.editing_indicator_id.as_deref() == Some(&id) {
                                                                            this.editing_indicator_id = None;
                                                                        } else {
                                                                            this.editing_indicator_id = Some(id.clone());
                                                                            if let Some(target) = this.indicators.iter().find(|i| i.id == id) {
                                                                                this.editing_param_inputs = target.params.iter().map(|(k, v)| (k.clone(), v.to_string())).collect();
                                                                            }
                                                                        }
                                                                        cx.notify();
                                                                    }
                                                                }),
                                                            )
                                                            .child("⚙"),
                                                    )
                                                    // Remove Instance
                                                    .child(
                                                        div()
                                                            .cursor_pointer()
                                                            .text_color(colors.bearish)
                                                            .hover(|s| s.text_color(colors.bearish.opacity(0.8)))
                                                            .on_mouse_down(
                                                                MouseButton::Left,
                                                                cx.listener({
                                                                    let id = ind_id.clone();
                                                                    move |this, _ev, _win, cx| {
                                                                        cx.stop_propagation();
                                                                        this.remove_indicator_instance(&id);
                                                                        cx.notify();
                                                                    }
                                                                }),
                                                            )
                                                            .child("✕"),
                                                    ),
                                            ),
                                    )
                                    .children(if is_editing {
                                        Some(
                                            div()
                                                .pt(px(6.0))
                                                .border_t_1()
                                                .border_color(colors.border)
                                                .flex()
                                                .flex_col()
                                                .gap(px(4.0))
                                                .text_xs()
                                                .children(ind.params.iter().map(|(p_name, _val)| {
                                                    let val_str = self.editing_param_inputs.get(p_name).cloned().unwrap_or_default();
                                                    let param_key = p_name.clone();
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(div().child(p_name.clone()))
                                                        .child(
                                                            div()
                                                                .px(px(6.0))
                                                                .py(px(2.0))
                                                                .bg(colors.muted.opacity(0.3))
                                                                .child(format!("{}: {}", param_key, val_str))
                                                        )
                                                }))
                                                .children(self.editing_param_error.as_ref().map(|err| {
                                                    div().text_color(colors.bearish).child(err.clone())
                                                }))
                                        )
                                    } else {
                                        None
                                    })
                            })),
                    )
                    // Available Library section
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(format!(
                                        "AVAILABLE INDICATORS ({})",
                                        available_kinds.len()
                                    )),
                            )
                            .children(available_kinds.into_iter().map(|kind| {
                                let label = SharedString::from(kind.label().to_string());
                                let cat_label = kind.category().label();
                                let k = kind.clone();
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .p(px(6.0))
                                    .bg(colors.muted.opacity(0.08))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .child(label),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    .bg(colors.muted.opacity(0.2))
                                                    .text_color(colors.muted_foreground)
                                                    .child(cat_label),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .cursor_pointer()
                                            .px(px(8.0))
                                            .py(px(2.0))
                                            .bg(colors.primary.opacity(0.2))
                                            .text_color(colors.primary)
                                            .hover(|s| {
                                                s.bg(colors.primary.opacity(0.3))
                                                    .text_color(colors.foreground)
                                            })
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _ev, _win, cx| {
                                                    cx.stop_propagation();
                                                    this.add_indicator_instance(k.clone());
                                                    cx.notify();
                                                }),
                                            )
                                            .child("+ Add"),
                                    )
                            })),
                    ),
            )
            // Footer resize handle
            .child(
                div()
                    .h(px(16.0))
                    .w_full()
                    .flex()
                    .justify_end()
                    .items_center()
                    .pr(px(4.0))
                    .cursor_nwse_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev: &MouseDownEvent, _win, cx| {
                            cx.stop_propagation();
                            this.library_resizing = true;
                            cx.notify();
                        }),
                    )
                    .child(div().w(px(8.0)).h(px(8.0)).bg(colors.muted_foreground)),
            )
            .into_any_element()
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
        d.points.push(DrawingPoint { timestamp: 100, price: 50.0 });
        d.points.push(DrawingPoint { timestamp: 200, price: 60.0 });
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
    fn test_color_picker_hex_parsing() {
        let colors = [
            "#C4E855", "#00F0FF", "#FFD600", "#FF0055", "#FFFFFF",
            "#00E676", "#2EBD85", "#2979FF", "#FF3D00", "#ECEFF1",
        ];
        for hex in colors {
            assert!(ChartPanel::parse_hex_color(hex).is_some());
        }
    }

    #[test]
    fn test_new_chart_has_no_default_indicators() {
        let layout = ChartLayout::new("binance", "BTC/USDT", "1m");
        assert!(layout.indicators.is_empty());
    }
}
