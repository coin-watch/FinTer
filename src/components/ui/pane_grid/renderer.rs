use crate::data::pane_tree::{Axis, PaneNode};
use gpui::InteractiveElement;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui::StyledImage;
use gpui_component::{Selectable, Sizable};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::theme::ActiveTheme;
use gpui_component::popover::Popover;
use gpui_component::menu::{DropdownMenu, GroupListPopup};

use crate::components::ui::pane_grid::drag_view::{DragView, DragContent};






pub struct ChartViewState {
    pub exchange: String,
    pub symbol: String,
    pub timeframe: String,
    pub chart_type: String,
    pub entity: Entity<crate::components::ui::chart_panel::ChartPanel>,
    pub _bridge: std::sync::Arc<crate::data::bridge::Bridge>,
}

pub struct TickerViewState {
    pub exchange: String,
    pub symbol: String,
    pub entity: Entity<crate::components::ui::ticker_panel::TickerPanel>,
}


pub struct OrderBookViewState {
    pub exchange: String,
    pub symbol: String,
    pub limit: usize,
    pub entity: Entity<crate::components::ui::orderbook_panel::OrderBookPanel>,
    pub _bridge: std::sync::Arc<crate::data::bridge::Bridge>,
}

pub struct AgentViewState {
    pub entity: Entity<crate::components::ui::agent_panel::AgentPanel>,
}

pub struct TapeViewState {
    pub exchange: String,
    pub symbol: String,
    pub entity: Entity<crate::components::ui::tape_panel::TapePanel>,
    pub _bridge: std::sync::Arc<crate::data::bridge::Bridge>,
}

pub struct ExecutionViewState {
    pub exchange: String,
    pub symbol: String,
    pub entity: Entity<crate::components::ui::execution_panel::ExecutionPanel>,
    pub _bridge: std::sync::Arc<crate::data::bridge::Bridge>,
}

pub struct WatchlistViewState {
    pub entity: Entity<crate::components::ui::watchlist_panel::WatchlistPanel>,
}

pub struct MarketCapViewState {
    pub entity: Entity<crate::components::ui::market_cap_panel::MarketCapPanel>,
}

pub struct NewsfeedViewState {
    pub priority: String,
    pub filter: String,
    pub entity: Entity<crate::components::ui::newsfeed_panel::NewsfeedPanel>,
}

pub struct ClockViewState {
    pub timezone: String,
    pub entity: Entity<crate::components::ui::clock_panel::ClockPanel>,
}

pub struct PaneGridRenderer {
    pub tree: Option<PaneNode>,
    pub active_pane_id: Option<String>,
    pub resizing: Option<ResizingState>,
    pub dragged_pane: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    pub maximized_pane: Option<String>,
    pub matrix_hover: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, (usize, usize)>>>,
    pub chart_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, ChartViewState>>>,
    pub ticker_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, TickerViewState>>>,
    pub orderbook_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, OrderBookViewState>>>,
    pub agent_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, AgentViewState>>>,
    pub tape_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, TapeViewState>>>,
    pub execution_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, ExecutionViewState>>>,
    pub positions_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, gpui::Entity<crate::components::ui::positions_panel::PositionsPanel>>>>,
    pub watchlist_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, WatchlistViewState>>>,
    pub market_cap_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, MarketCapViewState>>>,
    pub newsfeed_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, NewsfeedViewState>>>,
    pub clock_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, ClockViewState>>>,
    pub fear_greed_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, FearGreedViewState>>>,
    /// Latest cross-dashboard snapshot, retained so Agent panes created after refresh receive it.
    agent_context: Option<serde_json::Value>,
    pub timezone: String,
    /// Global setting; individual panels cannot bypass it.
    pub sound_alerts_enabled: bool,
}

pub struct FearGreedViewState {
    pub entity: gpui::Entity<crate::components::ui::fear_greed_panel::FearGreedPanel>,
}


#[derive(Clone, Debug)]
pub struct ResizingState {
    pub split_id: String,
    pub axis: Axis,
    pub initial_ratio: f32,
    pub initial_mouse_pos: gpui::Point<Pixels>,
    pub container_size: f32,
}


impl gpui::EventEmitter<PaneGridEvent> for gpui_component::popover::PopoverState {}

/// Cache only module contents, not the dashboard shell. GPUI invalidates this
/// cache on entity/descendant notifications, bounds changes and window refreshes.
/// A full-size layout keeps each module constrained to its allocated pane.
fn cached_panel<V: Render>(panel: Entity<V>) -> AnyElement {
    AnyView::from(panel)
        .cached(StyleRefinement::default().size_full())
        .into_any_element()
}

fn config_str(config: Option<&serde_json::Map<String, serde_json::Value>>, key: &str, default: &str) -> String {
    config
        .and_then(|values| values.get(key))
        .and_then(|value| value.as_str())
        .unwrap_or(default)
        .to_string()
}

fn config_usize(config: Option<&serde_json::Map<String, serde_json::Value>>, key: &str, default: usize) -> usize {
    config
        .and_then(|values| values.get(key))
        .and_then(|value| value.as_u64())
        .map(|value| value as usize)
        .unwrap_or(default)
}

fn config_bool(config: Option<&serde_json::Map<String, serde_json::Value>>, key: &str, default: bool) -> bool {
    config
        .and_then(|values| values.get(key))
        .and_then(|value| value.as_bool())
        .unwrap_or(default)
}

fn update_pane_config(pane_id: String, key: &'static str, value: serde_json::Value, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |_window, cx| {
        let mut config = serde_json::Map::new();
        config.insert(key.to_string(), value);
        cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
            let _ = global.view.update(cx, |view, cx| {
                view.handle_pane_event(
                    &PaneGridEvent::UpdateModuleConfig {
                        pane_id,
                        config: serde_json::Value::Object(config),
                    },
                    cx,
                )
            });
        });
    });
}

fn update_exchange_config(pane_id: String, exchange: String, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |_window, cx| {
        let symbol = crate::data::gateway_registry::GatewayRegistry::global()
            .symbols(&exchange)
            .into_iter()
            .next()
            .unwrap_or_else(|| "BTC/USDT".to_string());
        let mut config = serde_json::Map::new();
        config.insert("exchange".to_string(), serde_json::Value::String(exchange));
        config.insert("symbol".to_string(), serde_json::Value::String(symbol));
        config.insert("market".to_string(), serde_json::Value::String("spot".to_string()));
        cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
            let _ = global.view.update(cx, |view, cx| {
                view.handle_pane_event(
                    &PaneGridEvent::UpdateModuleConfig {
                        pane_id,
                        config: serde_json::Value::Object(config),
                    },
                    cx,
                )
            });
        });
    });
}

const CANDLE_TYPES: [(&str, &str); 7] = [
    ("candle_solid", "Candle Solid"),
    ("candle_stroke", "Candle Stroke"),
    ("candle_up_stroke", "Candle Up Stroke"),
    ("candle_down_stroke", "Candle Down Stroke"),
    ("heikin_ashi", "Heikin-Ashi"),
    ("ohlc", "OHLC"),
    ("area", "Area"),
];

fn timezone_choices() -> &'static [(&'static str, &'static str)] {
    &[
        // UTC
        ("UTC", "UTC"),
        ("GMT", "GMT"),
        // Americas
        ("America/New_York", "New York, United States"),
        ("America/Chicago", "Chicago, United States"),
        ("America/Denver", "Denver, United States"),
        ("America/Los_Angeles", "Los Angeles, United States"),
        ("America/Toronto", "Toronto, Canada"),
        ("America/Vancouver", "Vancouver, Canada"),
        ("America/Sao_Paulo", "São Paulo, Brazil"),
        ("America/Mexico_City", "Mexico City, Mexico"),
        ("America/Buenos_Aires", "Buenos Aires, Argentina"),
        // Europe
        ("Europe/London", "London, United Kingdom"),
        ("Europe/Paris", "Paris, France"),
        ("Europe/Berlin", "Berlin, Germany"),
        ("Europe/Rome", "Rome, Italy"),
        ("Europe/Madrid", "Madrid, Spain"),
        ("Europe/Amsterdam", "Amsterdam, Netherlands"),
        ("Europe/Zurich", "Zurich, Switzerland"),
        ("Europe/Vienna", "Vienna, Austria"),
        ("Europe/Stockholm", "Stockholm, Sweden"),
        ("Europe/Oslo", "Oslo, Norway"),
        ("Europe/Copenhagen", "Copenhagen, Denmark"),
        ("Europe/Helsinki", "Helsinki, Finland"),
        ("Europe/Warsaw", "Warsaw, Poland"),
        ("Europe/Prague", "Prague, Czech Republic"),
        ("Europe/Budapest", "Budapest, Hungary"),
        ("Europe/Athens", "Athens, Greece"),
        ("Europe/Istanbul", "Istanbul, Turkey"),
        ("Europe/Moscow", "Moscow, Russia"),
        // Asia
        ("Asia/Tokyo", "Tokyo, Japan"),
        ("Asia/Shanghai", "Shanghai, China"),
        ("Asia/Hong_Kong", "Hong Kong"),
        ("Asia/Singapore", "Singapore"),
        ("Asia/Seoul", "Seoul, South Korea"),
        ("Asia/Taipei", "Taipei, Taiwan"),
        ("Asia/Bangkok", "Bangkok, Thailand"),
        ("Asia/Jakarta", "Jakarta, Indonesia"),
        ("Asia/Kuala_Lumpur", "Kuala Lumpur, Malaysia"),
        ("Asia/Manila", "Manila, Philippines"),
        ("Asia/Ho_Chi_Minh", "Ho Chi Minh City, Vietnam"),
        ("Asia/Mumbai", "Mumbai, India"),
        ("Asia/Kolkata", "Kolkata, India"),
        ("Asia/Dubai", "Dubai, United Arab Emirates"),
        ("Asia/Tehran", "Tehran, Iran"),
        ("Asia/Karachi", "Karachi, Pakistan"),
        ("Asia/Dhaka", "Dhaka, Bangladesh"),
        // Australia & Pacific
        ("Australia/Sydney", "Sydney, Australia"),
        ("Australia/Melbourne", "Melbourne, Australia"),
        ("Australia/Perth", "Perth, Australia"),
        ("Australia/Brisbane", "Brisbane, Australia"),
        ("Australia/Adelaide", "Adelaide, Australia"),
        ("Pacific/Auckland", "Auckland, New Zealand"),
        ("Pacific/Fiji", "Fiji"),
        ("Pacific/Honolulu", "Honolulu, United States"),
        // Africa
        ("Africa/Cairo", "Cairo, Egypt"),
        ("Africa/Johannesburg", "Johannesburg, South Africa"),
        ("Africa/Lagos", "Lagos, Nigeria"),
        ("Africa/Nairobi", "Nairobi, Kenya"),
        ("Africa/Casablanca", "Casablanca, Morocco"),
        ("Africa/Tunis", "Tunis, Tunisia"),
        ("Africa/Algiers", "Algiers, Algeria"),
    ]
}

