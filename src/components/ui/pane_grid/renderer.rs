use crate::data::pane_tree::{Axis, PaneNode};
use gpui::InteractiveElement;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui::StyledImage;
use gpui_component::{Selectable, Sizable};
use gpui_component::scroll::ScrollableElement;
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
    pub newsfeed_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, NewsfeedViewState>>>,
    pub clock_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, ClockViewState>>>,
    pub fear_greed_views: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, FearGreedViewState>>>,
    pub timezone: String,
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

fn candle_type_from_config(value: &str) -> gpui_component::chart::CandleType {
    match value {
        "candle_stroke" => gpui_component::chart::CandleType::Stroke,
        "candle_up_stroke" => gpui_component::chart::CandleType::UpStroke,
        "candle_down_stroke" => gpui_component::chart::CandleType::DownStroke,
        "ohlc" => gpui_component::chart::CandleType::Ohlc,
        "area" => gpui_component::chart::CandleType::Area,
        "heikin_ashi" => gpui_component::chart::CandleType::HeikinAshi,
        "candle_solid" | _ => gpui_component::chart::CandleType::Solid,
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
            newsfeed_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            clock_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            fear_greed_views: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            timezone: "UTC".into(),
        }
    }

    pub fn set_tree(&mut self, tree: PaneNode, cx: &mut Context<Self>) {
        self.tree = Some(tree);
        cx.notify();
    }


    fn render_node(&self, node: &PaneNode, current_size: gpui::Size<f32>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match node {
            PaneNode::Pane { id, data } => {
                let theme = cx.theme().clone();
                let module_type = data.module_type.as_deref().unwrap_or("Empty");

                let is_empty = module_type == "Empty" || module_type.is_empty();
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
                    "chart" | "ticker" | "orderbook" | "depth"
                );
                let suffix = match module_type.to_lowercase().as_str() {
                    "orderbook" => Some("OB"),
                    "depth" => Some("DEPTH"),
                    _ => None,
                };

                div()
                    .id(ElementId::Name(format!("pane-wrapper-{pane_id}").into()))
                    .size_full()
                    .p(px(4.0))
                    .when(is_dragged, |this| {
                        this.child(
                            div()
                                .size_full()
                                .border_2()
                                .border_dashed()
                                .border_color(theme.colors.border.opacity(0.5))
                                .bg(theme.colors.muted.opacity(0.2))
                                .rounded_sm(),
                        )
                    })
                    .when(!is_dragged, |this| {
                        this.child(
                            div()
                                .id(ElementId::Name(format!("pane-container-{pane_id}").into()))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |_this, _event, _win, cx| {
                                    cx.emit(PaneGridEvent::SetActivePane {
                                        pane_id: click_id.clone(),
                                    });
                                }),
                            )
                            .size_full()
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
                            .w_full()
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
                                        "orderbook" | "depth" => self.orderbook_views.borrow().get(&pane_id).map(|s| DragContent::Orderbook(s.entity.clone())),
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
                                                        .rounded(px(3.0))
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
                                                    if is_empty {
                                                        "Select Module".to_string()
                                                    } else if module_type.to_lowercase() == "time" || module_type.to_lowercase() == "clock" {
                                                        current_timezone.replace('/', ", ").replace('_', " ")
                                                    } else if module_type.to_lowercase() == "marketcap" {
                                                        "Market Cap".to_string()
                                                    } else if module_type.to_lowercase() == "feargreed" {
                                                        "Fear & Greed Index".to_string()
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
                                        let common_timeframes = ["1m", "5m", "15m", "1h", "4h", "1d"];
                                        let extra_timeframes = [
                                            "1s", "3m", "30m", "2h", "6h", "8h", "12h", "1w", "1M",
                                        ];
                                        let current_timeframe = chart_timeframe.clone();
                                        let pane_id = pane_id.clone();
                                        let mut controls = gpui_component::h_flex()
                                            .flex_shrink_0()
                                            .gap(px(1.0))
                                            .items_center();

                                        for timeframe in common_timeframes {
                                            let pane_id = pane_id.clone();
                                            let selected = current_timeframe == timeframe;
                                            controls = controls.child(
                                                Button::new(ElementId::Name(
                                                    format!("timeframe-{pane_id}-{timeframe}").into(),
                                                ))
                                                .label(timeframe)
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
                                                gpui::Corner::BottomLeft,
                                                move |mut menu, _window, _cx| {
                                                    for timeframe in extra_timeframes {
                                                        let pane_id = pane_id.clone();
                                                        let selected = current_timeframe == timeframe;
                                                        menu = menu.item(
                                                            gpui_component::menu::PopupMenuItem::new(timeframe)
                                                                .checked(selected)
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
                                                    menu
                                                },
                                            ),
                                        )
                                        ))
                                    }),
                            )
                            // Toolbar
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
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
                                                move |mut menu, _window, _cx| {
                                                    let module_types = [
                                                        ("time", "Time"),
                                                        ("ticker", "Ticker"),
                                                        ("chart", "Chart"),
                                                        ("orderbook", "Orderbook"),
                                                        ("depth", "Depth"),
                                                        ("marketcap", "Market Cap"),
                                                        ("feargreed", "Fear and Greed Index"),
                                                        ("editor", "Editor"),
                                                        ("newsfeed", "Newsfeed"),
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
                                                        "ticker" | "orderbook" | "depth" => {
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
                                                            for symbol in symbol_choices(&current_exchange) {
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
                                                            for symbol in symbol_choices(&current_exchange) {
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
                                                        _ => {
                                                            menu = menu.separator().label("Settings");
                                                            menu = menu.item(
                                                                gpui_component::menu::PopupMenuItem::new("No configuration options available")
                                                                    .disabled(true),
                                                            );
                                                        }
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
                                    let timeframe = config_str(config, "interval", "1m");
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
                                        if let Ok(Some(session)) = crate::data::session_store::load_session() {
                                            bridge_config.access_token = Some(session.access_token);
                                        }
                                        let bridge_config_clone = bridge_config.clone();
                                        match crate::data::bridge::Bridge::start(bridge_config) {
                                            Ok(bridge) => {
                                                let rx = bridge.subscribe_ohlcv();
                                                let tick_rx = bridge.subscribe();
                                                let feed = crate::data::candle_feed::CandleFeed::new(rx, Some(tick_rx), Some(timeframe.clone()));
                                                let title = format!("{} {}", exchange.to_uppercase(), symbol);
                                                let tf = timeframe.clone();
                                                
                                                let bc = bridge_config_clone;
                                                let feed_clone = feed.clone();
                                                let load_more: std::sync::Arc<dyn Fn(i64) + Send + Sync> = std::sync::Arc::new(move |until: i64| {
                                                    let bc = bc.clone();
                                                    let feed = feed_clone.clone();
                                                    crate::data::client::tokio_runtime().spawn(async move {
                                                        if let Ok(history) = crate::data::bridge::fetch_ohlcv_history(&bc, Some(until)).await {
                                                            feed.prepend_history(&history);
                                                        }
                                                    });
                                                });
                                                
                                                let chart_panel = cx.new(|cx| {
                                                    let mut panel = crate::components::ui::chart_panel::ChartPanel::new(feed, title, tf, cx);
                                                    panel.on_load_more = Some(load_more);
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
                                                chart_panel.into_any_element()
                                            }
                                            Err(e) => {
                                                div().size_full().flex().items_center().justify_center().child(format!("Error starting chart: {e}")).into_any_element()
                                            }
                                        }
                                    } else {
                                        chart_map.get(&pane_id).unwrap().entity.clone().into_any_element()
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

                                    if needs_recreate {
                                        let panel = cx.new(|cx| crate::components::ui::ticker_panel::TickerPanel::new(&exchange, &symbol, cx));
                                        let state = TickerViewState {
                                            exchange: exchange.clone(),
                                            symbol: symbol.clone(),
                                            entity: panel.clone(),
                                        };
                                        ticker_map.insert(pane_id.clone(), state);
                                        panel.into_any_element()
                                    } else {
                                        ticker_map.get(&pane_id).unwrap().entity.clone().into_any_element()
                                    }
                                }
                                "orderbook" | "depth" => {
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
                                        if let Ok(Some(session)) = crate::data::session_store::load_session() {
                                            bridge_config.access_token = Some(session.access_token);
                                        }
                                        match crate::data::bridge::Bridge::start(bridge_config) {
                                            Ok(bridge) => {
                                                let feed = crate::data::orderbook_feed::OrderBookFeed::new(bridge.subscribe_orderbook());
                                                let title = format!("{} {}", exchange.to_uppercase(), symbol);
                                                let panel = cx.new(|cx| crate::components::ui::orderbook_panel::OrderBookPanel::new(feed, title, cx));
                                                let state = OrderBookViewState {
                                                    exchange: exchange.clone(),
                                                    symbol: symbol.clone(),
                                                    limit,
                                                    entity: panel.clone(),
                                                    _bridge: std::sync::Arc::new(bridge),
                                                };
                                                orderbook_map.insert(pane_id.clone(), state);
                                                panel.into_any_element()
                                            }
                                            Err(e) => div().size_full().flex().items_center().justify_center().child(format!("Error starting orderbook: {e}")).into_any_element(),
                                        }
                                    } else {
                                        orderbook_map.get(&pane_id).unwrap().entity.clone().into_any_element()
                                    }
                                }
                                "tape" => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("Tape Module (Rendered)")
                                    .into_any_element(),
                                "positions" => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("Positions Module (Rendered)")
                                    .into_any_element(),
                                "watchlist" => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child("Watchlist Module (Rendered)")
                                    .into_any_element(),
                                "feargreed" => {
                                    let mut fg_map = self.fear_greed_views.borrow_mut();
                                    if !fg_map.contains_key(id) {
                                        let entity = cx.new(|cx| crate::components::ui::fear_greed_panel::FearGreedPanel::new(cx));
                                        fg_map.insert(id.clone(), FearGreedViewState { entity: entity.clone() });
                                        entity.into_any_element()
                                    } else {
                                        fg_map.get(id).unwrap().entity.clone().into_any_element()
                                    }
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

                                    if needs_recreate {
                                        let p_id = id.clone();
                                        let p = priority.clone();
                                        let f = filter.clone();
                                        let entity = cx.new(|cx| crate::components::ui::newsfeed_panel::NewsfeedPanel::new(p_id, p, f, cx));
                                        let timezone = self.timezone.clone();
                                        entity.update(cx, |panel, _| panel.set_timezone(timezone));
                                        let state = NewsfeedViewState { priority, filter, entity: entity.clone() };
                                        newsfeed_map.insert(id.clone(), state);
                                        entity.into_any_element()
                                    } else {
                                        newsfeed_map.get(id).unwrap().entity.clone().into_any_element()
                                    }
                                }
                                "time" | "clock" => {
                                    let config = data.config.as_object();
                                    let tz_str = config
                                        .and_then(|c| c.get("timezone"))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or(&self.timezone)
                                        .to_string();

                                    let mut clock_map = self.clock_views.borrow_mut();
                                    let needs_recreate = match clock_map.get(id) {
                                        Some(existing) => existing.timezone != tz_str,
                                        None => true,
                                    };

                                    if needs_recreate {
                                        let p_id = id.clone();
                                        let timezone = tz_str.clone();
                                        let entity = cx.new(|cx| crate::components::ui::clock_panel::ClockPanel::new(p_id, timezone.clone(), cx));
                                        let state = ClockViewState { timezone, entity: entity.clone() };
                                        clock_map.insert(id.clone(), state);
                                        entity.into_any_element()
                                    } else {
                                        clock_map.get(id).unwrap().entity.clone().into_any_element()
                                    }
                                }
                                _ => div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(if is_empty { "Empty module".to_string() } else { module_type.to_string() })
                                    .into_any_element(),
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
            
            // If a pane is maximized, we want to extract just that pane node
            // and render it using current_size
            if let Some(max_id) = &self.maximized_pane {
                let mut max_node = None;
                tree.find_node(max_id, &mut |node| {
                    max_node = Some(node.clone());
                });
                if let Some(node) = max_node {
                    return root.child(self.render_node(&node, current_size, cx)).into_any_element();
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