fn exchange_choices() -> Vec<String> {
    let reg_exchanges = crate::data::gateway_registry::GatewayRegistry::global().exchanges();
    if !reg_exchanges.is_empty() {
        return reg_exchanges;
    }
    vec![
        "binance", "bybit", "okx", "kraken", "coinbase", "kucoin", "gateio",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn symbol_choices(exchange: &str) -> Vec<String> {
    let reg_symbols = crate::data::gateway_registry::GatewayRegistry::global().symbols(exchange);
    if !reg_symbols.is_empty() {
        return reg_symbols;
    }
    vec![
        "BTC/USDT", "ETH/USDT", "SOL/USDT", "BNB/USDT", "XRP/USDT", "DOGE/USDT", "ADA/USDT",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn symbol_choices_for_menu(exchange: &str, _current_symbol: &str) -> Vec<String> {
    symbol_choices(exchange)
}

fn candle_type_from_config(value: &str) -> gpui_component::chart::CandleType {
    match value {
        "candle_stroke" => gpui_component::chart::CandleType::Stroke,
        "candle_up_stroke" => gpui_component::chart::CandleType::UpStroke,
        "candle_down_stroke" => gpui_component::chart::CandleType::DownStroke,
        "ohlc" => gpui_component::chart::CandleType::Ohlc,
        "area" => gpui_component::chart::CandleType::Area,
        "heikin_ashi" => gpui_component::chart::CandleType::HeikinAshi,
        "candle_solid" => gpui_component::chart::CandleType::Solid,
        _ => gpui_component::chart::CandleType::Solid,
    }
}

fn canonical_exchange(exchange: &str) -> String {
    let ex = exchange.to_lowercase();
    if ex.contains("binance") {
        "binance".to_string()
    } else if ex.contains("huobi") || ex.contains("htx") {
        "htx".to_string()
    } else if ex.contains("okx") {
        "okx".to_string()
    } else if ex.contains("bybit") {
        "bybit".to_string()
    } else if ex.contains("kraken") {
        "kraken".to_string()
    } else if ex.contains("coinbase") {
        "coinbase".to_string()
    } else if ex.contains("kucoin") {
        "kucoin".to_string()
    } else if ex.contains("gateio") || ex.contains("gate") {
        "gateio".to_string()
    } else {
        ex
    }
}

fn exchange_icon_url(exchange: &str) -> String {
    let canonical = canonical_exchange(exchange);
    format!("https://raw.githubusercontent.com/0xa3k5/web3icons/main/packages/core/src/svgs/exchanges/background/{canonical}.svg")
}

fn parse_symbol_parts(symbol: &str) -> (String, Option<String>) {
    let cleaned = symbol.split(':').next().unwrap_or(symbol);
    let parts: Vec<&str> = cleaned.split(|c| c == '/' || c == '_' || c == '-').collect();
    if parts.len() >= 2 {
        (parts[0].to_lowercase(), Some(parts[1].to_lowercase()))
    } else {
        (cleaned.to_lowercase(), None)
    }
}

fn crypto_icon_url(asset: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/spothq/cryptocurrency-icons/master/svg/color/{}.svg",
        asset.to_lowercase()
    )
}

fn exchange_badge_color(exchange: &str) -> gpui::Hsla {
    match exchange.to_lowercase().as_str() {
        "binance" => gpui::Hsla::from(gpui::rgba(0xf3ba2fff)),
        "bybit" => gpui::Hsla::from(gpui::rgba(0xf7a600ff)),
        "okx" => gpui::Hsla::from(gpui::rgba(0x2d60ffff)),
        "kraken" => gpui::Hsla::from(gpui::rgba(0x5741d9ff)),
        "coinbase" => gpui::Hsla::from(gpui::rgba(0x0052ffff)),
        "kucoin" => gpui::Hsla::from(gpui::rgba(0x24ae8fff)),
        "gateio" | "gate" => gpui::Hsla::from(gpui::rgba(0x2354e6ff)),
        _ => gpui::Hsla::from(gpui::rgba(0x888888ff)),
    }
}

fn asset_badge_color(asset: &str) -> gpui::Hsla {
    match asset.to_uppercase().as_str() {
        "BTC" => gpui::Hsla::from(gpui::rgba(0xf7931aff)),
        "ETH" => gpui::Hsla::from(gpui::rgba(0x627eeaff)),
        "SOL" => gpui::Hsla::from(gpui::rgba(0x14f195ff)),
        "BNB" => gpui::Hsla::from(gpui::rgba(0xf3ba2fff)),
        "XRP" => gpui::Hsla::from(gpui::rgba(0x23292fff)),
        "DOGE" => gpui::Hsla::from(gpui::rgba(0xc2a633ff)),
        "ADA" => gpui::Hsla::from(gpui::rgba(0x0033adff)),
        "AVAX" => gpui::Hsla::from(gpui::rgba(0xe84142ff)),
        "DOT" => gpui::Hsla::from(gpui::rgba(0xe6007aff)),
        "LINK" => gpui::Hsla::from(gpui::rgba(0x375bd2ff)),
        "MATIC" | "POL" => gpui::Hsla::from(gpui::rgba(0x8247e5ff)),
        "NEAR" => gpui::Hsla::from(gpui::rgba(0x000000ff)),
        "SUI" => gpui::Hsla::from(gpui::rgba(0x4ca2ffff)),
        "APT" => gpui::Hsla::from(gpui::rgba(0x2ed8a7ff)),
        _ => gpui::Hsla::from(gpui::rgba(0x6366f1ff)),
    }
}

pub(crate) fn render_coinwatch_icons(pane_id: &str, exchange: &str, symbol: &str) -> impl IntoElement {
    let ex_url = exchange_icon_url(exchange);
    let ex_color = exchange_badge_color(exchange);
    let ex_initial = exchange.chars().next().unwrap_or('E').to_uppercase().to_string();

    let (base, quote_opt) = parse_symbol_parts(symbol);
    let base_url = crypto_icon_url(&base);
    let base_color = asset_badge_color(&base);
    let base_initial = base.chars().next().unwrap_or('•').to_uppercase().to_string();

    let make_badge = |bg: gpui::Hsla, text: String| {
        div()
            .size_full()
            .rounded_full()
            .bg(bg)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.0))
            .font_weight(FontWeight::BOLD)
            .text_color(gpui::white())
            .child(text)
            .into_any_element()
    };

    let ex_id = ElementId::Name(format!("ex-icon-{pane_id}").into());
    let ex_fallback_color = ex_color;
    let ex_fallback_text = ex_initial.clone();
    let ex_loading_color = ex_color;
    let ex_loading_text = ex_initial;

    let ex_disc = div()
        .absolute()
        .top_0()
        .left(px(0.0))
        .w(px(20.0))
        .h(px(20.0))
        .rounded_full()
        .overflow_hidden()
        .shadow_xs()
        .child(
            img(ex_url)
                .id(ex_id)
                .size_full()
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .with_loading(move || make_badge(ex_loading_color, ex_loading_text.clone()))
                .with_fallback(move || make_badge(ex_fallback_color, ex_fallback_text.clone())),
        );

    let base_id = ElementId::Name(format!("base-icon-{pane_id}").into());
    let base_fallback_color = base_color;
    let base_fallback_text = base_initial.clone();
    let base_loading_color = base_color;
    let base_loading_text = base_initial;

    let base_disc = div()
        .absolute()
        .top_0()
        .left(px(12.0))
        .w(px(20.0))
        .h(px(20.0))
        .rounded_full()
        .overflow_hidden()
        .shadow_xs()
        .child(
            img(base_url)
                .id(base_id)
                .size_full()
                .rounded_full()
                .object_fit(gpui::ObjectFit::Cover)
                .with_loading(move || make_badge(base_loading_color, base_loading_text.clone()))
                .with_fallback(move || make_badge(base_fallback_color, base_fallback_text.clone())),
        );

    let has_quote = quote_opt.is_some();
    let quote_disc = quote_opt.map(|quote| {
        let quote_url = crypto_icon_url(&quote);
        let quote_color = asset_badge_color(&quote);
        let quote_initial = quote.chars().next().unwrap_or('•').to_uppercase().to_string();

        let quote_id = ElementId::Name(format!("quote-icon-{pane_id}").into());
        let quote_fallback_color = quote_color;
        let quote_fallback_text = quote_initial.clone();
        let quote_loading_color = quote_color;
        let quote_loading_text = quote_initial;

        div()
            .absolute()
            .top_0()
            .left(px(24.0))
            .w(px(20.0))
            .h(px(20.0))
            .rounded_full()
            .overflow_hidden()
            .shadow_xs()
            .child(
                img(quote_url)
                    .id(quote_id)
                    .size_full()
                    .rounded_full()
                    .object_fit(gpui::ObjectFit::Cover)
                    .with_loading(move || make_badge(quote_loading_color, quote_loading_text.clone()))
                    .with_fallback(move || make_badge(quote_fallback_color, quote_fallback_text.clone())),
            )
    });

    let cluster_width = if has_quote { px(44.0) } else { px(32.0) };

    div()
        .w(cluster_width)
        .h(px(20.0))
        .flex_shrink_0()
        .relative()
        .child(ex_disc)
        .child(base_disc)
        .children(quote_disc)
}


use std::sync::mpsc::{Sender, Receiver};

pub struct PendingGridEvents {
    pub tx: Sender<PaneGridEvent>,
    pub rx: Receiver<PaneGridEvent>,
}
impl Default for PendingGridEvents {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx }
    }
}
impl gpui::Global for PendingGridEvents {}


#[derive(Clone)]
pub enum PaneGridEvent {
    UpdateModuleConfig {
        pane_id: String,
        config: serde_json::Value,
    },
    SetModuleType {
        pane_id: String,
        module_type: String,
    },
    ToggleMaximize {
        pane_id: String,
    },
    SplitPane {
        pane_id: String,
        axis: Axis,
    },
    SplitMatrix {
        pane_id: String,
        rows: usize,
        cols: usize,
    },
    ClosePane {
        pane_id: String,
    },
    SetActivePane {
        pane_id: String,
    },
    SwapPanes {
        id_a: String,
        id_b: String,
    },
    DropSplit {
        target_pane_id: String,
        dragged_pane_id: String,
        region: String,
    },
    ResizeSplit {
        split_id: String,
        ratio: f32,
    },
    EndResize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DragPane(pub String);

impl EventEmitter<PaneGridEvent> for PaneGridRenderer {}

impl PaneGridRenderer {
    pub fn new() -> Self {
        Self {
            tree: None,
            active_pane_id: None,
            resizing: None,
            dragged_pane: std::rc::Rc::new(std::cell::RefCell::new(None)),
            maximized_pane: None,
            matrix_hover: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            chart_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            ticker_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            orderbook_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            agent_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            tape_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            execution_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            positions_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            watchlist_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            market_cap_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            newsfeed_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            clock_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            fear_greed_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            agent_context: None,
            timezone: "UTC".into(),
            sound_alerts_enabled: true,
        }
    }

    /// Bounded serializable market snapshots from every loaded module in this dashboard.
    pub fn agent_market_snapshot(&self, cx: &App) -> serde_json::Value {
        let tickers = self.ticker_views.borrow().iter().map(|(pane_id, state)| {
            serde_json::json!({"pane_id": pane_id, "type": "ticker", "data": state.entity.read(cx).agent_snapshot()})
        }).collect::<Vec<_>>();
        let charts = self.chart_views.borrow().iter().map(|(pane_id, state)| {
            serde_json::json!({"pane_id": pane_id, "type": "chart", "data": state.entity.read(cx).agent_snapshot()})
        }).collect::<Vec<_>>();
        let order_books = self.orderbook_views.borrow().iter().map(|(pane_id, state)| {
            serde_json::json!({"pane_id": pane_id, "type": "orderbook", "data": state.entity.read(cx).agent_snapshot()})
        }).collect::<Vec<_>>();
        let tapes = self.tape_views.borrow().iter().map(|(pane_id, state)| {
            serde_json::json!({"pane_id": pane_id, "type": "tape", "data": state.entity.read(cx).agent_snapshot()})
        }).collect::<Vec<_>>();
        let news = self.newsfeed_views.borrow().iter().map(|(pane_id, state)| {
            serde_json::json!({"pane_id": pane_id, "type": "newsfeed", "data": state.entity.read(cx).agent_snapshot()})
        }).collect::<Vec<_>>();
        let fear_greed = self.fear_greed_views.borrow().iter().map(|(pane_id, state)| {
            serde_json::json!({"pane_id": pane_id, "type": "fear_greed", "data": state.entity.read(cx).agent_snapshot()})
        }).collect::<Vec<_>>();
        serde_json::json!({"tickers": tickers, "charts": charts, "order_books": order_books, "tapes": tapes, "news": news, "fear_greed": fear_greed})
    }

    pub fn set_agent_context(&mut self, context: serde_json::Value, cx: &mut Context<Self>) {
        self.agent_context = Some(context);
        let snapshot = self.agent_context.clone().unwrap_or(serde_json::Value::Null);
        for state in self.agent_views.borrow().values() {
            state.entity.update(cx, |panel, _| panel.set_context(snapshot.clone()));
        }
    }

    pub fn set_tree(&mut self, tree: PaneNode, cx: &mut Context<Self>) {
        fn collect_modules(node: &PaneNode, modules: &mut std::collections::HashMap<String, String>) {
            match node {
                PaneNode::Pane { id, data } => {
                    modules.insert(
                        id.clone(),
                        data.module_type.as_deref().unwrap_or("empty").to_lowercase(),
                    );
                }
                PaneNode::Split { a, b, .. } => {
                    collect_modules(a, modules);
                    collect_modules(b, modules);
                }
            }
        }

        let mut modules = std::collections::HashMap::new();
        collect_modules(&tree, &mut modules);

        // Releasing a cached state drops its panel and bridge. Bridge::drop cancels
        // the WebSocket supervisor, while MarketCapPanel::drop cancels its syncer.
        self.chart_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "chart"));
        self.ticker_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "ticker"));
        self.orderbook_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "orderbook"));
        self.agent_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "agent"));
        self.tape_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "trades" || m == "tape"));
        self.watchlist_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "watchlist"));
        self.market_cap_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "marketcap"));
        self.newsfeed_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "newsfeed"));
        self.clock_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "time" || m == "clock"));
        self.fear_greed_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "feargreed"));

        self.positions_views.borrow_mut().retain(|id, _| modules.get(id).is_some_and(|m| m == "positions"));
        self.execution_views.borrow_mut().retain(|id, state| {
            fn matches_config(node: &PaneNode, id: &str, state: &ExecutionViewState) -> bool {
                match node {
                    PaneNode::Pane { id: pane_id, data } if pane_id == id => {
                        let config = data.config.as_object();
                        matches!(data.module_type.as_deref().unwrap_or("").to_lowercase().as_str(), "execution" | "tradedesk")
                            && config_str(config, "exchange", "binance") == state.exchange
                            && config_str(config, "symbol", "BTC/USDT") == state.symbol
                    }
                    PaneNode::Split { a, b, .. } => matches_config(a, id, state) || matches_config(b, id, state),
                    _ => false,
                }
            }
            matches_config(&tree, id, state)
        });
        self.tree = Some(tree);
        cx.notify();
    }

    fn render_placeholder_selector(
        &self,
        pane_id: &str,
        width: f32,
        height: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = cx.theme().clone();
        let colors = &theme.colors;

        let modules: [(&str, &str, &str, &str); 12] = [
            ("chart", "Chart", "Candlestick & technical indicators", "icons/chart-candlestick.svg"),
            ("ticker", "Ticker", "Live price, stats & sparkline", "icons/trending-up.svg"),
            ("orderbook", "Order Book", "Level 2 market depth & ladder", "icons/sort-descending.svg"),
            ("agent", "Agent", "AI market assistant & cross-dashboard analysis", "icons/cpu.svg"),
            ("trades", "Trades / Tape", "Real-time executed market tape", "icons/trending-up.svg"),
            ("positions", "Positions", "Open positions and unrealized P&L", "icons/crosshair.svg"),
            ("execution", "Trade Desk", "Read-only order ticket preview", "icons/crosshair.svg"),
            ("watchlist", "Watchlist", "Multi-symbol real-time price tracker", "icons/star.svg"),
            ("marketcap", "Market Cap", "Crypto capitalization heatmap & table", "icons/chart-pie.svg"),
            ("newsfeed", "Newsfeed", "Real-time crypto & market news", "icons/message-square.svg"),
            ("time", "Clock", "Multi-timezone market clocks", "icons/globe.svg"),
            ("feargreed", "Fear & Greed", "Crypto market sentiment index", "icons/chart-pie.svg"),
        ];

        // Only show a grid when all of its rows fit inside the pane. Otherwise
        // use denser columns, or the dropdown for very small panes.
        let spacious_columns = if width >= 540.0 && height >= 340.0 {
            Some(3)
        } else if width >= 380.0 && height >= 500.0 {
            Some(2)
        } else {
            None
        };
        let compact_columns = if width >= 400.0 && height >= 200.0 {
            Some(3)
        } else if width >= 280.0 && height >= 290.0 {
            Some(2)
        } else if width >= 180.0 && height >= 550.0 {
            Some(1)
        } else {
            None
        };

        if let Some(columns) = spacious_columns {
            let rows: Vec<Vec<(&str, &str, &str, &str)>> =
                modules.chunks(columns).map(|row| row.to_vec()).collect();

            let mut grid_div = div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).w_full().max_w(px(580.0));

            for row in rows {
                let mut row_div = div().flex_1().min_h_0().flex().flex_row().gap(px(6.0)).w_full();
                for (mod_id, label, desc, icon_path) in row {
                    let target_pane_id = pane_id.to_string();
                    let mod_id_str = mod_id.to_string();
                    let card = div()
                        .id(ElementId::Name(format!("placeholder-card-{pane_id}-{mod_id}").into()))
                        .flex_1()
                        .h_full()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .p(px(8.0))
                        
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.muted.opacity(0.2))
                        .cursor_pointer()
                        .hover(|s| s.border_color(colors.primary).bg(colors.muted.opacity(0.55)))
                        .on_mouse_down(MouseButton::Left, cx.listener({
                            let target_pane_id = target_pane_id.clone();
                            let mod_id_str = mod_id_str.clone();
                            move |_this, _event, window, cx| {
                                let target_pane_id = target_pane_id.clone();
                                let mod_id_str = mod_id_str.clone();
                                window.defer(cx, move |_win, cx| {
                                    cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                        let _ = global.view.update(cx, |view, cx| {
                                            view.handle_pane_event(
                                                &PaneGridEvent::SetModuleType {
                                                    pane_id: target_pane_id,
                                                    module_type: mod_id_str,
                                                },
                                                cx,
                                            );
                                        });
                                    });
                                });
                            }
                        }))
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(32.0))
                                .h(px(32.0))
                                
                                .bg(colors.primary.opacity(0.12))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    gpui_component::Icon::default()
                                        .path(icon_path)
                                        .size(px(16.0))
                                        .text_color(colors.primary)
                                )
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(1.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.foreground)
                                        .child(label)
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(colors.muted_foreground)
                                        .truncate()
                                        .child(desc)
                                )
                        );
                    row_div = row_div.child(card);
                }
                grid_div = grid_div.child(row_div);
            }

            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(px(8.0))
                .bg(colors.background)
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.0))
                        .mb(px(8.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                
                                .bg(colors.primary.opacity(0.12))
                                .text_size(px(9.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.primary)
                                .child("EMPTY WORKSPACE PANE")
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.foreground)
                                .child("Select a module to display")
                        )
                )
                .child(grid_div)
                .into_any_element()
        } else if let Some(columns) = compact_columns {
            let rows: Vec<Vec<(&str, &str, &str, &str)>> =
                modules.chunks(columns).map(|row| row.to_vec()).collect();

            let mut list_div = div().flex_1().min_h_0().flex().flex_col().gap(px(3.0)).w_full().max_w(px(580.0));

            for row in rows {
                let mut row_div = div().flex_1().min_h_0().flex().flex_row().gap(px(3.0)).w_full();
                for (mod_id, label, _, icon_path) in row {
                    let target_pane_id = pane_id.to_string();
                    let mod_id_str = mod_id.to_string();
                    let item = div()
                        .id(ElementId::Name(format!("compact-pick-{pane_id}-{mod_id}").into()))
                        .flex_1()
                        .h_full()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(6.0))
                        .py(px(4.0))
                        
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.muted.opacity(0.2))
                        .cursor_pointer()
                        .hover(|s| s.border_color(colors.primary).bg(colors.muted.opacity(0.5)))
                        .on_mouse_down(MouseButton::Left, cx.listener({
                            let target_pane_id = target_pane_id.clone();
                            let mod_id_str = mod_id_str.clone();
                            move |_this, _event, window, cx| {
                                let target_pane_id = target_pane_id.clone();
                                let mod_id_str = mod_id_str.clone();
                                window.defer(cx, move |_win, cx| {
                                    cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                        let _ = global.view.update(cx, |view, cx| {
                                            view.handle_pane_event(
                                                &PaneGridEvent::SetModuleType {
                                                    pane_id: target_pane_id,
                                                    module_type: mod_id_str,
                                                },
                                                cx,
                                            );
                                        });
                                    });
                                });
                            }
                        }))
                        .child(
                            gpui_component::Icon::default()
                                .path(icon_path)
                                .size(px(13.0))
                                .text_color(colors.primary)
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.foreground)
                                .truncate()
                                .child(label)
                        );
                    row_div = row_div.child(item);
                }
                list_div = list_div.child(row_div);
            }

            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(px(6.0))
                .bg(colors.background)
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.muted_foreground)
                        .mb(px(4.0))
                        .child("Select Module")
                )
                .child(list_div)
                .into_any_element()
        } else {
            let target_pane_id = pane_id.to_string();
            let btn = Button::new(ElementId::Name(format!("mini-pick-btn-{pane_id}").into()))
                .label("Select Module")
                .xsmall()
                .ghost()
                .dropdown_caret(true)
                .dropdown_menu_with_anchor(Corner::BottomLeft, move |mut menu, _window, _cx| {
                    for (mod_id, label, _, _) in modules {
                        let target_pane_id = target_pane_id.clone();
                        let mod_id_str = mod_id.to_string();
                        menu = menu.item(
                            gpui_component::menu::PopupMenuItem::new(label)
                                .on_click(move |_event, window, cx| {
                                    let target_pane_id = target_pane_id.clone();
                                    let mod_id_str = mod_id_str.clone();
                                    window.defer(cx, move |_window, cx| {
                                        cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                            let _ = global.view.update(cx, |view, cx| {
                                                view.handle_pane_event(
                                                    &PaneGridEvent::SetModuleType {
                                                        pane_id: target_pane_id,
                                                        module_type: mod_id_str,
                                                    },
                                                    cx,
                                                );
                                            });
                                        });
                                    });
                                }),
                        );
                    }
                    menu
                });

            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(4.0))
                .bg(colors.background)
                .child(btn)
                .into_any_element()
        }
    }

    fn render_node(&self, node: &PaneNode, current_size: gpui::Size<f32>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match node {
            PaneNode::Pane { id, data } => {
                let theme = cx.theme().clone();
                let module_type = data.module_type.as_deref().unwrap_or("Empty");
                let content_w = (current_size.width - 6.0).max(10.0);
                let content_h = (current_size.height - 6.0 - 33.0).max(10.0);

                let is_empty = module_type.eq_ignore_ascii_case("empty") || module_type.is_empty();
                let pane_id = id.clone();
                let split_h_id = pane_id.clone();
                let split_v_id = pane_id.clone();
                let close_id = pane_id.clone();
                let click_id = pane_id.clone();
                let chart_timeframe = data
                    .config
                    .as_object()
                    .and_then(|config| config.get("interval"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("1m")
                    .to_string();
                let config_map = data.config.as_object();
                let chart_type = config_str(config_map, "type", "candle_solid");
                let current_exchange = config_str(config_map, "exchange", "binance");
                let current_symbol = config_str(config_map, "symbol", "BTC/USDT");
                let current_timezone = config_str(config_map, "timezone", &self.timezone);

                let is_active = self.active_pane_id.as_ref() == Some(&pane_id);
                let matrix_hover = self.matrix_hover.clone();
                let is_dragged = *self.dragged_pane.borrow() == Some(pane_id.clone());
                let border_color = if is_active {
                    theme.colors.primary
                } else {
                    theme.colors.border
                };
                let title_bg = if is_active {
                    theme.colors.primary
                } else {
                    theme.colors.muted
                };
                let title_fg = if is_active {
                    theme.colors.primary_foreground
                } else {
                    theme.colors.muted_foreground
                };

                let is_market_module = matches!(
                    module_type.to_lowercase().as_str(),
                    "chart" | "ticker" | "orderbook" | "trades" | "tape" | "execution" | "tradedesk"
                );
                if is_market_module {
                    crate::data::gateway_registry::GatewayRegistry::global()
                        .prefetch_symbols(&current_exchange);
                }
                let suffix = match module_type.to_lowercase().as_str() {
                    "orderbook" => Some("OB"),
                    "agent" => None,
                    "trades" | "tape" => Some("TRADES"),
                    "execution" | "tradedesk" => Some("DESK"),
                    _ => None,
                };

                div()
                    .id(ElementId::Name(format!("pane-wrapper-{pane_id}").into()))
                    .size_full()
                    // Account for this wrapper's horizontal padding so a maximized pane
                    // remains inside the pane-grid bounds after response remeasurement.
                    .w(px(content_w))
                    .max_w_full()
                    .flex_none()
                    .p(px(3.0))
                    .when(is_dragged, |this| {
                        this.child(
                            div()
                                .size_full()
                                .border_2()
                                .border_dashed()
                                .border_color(theme.colors.border.opacity(0.5))
                                .bg(theme.colors.muted.opacity(0.2))
                                ,
                        )
                    })
                    .when(!is_dragged, |this| {
                        this.child(
                            div()
                                .id(ElementId::Name(format!("pane-container-{pane_id}").into()))
                            .w_full()
                            .min_w_0()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _event, _win, cx| {
                                    cx.emit(PaneGridEvent::SetActivePane {
                                        pane_id: click_id.clone(),
                                    });
                                }),
                            )
                            .size_full()
                            // `pane-wrapper` has 3px padding on each side; use its inner width.
                            .w(px(content_w))
                            .max_w_full()
                            .flex_none()
                            .bg(theme.colors.background)
                            .border_2()
                            .border_color(border_color)
                            
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(
                                // Unified Title Bar
                        div()
                            .id(ElementId::Name(format!("titlebar-{pane_id}").into()))
                            // Keep this row at the pane width even when response content
                            // causes a new intrinsic-size measurement.
                            .w_full()
                            .min_w_full()
                            .flex_none()
                            .flex_shrink_0()
                            .h(px(33.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .bg(title_bg)
                            .text_color(title_fg)
                            .border_b_1()
                            .border_color(theme.colors.border)
                            .drag_over::<DragPane>(|style, _drag, _win, _cx| style.bg(gpui::rgba(0x4ade8055)))
                            .on_drop({
                                let target_id = pane_id.clone();
                                cx.listener(move |_this, drag_pane: &DragPane, _window, cx| {
                                    if drag_pane.0 != target_id {
                                        cx.emit(PaneGridEvent::SwapPanes {
                                            id_a: drag_pane.0.clone(),
                                            id_b: target_id.clone(),
                                        });
                                    }
                                })
                            })
                            .on_drag(
                                DragPane(pane_id.clone()),
                                {
                                    let mod_type = if is_empty { "Select Module".to_string() } else { module_type.to_string() };
                                    let drag_content = match module_type.to_lowercase().as_str() {
                                        "chart" => self.chart_views.borrow().get(&pane_id).map(|s| DragContent::Chart(s.entity.clone())),
                                        "ticker" => self.ticker_views.borrow().get(&pane_id).map(|s| DragContent::Ticker(s.entity.clone())),
                                        "orderbook" => self.orderbook_views.borrow().get(&pane_id).map(|s| DragContent::Orderbook(s.entity.clone())),
                                        "agent" => self.agent_views.borrow().get(&pane_id).map(|s| DragContent::Agent(s.entity.clone())),
                                        "trades" | "tape" => self.tape_views.borrow().get(&pane_id).map(|s| DragContent::Tape(s.entity.clone())),
                                        "execution" | "tradedesk" => self.execution_views.borrow().get(&pane_id).map(|s| DragContent::Execution(s.entity.clone())),
                                        "positions" => self.positions_views.borrow().get(&pane_id).map(|s| DragContent::Positions(s.clone())),
                                        "watchlist" => self.watchlist_views.borrow().get(&pane_id).map(|s| DragContent::Watchlist(s.entity.clone())),
                                        "marketcap" => self.market_cap_views.borrow().get(&pane_id).map(|s| DragContent::MarketCap(s.entity.clone())),
                                        "newsfeed" => self.newsfeed_views.borrow().get(&pane_id).map(|s| DragContent::Newsfeed(s.entity.clone())),
                                        "time" | "clock" => self.clock_views.borrow().get(&pane_id).map(|s| DragContent::Clock(s.entity.clone())),
                                        "feargreed" => self.fear_greed_views.borrow().get(&pane_id).map(|s| DragContent::FearGreed(s.entity.clone())),
                                        _ => None,
                                    }.unwrap_or_else(|| DragContent::Placeholder(mod_type.clone()));
                                    let w = current_size.width;
                                    let h = current_size.height;
                                    let dragged_pane_ref = self.dragged_pane.clone();
                                    let pane_id_clone = pane_id.clone();
                                    let ex_clone = current_exchange.clone();
                                    let sym_clone = current_symbol.clone();
                                    let tf_clone = chart_timeframe.clone();
                                    let tz_clone = current_timezone.clone();
                                    move |_drag_pane, _offset, window, app| {
                                        *dragged_pane_ref.borrow_mut() = Some(pane_id_clone.clone());
                                        window.refresh();

                                        let drop_ref = dragged_pane_ref.clone();
                                        let content_clone = drag_content.clone();
                                        app.new(|_cx| DragView {
                                            module_type: mod_type.clone(),
                                            exchange: ex_clone.clone(),
                                            symbol: sym_clone.clone(),
                                            timeframe: tf_clone.clone(),
                                            timezone: tz_clone.clone(),
                                            content: content_clone,
                                            width: w,
                                            height: h,
                                            on_drop: std::rc::Rc::new(move || {
                                                *drop_ref.borrow_mut() = None;
                                            }),
                                        })
                                    }
                                }
                            )
                            .child(
                                gpui_component::h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(6.0))
                                    .items_center()
                                    .overflow_hidden()
                                    .when(is_market_module, |this| {
                                        this.child(render_coinwatch_icons(&pane_id, &current_exchange, &current_symbol))
                                            .when_some(suffix, |this, s| {
                                                this.child(
                                                    div()
                                                        .flex_shrink_0()
                                                        .px(px(4.0))
                                                        .py(px(1.0))
                                                        
                                                        .bg(theme.colors.muted.opacity(0.5))
                                                        .border_1()
                                                        .border_color(theme.colors.border.opacity(0.4))
                                                        .text_size(px(10.0))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(title_fg.opacity(0.85))
                                                        .child(s),
                                                )
                                            })
                                    })
                                    .when(!is_market_module, |this| {
                                        this.child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .text_sm()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(
                                                    if module_type.eq_ignore_ascii_case("agent") {
                                                        "Agent".to_string()
                                                    } else if is_empty {
                                                        "Select Module".to_string()
                                                    } else if module_type.to_lowercase() == "time" || module_type.to_lowercase() == "clock" {
                                                        current_timezone.replace('/', ", ").replace('_', " ")
                                                    } else if module_type.to_lowercase() == "marketcap" {
                                                        "Market Cap".to_string()
                                                    } else if module_type.to_lowercase() == "watchlist" {
                                                        "Watchlist".to_string()
                                                    } else if module_type.to_lowercase() == "feargreed" {
                                                        "Fear & Greed Index".to_string()
                                                    } else if module_type.to_lowercase() == "positions" {
                                                        "Positions".to_string()
                                                    } else {
                                                        module_type.to_string()
                                                    },
                                                ),
                                        )
                                    })
                                    .when(module_type.to_lowercase() == "chart", |this| {
                                        let this = this.child(
                                            div()
                                                .flex_shrink_0()
                                                .w(px(1.0))
                                                .h(px(14.0))
                                                .bg(theme.colors.border.opacity(0.6))
                                                .mx(px(2.0)),
                                        );
                                        let current_timeframe = chart_timeframe.clone();
                                        let all_timeframes = [
                                            "1s", "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d", "1w", "1M",
                                        ];

                                        if current_size.width < 340.0 {
                                            let pane_id = pane_id.clone();
                                            let current_tf = current_timeframe.clone();
                                            this.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .child(
                                                        Button::new(ElementId::Name(format!("timeframe-dropdown-{pane_id}").into()))
                                                            .label(current_timeframe)
                                                            .xsmall()
                                                            .ghost()
                                                            .selected(true)
                                                            .when(is_active, |button| {
                                                                button
                                                                    .bg(theme.colors.background)
                                                                    .text_color(theme.colors.primary)
                                                                    .font_weight(gpui::FontWeight::BOLD)
                                                            })
                                                            .when(!is_active, |button| {
                                                                button
                                                                    .bg(theme.colors.primary)
                                                                    .text_color(theme.colors.primary_foreground)
                                                                    .font_weight(gpui::FontWeight::BOLD)
                                                            })
                                                            .dropdown_caret(true)
                                                            .dropdown_menu_with_anchor(Corner::BottomLeft, move |mut menu, _window, _cx| {
                                                                for tf in all_timeframes {
                                                                    let pane_id = pane_id.clone();
                                                                    let selected = current_tf == tf;
                                                                    menu = menu.item(
                                                                        gpui_component::menu::PopupMenuItem::new(tf)
                                                                            .checked(selected)
                                                                            .on_click(move |_event, window, cx| {
                                                                                let pane_id = pane_id.clone();
                                                                                let tf_str = tf.to_string();
                                                                                window.defer(cx, move |_window, cx| {
                                                                                    let mut config = serde_json::Map::new();
                                                                                    config.insert("interval".to_string(), serde_json::Value::String(tf_str));
                                                                                    cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                                                                        let _ = global.view.update(cx, |view, cx| {
                                                                                            view.handle_pane_event(
                                                                                                &PaneGridEvent::UpdateModuleConfig {
                                                                                                    pane_id,
                                                                                                    config: serde_json::Value::Object(config),
                                                                                                },
                                                                                                cx,
                                                                                            );
                                                                                        });
                                                                                    });
                                                                                });
                                                                            }),
                                                                    );
                                                                }
                                                                menu
                                                            }),
                                                    )
                                            )
                                        } else {
                                            let common_timeframes: &[&str] = if current_size.width < 500.0 {
                                                &["1m", "15m", "1h", "1d"]
                                            } else {
                                                &["1m", "5m", "15m", "1h", "4h", "1d"]
                                            };
                                            let extra_timeframes: Vec<&str> = all_timeframes
                                                .iter()
                                                .copied()
                                                .filter(|tf| !common_timeframes.contains(tf))
                                                .collect();

                                            let pane_id = pane_id.clone();
                                            let mut controls = gpui_component::h_flex()
                                                .flex_shrink_0()
                                                .gap(px(1.0))
                                                .items_center();

                                            for timeframe in common_timeframes {
                                                let pane_id = pane_id.clone();
                                                let selected = current_timeframe == *timeframe;
                                                controls = controls.child(
                                                    Button::new(ElementId::Name(
                                                        format!("timeframe-{pane_id}-{timeframe}").into(),
                                                    ))
                                                    .label(*timeframe)
                                                    .xsmall()
                                                    .ghost()
                                                    .selected(selected)
                                                    .when(selected, |button| {
                                                        if is_active {
                                                            button
                                                                .bg(theme.colors.background)
                                                                .text_color(theme.colors.primary)
                                                                .font_weight(gpui::FontWeight::BOLD)
                                                        } else {
                                                            button
                                                                .bg(theme.colors.primary)
                                                                .text_color(theme.colors.primary_foreground)
                                                                .font_weight(gpui::FontWeight::BOLD)
                                                        }
                                                    })
                                                    .when(!selected && is_active, |button| {
                                                        button.text_color(title_fg.opacity(0.8))
                                                    })
                                                    .on_click(move |_event, window, cx| {
                                                        let pane_id = pane_id.clone();
                                                        let timeframe = timeframe.to_string();
                                                        window.defer(cx, move |_window, cx| {
                                                            let mut config = serde_json::Map::new();
                                                            config.insert(
                                                                "interval".to_string(),
                                                                serde_json::Value::String(timeframe),
                                                            );
                                                            cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                                                let _ = global.view.update(cx, |view, cx| {
                                                                    view.handle_pane_event(
                                                                        &PaneGridEvent::UpdateModuleConfig {
                                                                            pane_id,
                                                                            config: serde_json::Value::Object(config),
                                                                        },
                                                                        cx,
                                                                    );
                                                                });
                                                            });
                                                        });
                                                    }),
                                                );
                                            }

                                            let pane_id = pane_id.clone();
                                            let is_extra_selected = extra_timeframes.contains(&current_timeframe.as_str());
                                            let more_label = if is_extra_selected {
                                                current_timeframe.clone()
                                            } else {
                                                "More".to_string()
                                            };
                                            let current_timeframe = current_timeframe.clone();
                                            this.child(
                                                div()
                                                    .flex_shrink_0()
                                                    .child(controls.child(
                                                        Button::new(ElementId::Name(
                                                            format!("timeframe-more-{pane_id}").into(),
                                                        ))
                                                        .label(more_label)
                                                        .xsmall()
                                                        .ghost()
                                                        .selected(is_extra_selected)
                                                        .when(is_extra_selected, |button| {
                                                            if is_active {
                                                                button
                                                                    .bg(theme.colors.background)
                                                                    .text_color(theme.colors.primary)
                                                                    .font_weight(gpui::FontWeight::BOLD)
                                                            } else {
                                                                button
                                                                    .bg(theme.colors.primary)
                                                                    .text_color(theme.colors.primary_foreground)
                                                                    .font_weight(gpui::FontWeight::BOLD)
                                                            }
                                                        })
                                                        .when(!is_extra_selected && is_active, |button| {
                                                            button.text_color(title_fg.opacity(0.8))
                                                        })
                                                        .dropdown_caret(true)
                                                        .dropdown_menu_with_anchor(
                                                            Corner::BottomLeft,
                                                            move |mut menu, _window, _cx| {
                                                                for timeframe in &extra_timeframes {
                                                                    let pane_id = pane_id.clone();
                                                                    let selected = current_timeframe == *timeframe;
                                                                    let tf_str = timeframe.to_string();
                                                                    menu = menu.item(
                                                                        gpui_component::menu::PopupMenuItem::new(tf_str.clone())
                                                                            .checked(selected)
                                                                            .on_click(move |_event, window, cx| {
                                                                                let pane_id = pane_id.clone();
                                                                                let timeframe = tf_str.clone();
                                                                                window.defer(cx, move |_window, cx| {
                                                                                    let mut config = serde_json::Map::new();
                                                                                    config.insert(
                                                                                        "interval".to_string(),
                                                                                        serde_json::Value::String(timeframe),
                                                                                    );
                                                                                    cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                                                                        let _ = global.view.update(cx, |view, cx| {
                                                                                            view.handle_pane_event(
                                                                                                &PaneGridEvent::UpdateModuleConfig {
                                                                                                    pane_id,
                                                                                                    config: serde_json::Value::Object(config),
                                                                                                },
                                                                                                cx,
                                                                                            );
                                                                                        });
                                                                                    });
                                                                                });
                                                                            }),
                                                                    );
                                                                }
                                                                menu
                                                            },
                                                        ),
                                                    ))
                                            )
                                        }
                                    }),
                            )
                            // Toolbar
                            .child(
                                div()
                                    .flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap(px(2.0))
                                    .when({
                                        let mod_name = match module_type.to_lowercase().as_str() {
                                            "chart" => "Chart",
                                            "ticker" => "Ticker",
                                            "orderbook" => "OrderBook",
                                            "trades" | "tape" => "Tape",
                                            "execution" | "tradedesk" => "Trade",
                                            "agent" => "AI Agent",
                                            "watchlist" => "Watchlist",
                                            "positions" => "Positions",
                                            "marketcap" => "MarketCap",
                                            "newsfeed" => "Newsfeed",
                                            "feargreed" => "FearGreed",
                                            _ => "",
                                        };
                                        !mod_name.is_empty() && crate::data::credits::CreditStore::module_error(mod_name).is_some()
                                    }, |toolbar| {
                                        toolbar.child(
                                            div()
                                                .id(ElementId::Name(format!("credit-warn-badge-{split_h_id}").into()))
                                                .px(px(4.0))
                                                .py(px(1.0))
                                                .rounded(px(3.0))
                                                .bg(theme.colors.danger.opacity(0.18))
                                                .border_1()
                                                .border_color(theme.colors.danger.opacity(0.45))
                                                .text_size(px(10.0))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(theme.colors.danger)
                                                .cursor_pointer()
                                                .tooltip(move |win, cx| gpui_component::tooltip::Tooltip::new("Credit limit reached. Click to top up credits.").build(win, cx))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|_this, _ev, _win, _cx| {
                                                        crate::data::credits::CreditStore::open_billing_page(None);
                                                    }),
                                                )
                                                .child("NO CREDITS"),
                                        )
                                    })
                                    .child(
                                        Popover::new(ElementId::Name(format!("layout-pop-{split_h_id}").into())).anchor(gpui::Corner::TopRight)
                                            .trigger(
                                                Button::new(ElementId::Name(
                                                    format!("split-btn-{split_h_id}").into(),
                                                ))
                                                .icon(
                                                    gpui_component::Icon::default()
                                                        .path("icons/layout-dashboard.svg"),
                                                )
                                                .ghost()
                                                .xsmall()
                                                .text_color(title_fg)
                                                .hover(|s| {
                                                    if is_active {
                                                        s.text_color(title_fg).bg(theme.colors.background.opacity(0.15))
                                                    } else {
                                                        s.text_color(theme.colors.foreground).bg(theme.colors.muted.opacity(0.4))
                                                    }
                                                })
                                            )
                                            .content({
                                                let pane_id_h = split_h_id.clone();
                                                let global_hover = matrix_hover.clone();
                                                move |_state, _window, cx| {
                                                    let mut grid_rows = div().flex().flex_col();
                                                    for r in 1..=5 {
                                                        let mut grid_cols = div().flex().flex_row();
                                                        for c in 1..=5 {
                                                            let is_hovered = match global_hover.borrow().get(&pane_id_h) {
                                                                Some(&(hr, hc)) => r <= hr && c <= hc,
                                                                None => false,
                                                            };
                                                            let hover_bg = if is_hovered { cx.theme().colors.primary.opacity(0.6) } else { cx.theme().colors.background };
                                                            
                                                            let hs = global_hover.clone();
                                                            let p_id_cell = pane_id_h.clone();
                                                            
                                                            let cell = div()
                                                                .id(ElementId::Name(format!("matrix-cell-{r}-{c}").into()))
                                                                .w(px(50.0))
                                                                .h(px(50.0))
                                                                .border_1()
                                                                .border_color(cx.theme().colors.primary)
                                                                .bg(hover_bg)
                                                                .cursor_pointer()
                                                                .on_mouse_move(cx.listener({
                                                                    let hs = hs.clone();
                                                                    let p_id_cell = p_id_cell.clone();
                                                                    move |_this, _ev, _win, cx| {
                                                                        cx.stop_propagation();
                                                                        let mut map = hs.borrow_mut();
                                                                        if map.get(&p_id_cell) != Some(&(r, c)) {
                                                                            map.insert(p_id_cell.clone(), (r, c));
                                                                            cx.notify();
                                                                        }
                                                                    }
                                                                }))
                                                                .on_mouse_down(MouseButton::Left, cx.listener({
                                                                    let p_id_cell = p_id_cell.clone();
                                                                    move |state: &mut gpui_component::popover::PopoverState, _ev, win, cx| {
                                                                        state.dismiss(win, cx);
                                                                        cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|g, cx| {
                                                                            let _ = g.view.update(cx, |this, cx| this.handle_pane_event(&PaneGridEvent::SplitMatrix { pane_id: p_id_cell.clone(), rows: r, cols: c }, cx));
                                                                        });
                                                                    }
                                                                }));
                                                            grid_cols = grid_cols.child(cell);
                                                        }
                                                        grid_rows = grid_rows.child(grid_cols);
                                                    }
                                                    
                                                    let matrix_ui = div()
                                                        .border_1()
                                                        .border_color(cx.theme().colors.primary)
                                                        .on_mouse_down_out(cx.listener({
                                                            let hs = global_hover.clone();
                                                            let p_id_h = pane_id_h.clone();
                                                            move |_this, _ev, _win, cx| {
                                                                hs.borrow_mut().remove(&p_id_h);
                                                                cx.notify();
                                                            }
                                                        }))
                                                        .child(grid_rows);
                                                    
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(4.0))
                                                        .p(px(8.0))
                                                        .on_mouse_move(cx.listener({
                                                            let hs = global_hover.clone();
                                                            let p_id_h = pane_id_h.clone();
                                                            move |_this, _ev, _win, cx| {
                                                                if hs.borrow().contains_key(&p_id_h) {
                                                                    hs.borrow_mut().remove(&p_id_h);
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                                .child("Module Split")
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(cx.theme().colors.muted_foreground)
                                                                .child("Split module layout")
                                                        )
                                                        .child(
                                                            div().h(px(4.0)) // Spacer
                                                        )
                                                        .child(
                                                            div().flex().flex_1().justify_center()
                                                                .child(matrix_ui)
                                                        )
                                                        .into_any_element()
                                                }

                                            })
                                    )
                                    .child(
                                        Button::new(ElementId::Name(format!("edit-btn-{split_v_id}").into()))
                                            .icon(gpui_component::Icon::default().path("icons/settings.svg"))
                                            .ghost()
                                            .xsmall()
                                            .text_color(title_fg)
                                            .hover(|s| {
                                                if is_active {
                                                    s.text_color(title_fg).bg(theme.colors.background.opacity(0.15))
                                                } else {
                                                    s.text_color(theme.colors.foreground).bg(theme.colors.muted.opacity(0.4))
                                                }
                                            })
                                            .group_list_popup_with_anchor(gpui::Corner::TopRight, {
                                                let pane_id_cfg = split_v_id.clone();
                                                let current_type = module_type.to_lowercase();
                                                let current_chart_type = chart_type.clone();
                                                let current_exchange = current_exchange.clone();
                                                let current_symbol = current_symbol.clone();
                                                let current_timezone = current_timezone.clone();
                                                let agent_views = self.agent_views.clone();
                                                move |mut menu, _window, cx| {
                                                    let module_types = [
                                                        ("chart", "Chart"),
                                                        ("ticker", "Ticker"),
                                                        ("orderbook", "Order Book"),
                                                        ("agent", "Agent"),
                                                        ("trades", "Trades / Tape"),
                                                        ("positions", "Positions"),
                                                        ("execution", "Trade Desk"),
                                                        ("watchlist", "Watchlist"),
                                                        ("marketcap", "Market Cap"),
                                                        ("newsfeed", "Newsfeed"),
                                                        ("feargreed", "Fear and Greed Index"),
                                                        ("time", "Time"),
                                                    ];

                                                    menu = menu.label("Module Type");
                                                    for (value, label) in module_types {
                                                        let pane_id = pane_id_cfg.clone();
                                                        let value = value.to_string();
                                                        menu = menu.item(gpui_component::menu::PopupMenuItem::new(label).checked(current_type == value).on_click(move |_event, window, cx| {
                                                            let pane_id = pane_id.clone();
                                                            let value = value.clone();
                                                            window.defer(cx, move |_window, cx| {
                                                                cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                                                    let _ = global.view.update(cx, |view, cx| view.handle_pane_event(&PaneGridEvent::SetModuleType { pane_id, module_type: value }, cx));
                                                                });
                                                            });
                                                        }));
                                                    }

                                                    match current_type.as_str() {
                                                        "time" => {
                                                            menu = menu.separator().label("Timezone");
                                                            for (tz_val, tz_lbl) in timezone_choices() {
                                                                let pane_id = pane_id_cfg.clone();
                                                                let selected = current_timezone == *tz_val;
                                                                let value = tz_val.to_string();
                                                                let item_label = if *tz_val == *tz_lbl {
                                                                    tz_val.to_string()
                                                                } else {
                                                                    format!("{tz_val} ({tz_lbl})")
                                                                };
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new(item_label)
                                                                        .checked(selected)
                                                                        .on_click(move |_event, window, cx| {
                                                                            update_pane_config(pane_id.clone(), "timezone", serde_json::Value::String(value.clone()), window, cx);
                                                                        }),
                                                                );
                                                            }
                                                        }
                                                        "agent" => {
                                                            menu = menu.separator().label("Conversations");
                                                            let current_conv_id = agent_views.borrow().get(&pane_id_cfg).and_then(|s| s.entity.read(cx).current_conversation_id());
                                                            let is_new = current_conv_id.is_none();
                                                            let agent_view = agent_views.borrow().get(&pane_id_cfg).map(|s| s.entity.clone());

                                                            {
                                                                let agent_view = agent_view.clone();
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new("New conversation")
                                                                        .checked(is_new)
                                                                        .on_click(move |_event, _window, cx| {
                                                                            if let Some(agent) = &agent_view {
                                                                                agent.update(cx, |panel, cx| panel.start_new_conversation(cx));
                                                                            }
                                                                        }),
                                                                );
                                                            }

                                                            let saved = crate::data::agent::load_all_conversations();
                                                            if saved.is_empty() {
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new("No past conversations")
                                                                        .disabled(true),
                                                                );
                                                            } else {
                                                                for conv in saved {
                                                                    let conv_id = conv.id.clone();
                                                                    let is_selected = current_conv_id.as_deref() == Some(&conv_id);
                                                                    let label = if conv.title.trim().is_empty() {
                                                                        format!("Chat ({})", conv.updated_at)
                                                                    } else {
                                                                        format!("{} ({})", conv.title, conv.updated_at)
                                                                    };
                                                                    let agent_view = agent_view.clone();
                                                                    menu = menu.item(
                                                                        gpui_component::menu::PopupMenuItem::new(label)
                                                                            .checked(is_selected)
                                                                            .on_click(move |_event, _window, cx| {
                                                                                if let Some(agent) = &agent_view {
                                                                                    agent.update(cx, |panel, cx| panel.load_conversation(&conv_id, cx));
                                                                                }
                                                                            }),
                                                                    );
                                                                }
                                                            }
                                                        }
                                                        "ticker" | "orderbook" | "trades" | "tape" | "execution" | "tradedesk" => {
                                                            menu = menu.separator().label("Exchange");
                                                            for exchange in exchange_choices() {
                                                                let pane_id = pane_id_cfg.clone();
                                                                let selected = current_exchange == exchange;
                                                                let exchange_id = exchange.clone();
                                                                let label = exchange.to_uppercase();
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new(label)
                                                                        .checked(selected)
                                                                        .on_click(move |_event, window, cx| {
                                                                            update_exchange_config(pane_id.clone(), exchange_id.clone(), window, cx);
                                                                        }),
                                                                );
                                                            }

                                                            menu = menu.label("Symbol");
                                                            for symbol in symbol_choices_for_menu(&current_exchange, &current_symbol) {
                                                                let pane_id = pane_id_cfg.clone();
                                                                let selected = current_symbol == symbol;
                                                                let value = symbol.clone();
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new(symbol)
                                                                        .checked(selected)
                                                                        .on_click(move |_event, window, cx| {
                                                                            update_pane_config(pane_id.clone(), "symbol", serde_json::Value::String(value.clone()), window, cx);
                                                                        }),
                                                                );
                                                            }
                                                        }
                                                        "chart" => {
                                                            menu = menu.separator().label("Exchange");
                                                            for exchange in exchange_choices() {
                                                                let pane_id = pane_id_cfg.clone();
                                                                let selected = current_exchange == exchange;
                                                                let exchange_id = exchange.clone();
                                                                let label = exchange.to_uppercase();
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new(label)
                                                                        .checked(selected)
                                                                        .on_click(move |_event, window, cx| {
                                                                            update_exchange_config(pane_id.clone(), exchange_id.clone(), window, cx);
                                                                        }),
                                                                );
                                                            }

                                                            menu = menu.label("Symbol");
                                                            for symbol in symbol_choices_for_menu(&current_exchange, &current_symbol) {
                                                                let pane_id = pane_id_cfg.clone();
                                                                let selected = current_symbol == symbol;
                                                                let value = symbol.clone();
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new(symbol)
                                                                        .checked(selected)
                                                                        .on_click(move |_event, window, cx| {
                                                                            update_pane_config(pane_id.clone(), "symbol", serde_json::Value::String(value.clone()), window, cx);
                                                                        }),
                                                                );
                                                            }

                                                            menu = menu.label("Type");
                                                            for (value, label) in CANDLE_TYPES {
                                                                let pane_id = pane_id_cfg.clone();
                                                                let selected = current_chart_type == value;
                                                                let value = value.to_string();
                                                                menu = menu.item(
                                                                    gpui_component::menu::PopupMenuItem::new(label)
                                                                        .checked(selected)
                                                                        .on_click(move |_event, window, cx| {
                                                                            update_pane_config(pane_id.clone(), "type", serde_json::Value::String(value.clone()), window, cx);
                                                                        }),
                                                                );
                                                            }
                                                        }
                                                        _ => {}
                                                    }
                                                    menu
                                                }
                                            }))
                                    .child(
                                        Button::new(ElementId::Name(
                                            format!("maximize-btn-{pane_id}").into(),
                                        ))
                                        .icon(
                                            if self.maximized_pane.as_ref() == Some(&pane_id) {
                                                gpui_component::Icon::default().path("icons/minimize.svg")
                                            } else {
                                                gpui_component::Icon::default().path("icons/maximize.svg")
                                            }
                                        )
                                        .ghost()
                                        .xsmall()
                                        .text_color(title_fg)
                                        .hover(|s| {
                                            if is_active {
                                                s.text_color(title_fg).bg(theme.colors.background.opacity(0.15))
                                            } else {
                                                s.text_color(theme.colors.foreground).bg(theme.colors.muted.opacity(0.4))
                                            }
                                        })
                                        .on_click({
                                            let p_id = pane_id.clone();
                                            move |_event, _window, cx| {
                                                cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(|global, cx| {
                                                    let _ = global.view.update(cx, |view, cx| {
                                                        view.handle_pane_event(
                                                            &PaneGridEvent::ToggleMaximize {
                                                                pane_id: p_id.clone(),
                                                            },
                                                            cx,
                                                        );
                                                    });
                                                });
                                            }
                                        })
                                    )
                                    .child(
                                        Button::new(ElementId::Name(
                                            format!("close-{close_id}").into(),
                                        ))
                                        .icon(
                                            gpui_component::Icon::default().path("icons/close.svg"),
                                        )
                                        .ghost()
                                        .xsmall()
                                        .text_color(title_fg)
                                        .hover(|s| s.text_color(theme.colors.bearish).bg(theme.colors.bearish.opacity(0.15)))
                                        .on_click(
                                            cx.listener(move |_this, _event, _win, cx| {
                                                cx.emit(PaneGridEvent::ClosePane {
                                                    pane_id: close_id.clone(),
                                                })
                                            }),
                                        ),
                                    ),
                            ),
                    )
                    // Content Area
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .min_h_0()
                            .relative()
                            .overflow_hidden()
                            .child(match module_type.to_lowercase().as_str() {
                                "chart" => {
                                    let config = data.config.as_object();
                                    let exchange = config_str(config, "exchange", "binance");
                                    let symbol = config_str(config, "symbol", "BTC/USDT");
                                    // Older FinTer layouts used `timeframe`; web layouts use `interval`.
                                    let timeframe = config_str(config, "interval", &config_str(config, "timeframe", "1m"));
                                    let chart_type = config_str(config, "type", "candle_solid");

                                    let mut chart_map = self.chart_views.borrow_mut();
                                    
                                    // 1. If only the chart_type changed, don't recreate the data bridge, just update the property.
                                    if let Some(existing) = chart_map.get_mut(&pane_id) {
                                        if existing.exchange == exchange && existing.symbol == symbol && existing.timeframe == timeframe && existing.chart_type != chart_type {
                                            existing.chart_type = chart_type.clone();
                                            let candle_type = candle_type_from_config(&chart_type);
                                            existing.entity.update(cx, |chart, _| {
                                                chart.set_candle_type(candle_type);
                                            });
                                        }
                                    }

                                    let needs_recreate = match chart_map.get(&pane_id) {
                                        Some(existing) => existing.exchange != exchange || existing.symbol != symbol || existing.timeframe != timeframe,
                                        None => true,
                                    };

                                    if needs_recreate {
                                        let mut bridge_config = crate::data::bridge::BridgeConfig::default_dev(&exchange, &symbol);
                                        bridge_config.timeframe = timeframe.clone();
                                        bridge_config.trades = false;
                                        bridge_config.orderbook = false;
                                        bridge_config.ticker = true;
                                        bridge_config.snapshot = Some(("ticker", 0, "Chart"));
                                        if timeframe == "1s" {
                                            bridge_config.fetch_history = false;
                                            bridge_config.kline = false;
                                            bridge_config.ticker = true;
                                        }
                                        if let Ok(Some(session)) = crate::data::session_store::load_session() {
                                            bridge_config.access_token = Some(session.access_token);
                                        }
                                        let bridge_config_clone = bridge_config.clone();
                                        match crate::data::bridge::Bridge::start(bridge_config) {
                                            Ok(bridge) => {
                                                let rx = bridge.subscribe_ohlcv();
                                                let tick_rx = bridge.subscribe();
                                                let feed = crate::data::candle_feed::CandleFeed::new(rx, Some(tick_rx), Some(timeframe.clone()));
                                                if !crate::data::client::is_api_url_configured() {
                                                    feed.set_error("No API service URL configured. Configure in Settings to stream data.".to_string());
                                                }
                                                let title = format!("{} {}", exchange.to_uppercase(), symbol);
                                                let tf = timeframe.clone();
                                                
                                                let bc = bridge_config_clone;
                                                let feed_clone = feed.clone();
                                                let load_more: std::sync::Arc<dyn Fn(i64, std::sync::mpsc::Sender<()>) + Send + Sync> = std::sync::Arc::new(move |until: i64, done: std::sync::mpsc::Sender<()>| {
                                                    let bc = bc.clone();
                                                    let feed = feed_clone.clone();
                                                    crate::data::client::tokio_runtime().spawn(async move {
                                                        match crate::data::bridge::fetch_ohlcv_history(&bc, Some(until)).await {
                                                            Ok(history) if !history.is_empty() => {
                                                                feed.prepend_history(&history);
                                                            }
                                                            Ok(_) => {
                                                                tracing::warn!("chart: historical request returned no candles through {}", until);
                                                            }
                                                            Err(e) => {
                                                                tracing::error!("chart: historical request failed through {}: {}", until, e);
                                                                feed.set_error(format!("{e}"));
                                                            }
                                                        }
                                                        let _ = done.send(());
                                                    });
                                                });
                                                
                                                let chart_panel = cx.new(|cx| {
                                                    let mut panel = crate::components::ui::chart_panel::ChartPanel::new(feed, title, tf, cx);
                                                    panel.on_load_more = Some(load_more);
                                                    panel.set_size(content_w, content_h);
                                                    panel
                                                });
                                                let timezone = self.timezone.clone();
                                                let candle_type = candle_type_from_config(&chart_type);
                                                chart_panel.update(cx, |chart, _| {
                                                    chart.set_context(exchange.clone(), symbol.clone());
                                                    chart.set_timezone(timezone);
                                                    chart.set_candle_type(candle_type);
                                                });
                                                let state = ChartViewState {
                                                    exchange: exchange.clone(),
                                                    symbol: symbol.clone(),
                                                    timeframe: timeframe.clone(),
                                                    chart_type: chart_type.clone(),
                                                    entity: chart_panel.clone(),
                                                    _bridge: std::sync::Arc::new(bridge),
                                                };
                                                chart_map.insert(pane_id.clone(), state);
                                                cached_panel(chart_panel)
                                            }
                                            Err(e) => {
                                                div().size_full().flex().items_center().justify_center().child(format!("Error starting chart: {e}")).into_any_element()
                                            }
                                        }
                                    } else {
                                        // Reuse the live feed; update layout only when the pane actually resizes.
                                        let ent = chart_map.get(&pane_id).unwrap().entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |chart, _| chart.set_size(content_w, content_h));
                                        }
                                        cached_panel(ent)
                                    }
                                }
                                "ticker" => {
                                    let config = data.config.as_object();
                                    let exchange = config_str(config, "exchange", "binance");
                                    let symbol = config_str(config, "symbol", "BTC/USDT");

                                    let mut ticker_map = self.ticker_views.borrow_mut();
                                    let needs_recreate = match ticker_map.get(&pane_id) {
                                        Some(existing) => existing.exchange != exchange || existing.symbol != symbol,
                                        None => true,
                                    };

                                    let panel = if needs_recreate {
                                        let panel = cx.new(|cx| {
                                            let mut p = crate::components::ui::ticker_panel::TickerPanel::new(&exchange, &symbol, cx);
                                            p.set_size(content_w, content_h);
                                            p
                                        });
                                        let state = TickerViewState {
                                            exchange: exchange.clone(),
                                            symbol: symbol.clone(),
                                            entity: panel.clone(),
                                        };
                                        ticker_map.insert(pane_id.clone(), state);
                                        panel
                                    } else {
                                        let ent = ticker_map.get(&pane_id).unwrap().entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        ent
                                    };
                                    cached_panel(panel)
                                }
                                "orderbook" => {
                                    let config = data.config.as_object();
                                    let exchange = config_str(config, "exchange", "binance");
                                    let symbol = config_str(config, "symbol", "BTC/USDT");
                                    let limit = config_usize(config, "limit", 20);

                                    let mut orderbook_map = self.orderbook_views.borrow_mut();
                                    let needs_recreate = match orderbook_map.get(&pane_id) {
                                        Some(existing) => existing.exchange != exchange || existing.symbol != symbol || existing.limit != limit,
                                        None => true,
                                    };

                                    if needs_recreate {
                                        let mut bridge_config = crate::data::bridge::BridgeConfig::default_dev(&exchange, &symbol);
                                        bridge_config.ticker = false;
                                        bridge_config.trades = false;
                                        bridge_config.kline = false;
                                        bridge_config.fetch_history = false;
                                        bridge_config.snapshot = Some(("orderbook", limit as u32, "OrderBook"));
                                        if let Ok(Some(session)) = crate::data::session_store::load_session() {
                                            bridge_config.access_token = Some(session.access_token);
                                        }
                                        match crate::data::bridge::Bridge::start(bridge_config) {
                                            Ok(bridge) => {
                                                let feed = crate::data::orderbook_feed::OrderBookFeed::new(bridge.subscribe_orderbook());
                                                let title = format!("{} {}", exchange.to_uppercase(), symbol);
                                                let panel = cx.new(|cx| {
                                                    let mut p = crate::components::ui::orderbook_panel::OrderBookPanel::new(feed, title, cx);
                                                    p.set_size(content_w, content_h);
                                                    p
                                                });
                                                let state = OrderBookViewState {
                                                    exchange: exchange.clone(),
                                                    symbol: symbol.clone(),
                                                    limit,
                                                    entity: panel.clone(),
                                                    _bridge: std::sync::Arc::new(bridge),
                                                };
                                                orderbook_map.insert(pane_id.clone(), state);
                                                cached_panel(panel)
                                            }
                                            Err(e) => div().size_full().flex().items_center().justify_center().child(format!("Error starting orderbook: {e}")).into_any_element(),
                                        }
                                    } else {
                                        let ent = orderbook_map.get(&pane_id).unwrap().entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        cached_panel(ent)
                                    }
                                }
                                "depth" | "agent" => {
                                    let mut agent_map = self.agent_views.borrow_mut();
                                    if !agent_map.contains_key(&pane_id) {
                                        let panel = cx.new(|cx| crate::components::ui::agent_panel::AgentPanel::new(cx));
                                        if let Some(context) = self.agent_context.clone() {
                                            panel.update(cx, |agent, _| agent.set_context(context));
                                        }
                                        agent_map.insert(pane_id.clone(), AgentViewState { entity: panel });
                                    }
                                    let ent = agent_map.get(&pane_id).unwrap().entity.clone();
                                    if !ent.read(cx).has_size(content_w, content_h) {
                                        ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                    }
                                    cached_panel(ent)
                                }
                                "trades" | "tape" => {
                                    let config = data.config.as_object();
                                    let exchange = config_str(config, "exchange", "binance");
                                    let symbol = config_str(config, "symbol", "BTC/USDT");
                                    let mut tape_map = self.tape_views.borrow_mut();
                                    let needs_recreate = match tape_map.get(&pane_id) {
                                        Some(existing) => existing.exchange != exchange || existing.symbol != symbol,
                                        None => true,
                                    };

                                    if needs_recreate {
                                        let mut bridge_config = crate::data::bridge::BridgeConfig::default_dev(&exchange, &symbol);
                                        bridge_config.ticker = false;
                                        bridge_config.orderbook = false;
                                        bridge_config.kline = false;
                                        bridge_config.fetch_history = false;
                                         bridge_config.snapshot = Some(("trades", 60, "Trades"));
                                        if let Ok(Some(session)) = crate::data::session_store::load_session() {
                                            bridge_config.access_token = Some(session.access_token);
                                        }
                                        match crate::data::bridge::Bridge::start(bridge_config) {
                                            Ok(bridge) => {
                                                let feed = crate::data::feed::TickFeed::new(bridge.subscribe_trades());
                                                let title = format!("{} {} Trades", exchange.to_uppercase(), symbol);
                                                let sound_alerts = self.sound_alerts_enabled;
                                                let panel = cx.new(|cx| {
                                                    let mut p = crate::components::ui::tape_panel::TapePanel::new(feed, title, cx);
                                                    p.set_size(content_w, content_h);
                                                    p.set_global_sound_enabled(sound_alerts);
                                                    p
                                                });
                                                tape_map.insert(pane_id.clone(), TapeViewState {
                                                    exchange: exchange.clone(),
                                                    symbol: symbol.clone(),
                                                    entity: panel.clone(),
                                                    _bridge: std::sync::Arc::new(bridge),
                                                });
                                                cached_panel(panel)
                                            }
                                            Err(e) => div().size_full().flex().items_center().justify_center().child(format!("Error starting trades: {e}")).into_any_element(),
                                        }
                                    } else {
                                        let ent = tape_map.get(&pane_id).unwrap().entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        cached_panel(ent)
                                    }
                                }
                                "positions" => {
                                    let mut positions_map = self.positions_views.borrow_mut();
                                    let panel = if let Some(entity) = positions_map.get(&pane_id) {
                                        if !entity.read(cx).has_size(content_w, content_h) {
                                            entity.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        entity.clone()
                                    } else {
                                        let entity = cx.new(|cx| {
                                            let mut p = crate::components::ui::positions_panel::PositionsPanel::new(
                                                Vec::new(), "Positions", cx,
                                            );
                                            p.set_size(content_w, content_h);
                                            p
                                        });
                                        positions_map.insert(pane_id.clone(), entity.clone());
                                        entity
                                    };
                                    cached_panel(panel)
                                }
                                "execution" | "tradedesk" => {
                                    let config = data.config.as_object();
                                    let exchange = config_str(config, "exchange", "binance");
                                    let symbol = config_str(config, "symbol", "BTC/USDT");
                                    let live_mode = config_bool(config, "live_mode", true);
                                    let mut execution_map = self.execution_views.borrow_mut();
                                    let needs_recreate = match execution_map.get(&pane_id) {
                                        Some(existing) => existing.exchange != exchange || existing.symbol != symbol,
                                        None => true,
                                    };
                                    if needs_recreate {
                                        let mut bridge_config = crate::data::bridge::BridgeConfig::default_dev(&exchange, &symbol);
                                        bridge_config.trades = false;
                                        bridge_config.orderbook = false;
                                        bridge_config.kline = false;
                                        bridge_config.fetch_history = false;
                                        bridge_config.snapshot = Some(("ticker", 0, "Trade Desk"));
                                        if let Ok(Some(session)) = crate::data::session_store::load_session() {
                                            bridge_config.access_token = Some(session.access_token);
                                        }
                                        match crate::data::bridge::Bridge::start(bridge_config) {
                                            Ok(bridge) => {
                                                let feed = crate::data::feed::TickFeed::new(bridge.subscribe());
                                                let title = format!("{} {} · Trade Desk", exchange.to_uppercase(), symbol);
                                                let panel = cx.new(|cx| {
                                                    let mut p = crate::components::ui::execution_panel::ExecutionPanel::new(feed, title, symbol.clone(), cx);
                                                    p.set_size(content_w, content_h);
                                                    p.set_pane_id(pane_id.clone());
                                                    p.set_exchange(exchange.clone());
                                                    p.set_live_mode(live_mode);
                                                    p
                                                });
                                                execution_map.insert(pane_id.clone(), ExecutionViewState {
                                                    exchange, symbol, entity: panel.clone(), _bridge: std::sync::Arc::new(bridge),
                                                });
                                                cached_panel(panel)
                                            }
                                            Err(e) => div().size_full().flex().items_center().justify_center().child(format!("Error starting trade desk price feed: {e}")).into_any_element(),
                                        }
                                    } else {
                                        let entity = execution_map.get(&pane_id).unwrap().entity.clone();
                                        entity.update(cx, |panel, cx| {
                                            if !panel.has_size(content_w, content_h) {
                                                panel.set_size(content_w, content_h);
                                                cx.notify();
                                            }
                                            panel.set_pane_id(pane_id.clone());
                                            panel.set_exchange(exchange.clone());
                                        });
                                        cached_panel(entity)
                                    }
                                }
                                "watchlist" => {
                                    let mut watchlist_map = self.watchlist_views.borrow_mut();
                                    let panel = if let Some(existing) = watchlist_map.get(&pane_id) {
                                        let ent = existing.entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        ent
                                    } else {
                                        let config = data.config.as_object();
                                        let market = config.and_then(|c| {
                                            let exchange = c.get("exchange")?.as_str()?;
                                            let symbol = c.get("symbol")?.as_str()?;
                                            Some((exchange, symbol))
                                        });
                                        let entity = cx.new(|cx| {
                                            let mut p = crate::components::ui::watchlist_panel::WatchlistPanel::new_with_market("Watchlist", market, cx);
                                            p.set_size(content_w, content_h);
                                            p
                                        });
                                        watchlist_map.insert(pane_id.clone(), WatchlistViewState { entity: entity.clone() });
                                        entity
                                    };
                                    cached_panel(panel)
                                }
                                "marketcap" => {
                                    let mut market_cap_map = self.market_cap_views.borrow_mut();
                                    let panel = if let Some(existing) = market_cap_map.get(&pane_id) {
                                        let ent = existing.entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        ent
                                    } else {
                                        let entity = cx.new(|cx| {
                                            let mut p = crate::components::ui::market_cap_panel::MarketCapPanel::new("Market Cap", cx);
                                            p.set_size(content_w, content_h);
                                            p
                                        });
                                        market_cap_map.insert(pane_id.clone(), MarketCapViewState { entity: entity.clone() });
                                        entity
                                    };
                                    cached_panel(panel)
                                }
                                "feargreed" => {
                                    let mut fg_map = self.fear_greed_views.borrow_mut();
                                    let panel = if !fg_map.contains_key(id) {
                                        let entity = cx.new(|cx| {
                                            let mut p = crate::components::ui::fear_greed_panel::FearGreedPanel::new(cx);
                                            p.set_size(content_w, content_h);
                                            p
                                        });
                                        fg_map.insert(id.clone(), FearGreedViewState { entity: entity.clone() });
                                        entity
                                    } else {
                                        let ent = fg_map.get(id).unwrap().entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            ent.update(cx, |p, _| p.set_size(content_w, content_h));
                                        }
                                        ent
                                    };
                                    cached_panel(panel)
                                }
                                "newsfeed" => {
                                    let config = data.config.as_object();
                                    let priority = config.and_then(|c| c.get("priority")).and_then(|v| v.as_str()).unwrap_or("latest").to_string();
                                    let filter = config.and_then(|c| c.get("filter")).and_then(|v| v.as_str()).unwrap_or("").to_string();

                                    let mut newsfeed_map = self.newsfeed_views.borrow_mut();
                                    let needs_recreate = match newsfeed_map.get(id) {
                                        Some(existing) => existing.priority != priority || existing.filter != filter,
                                        None => true,
                                    };

                                    let panel = if needs_recreate {
                                        let p_id = id.clone();
                                        let p = priority.clone();
                                        let f = filter.clone();
                                        let entity = cx.new(|cx| {
                                            let mut panel = crate::components::ui::newsfeed_panel::NewsfeedPanel::new(p_id, p, f, cx);
                                            panel.set_size(content_w, content_h);
                                            panel
                                        });
                                        let timezone = self.timezone.clone();
                                        entity.update(cx, |panel, _| panel.set_timezone(timezone));
                                        let state = NewsfeedViewState { priority, filter, entity: entity.clone() };
                                        newsfeed_map.insert(id.clone(), state);
                                        entity
                                    } else {
                                        let ent = newsfeed_map.get(id).unwrap().entity.clone();
                                        if !ent.read(cx).has_size(content_w, content_h) {
                                            // A maximize transition changes the pane dimensions
                                            // outside the newsfeed entity. Notify it explicitly
                                            // so its header relayouts immediately instead of
                                            // waiting for a subsequent drag interaction.
                                            ent.update(cx, |p, cx| {
                                                p.set_size(content_w, content_h);
                                                cx.notify();
                                            });
                                        }
                                        ent
                                    };
                                    cached_panel(panel)
                                }
                                "time" | "clock" => {
                                    let config = data.config.as_object();
                                    let tz_str = config
                                        .and_then(|c| c.get("timezone"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(&self.timezone)
                                        .to_string();

                                    let mut clock_map = self.clock_views.borrow_mut();
                                    let needs_recreate = !clock_map.contains_key(id);

                                    let panel = if needs_recreate {
                                        let p_id = id.clone();
                                        let timezone = tz_str.clone();
                                        let entity = cx.new(|cx| {
                                            let mut p = crate::components::ui::clock_panel::ClockPanel::new(p_id, timezone.clone(), cx);
                                            p.set_size(content_w, content_h);
                                            p
                                        });
                                        let state = ClockViewState { timezone, entity: entity.clone() };
                                        clock_map.insert(id.clone(), state);
                                        entity
                                    } else {
                                        let state = clock_map.get_mut(id).unwrap();
                                        let ent = state.entity.clone();
                                        // Keep the entity synchronized with the pane config. This is
                                        // intentionally done independently of size updates so a
                                        // timezone change is visible without rebuilding the clock.
                                        // Always push the pane's configured timezone into the
                                        // cached entity. The pane tree is the source of truth;
                                        // relying only on the cached state can leave the clock
                                        // showing the previous timezone after a menu selection.
                                        let needs_resize = !ent.read(cx).has_size(content_w, content_h);
                                        state.timezone = tz_str.clone();
                                        ent.update(cx, |p, _| {
                                            p.set_timezone(tz_str.clone());
                                            if needs_resize {
                                                p.set_size(content_w, content_h);
                                            }
                                        });
                                        ent
                                    };
                                    cached_panel(panel)
                                }
                                _ => {
                                    if is_empty {
                                        self.render_placeholder_selector(&pane_id, content_w, content_h, cx)
                                    } else {
                                        div()
                                            .size_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(module_type.to_string())
                                            .into_any_element()
                                    }
                                }
                            })
                            // Unconditionally render drop zones so they are always in the hit-test tree and stay in front of content
                            .when(self.dragged_pane.borrow().is_some(), |this| this.map(|this| {
                                let c_id = pane_id.clone();
                                let t1 = pane_id.clone();
                                let t2 = pane_id.clone();
                                let t3 = pane_id.clone();
                                let t4 = pane_id.clone();
                                this
                                    .occlude()
                                // Center (Swap)
                                .child(
                                    div()
                                        .absolute()
                                        .top(relative(0.2))
                                        .left(relative(0.2))
                                        .w(relative(0.6))
                                        .h(relative(0.6))
                                        .drag_over::<DragPane>(|style, _drag, _win, _cx| style.bg(gpui::rgba(0x4ade8033))) // Green highlight
                                                .on_drop(cx.listener(
                                            move |_this, drag_pane: &DragPane, _window, cx| {
                                                if drag_pane.0 != c_id {
                                                    cx.emit(PaneGridEvent::SwapPanes {
                                                        id_a: drag_pane.0.clone(),
                                                        id_b: c_id.clone(),
                                                    });
                                                }
                                            },
                                        ))
                                        .occlude(),
                                )
                                // Top
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .w_full()
                                        .h(relative(0.2))
                                        .drag_over::<DragPane>(|style, _drag, _win, _cx| style.bg(gpui::rgba(0x3b82f633))) // Blue highlight
                                        .on_drop(cx.listener(
                                            move |_this, drag_pane: &DragPane, _window, cx| {
                                                if drag_pane.0 != t1 {
                                                    cx.emit(PaneGridEvent::DropSplit {
                                                        target_pane_id: t1.clone(),
                                                        dragged_pane_id: drag_pane.0.clone(),
                                                        region: "top".to_string(),
                                                    });
                                                }
                                            },
                                        )),
                                )
                                // Bottom
                                .child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .left_0()
                                        .w_full()
                                        .h(relative(0.2))
                                        .drag_over::<DragPane>(|style, _drag, _win, _cx| style.bg(gpui::rgba(0x3b82f633)))
                                        .on_drop(cx.listener(
                                            move |_this, drag_pane: &DragPane, _window, cx| {
                                                if drag_pane.0 != t2 {
                                                    cx.emit(PaneGridEvent::DropSplit {
                                                        target_pane_id: t2.clone(),
                                                        dragged_pane_id: drag_pane.0.clone(),
                                                        region: "bottom".to_string(),
                                                    });
                                                }
                                            },
                                        )),
                                )
                                // Left
                                .child(
                                    div()
                                        .absolute()
                                        .top(relative(0.2))
                                        .left_0()
                                        .w(relative(0.2))
                                        .h(relative(0.6))
                                        .drag_over::<DragPane>(|style, _drag, _win, _cx| style.bg(gpui::rgba(0x3b82f633)))
                                        .on_drop(cx.listener(
                                            move |_this, drag_pane: &DragPane, _window, cx| {
                                                if drag_pane.0 != t3 {
                                                    cx.emit(PaneGridEvent::DropSplit {
                                                        target_pane_id: t3.clone(),
                                                        dragged_pane_id: drag_pane.0.clone(),
                                                        region: "left".to_string(),
                                                    });
                                                }
                                            },
                                        )),
                                )
                                // Right
                                .child(
                                    div()
                                        .absolute()
                                        .top(relative(0.2))
                                        .right_0()
                                        .w(relative(0.2))
                                        .h(relative(0.6))
                                        .drag_over::<DragPane>(|style, _drag, _win, _cx| style.bg(gpui::rgba(0x3b82f633)))
                                        .on_drop(cx.listener(
                                            move |_this, drag_pane: &DragPane, _window, cx| {
                                                if drag_pane.0 != t4 {
                                                    cx.emit(PaneGridEvent::DropSplit {
                                                        target_pane_id: t4.clone(),
                                                        dragged_pane_id: drag_pane.0.clone(),
                                                        region: "right".to_string(),
                                                    });
                                                }
                                            },
                                        )),
                                )
                            }))
                    )
                    )
                    }) // close when
                    .into_any_element()
            }
            PaneNode::Split {
                id, axis, ratio, a, b, ..
            } => {
                let is_horizontal = *axis == Axis::Horizontal; // Horizontal divider -> top/bottom

                let ratio_a = *ratio;
                let ratio_b = 1.0 - ratio_a;

                let (size_a, size_b, total_pixels) = if is_horizontal {
                    (
                        gpui::Size { width: current_size.width, height: current_size.height * ratio_a },
                        gpui::Size { width: current_size.width, height: current_size.height * ratio_b },
                        current_size.height
                    )
                } else {
                    (
                        gpui::Size { width: current_size.width * ratio_a, height: current_size.height },
                        gpui::Size { width: current_size.width * ratio_b, height: current_size.height },
                        current_size.width
                    )
                };

                let split_id = id.clone();
                let split_axis = axis.clone();
                let current_ratio = *ratio;

                div()
                    .size_full()
                    .flex()
                    .when(is_horizontal, |this| this.flex_col())
                    .when(!is_horizontal, |this| this.flex_row())
                    .child(
                        div()
                            .flex_basis(relative(ratio_a))
                            .flex_grow()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(self.render_node(a, size_a, cx)),
                    )
                    .child(
                        // Divider - invisible but draggable
                        div()
                            .bg(gpui::transparent_black())
                            .when(is_horizontal, |this| {
                                this.h(px(8.0)).w_full().cursor_row_resize().my(px(-4.0))
                            })
                            .when(!is_horizontal, |this| {
                                this.w(px(8.0)).h_full().cursor_col_resize().mx(px(-4.0))
                            })
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _win, _cx| {
                                this.resizing = Some(ResizingState {
                                    split_id: split_id.clone(),
                                    axis: split_axis.clone(),
                                    initial_ratio: current_ratio,
                                    initial_mouse_pos: ev.position,
                                    container_size: total_pixels,
                                });
                            }))
                    )
                    .child(
                        div()
                            .flex_basis(relative(ratio_b))
                            .flex_grow()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(self.render_node(b, size_b, cx)),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for PaneGridRenderer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let viewport = window.viewport_size();
        let current_size = gpui::Size { width: f32::from(viewport.width), height: f32::from(viewport.height) };

        if let Some(tree) = &self.tree {
            let mut root = div().size_full().bg(theme.colors.background).p(px(4.0));

            // The root padding remains present while maximized, so pass the
            // usable inner dimensions to the pane. Passing the raw viewport
            // makes the pane's flex measurement stale until a later drag
            // forces a new layout pass.
            let root_size = gpui::Size {
                width: (current_size.width - 8.0).max(0.0),
                height: (current_size.height - 8.0).max(0.0),
            };
            
            // If a pane is maximized, we want to extract just that pane node
            // and render it using the root's usable dimensions
            if let Some(max_id) = &self.maximized_pane {
                let mut max_node = None;
                tree.find_node(max_id, &mut |node| {
                    max_node = Some(node.clone());
                });
                if let Some(node) = max_node {
                    return root
                        .child(
                            div()
                                .size_full()
                                .w_full()
                                .min_w_0()
                                .overflow_hidden()
                                .flex_none()
                                .child(self.render_node(&node, root_size, cx)),
                        )
                        .into_any_element();
                }
            }
            
            if self.resizing.is_some() {
                root = root
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _win, cx| {
                        if let Some(state) = &this.resizing {
                            let delta = if state.axis == Axis::Horizontal {
                                f32::from(ev.position.y) - f32::from(state.initial_mouse_pos.y)
                            } else {
                                f32::from(ev.position.x) - f32::from(state.initial_mouse_pos.x)
                            };
                            
                            let delta_ratio = delta / state.container_size;
                            let new_ratio = (state.initial_ratio + delta_ratio).clamp(0.1, 0.9);
                            cx.emit(PaneGridEvent::ResizeSplit {
                                split_id: state.split_id.clone(),
                                ratio: new_ratio,
                            });
                        }
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(move |this, _ev, _win, cx| {
                        if this.resizing.is_some() {
                            this.resizing = None;
                            cx.emit(PaneGridEvent::EndResize);
                        }
                    }));
            }
            
            // Each module is its own entity. Render those entities through GPUI's
            // retained-view cache so an update in one live module does not rebuild
            // and repaint every idle sibling in a populated dashboard.
            root.child(self.render_node(tree, current_size, cx)).into_any_element()
        } else {
            div()
                .size_full()
                .bg(theme.colors.background)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_color(theme.colors.muted_foreground)
                        .child("No layout loaded"),
                ).into_any_element()
        }
    }
}

impl Default for PaneGridRenderer {
    fn default() -> Self {
        Self::new()
    }
}
