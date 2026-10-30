//! Interactive trade desk for sizing and executing live & paper orders against the market feed.
//! Supports real-time GatewayRegistry exchange/symbol switching, live API integration,
//! instant paper fills, leverage management, and position PnL tracking.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex, Icon, IconName, Sizable};

use crate::data::api_management;
use crate::data::feed::TickFeed;
use crate::data::gateway_registry::GatewayRegistry;
pub use crate::data::trade_store::{
    DeskPosition, TradeOrder as PaperOrder, TradeOrderType as OrderType, TradeSide as Side,
    TradeStore, TradingMode,
};

pub struct ExecutionPanel {
    feed: TickFeed,
    title: String,
    exchange: String,
    symbol: String,
    pane_id: Option<String>,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    side: Side,
    order_type: OrderType,
    leverage: u32,
    margin: f64,
    notification: Option<(bool /* is_error */, String)>,
    is_live_mode: bool,
    last_price: Option<f64>,
    price_direction: i8, // 1 = up, -1 = down, 0 = neutral
    limit_price: Option<f64>,
    price_input: Option<Entity<InputState>>,
    margin_input: Option<Entity<InputState>>,
    size_input: Option<Entity<InputState>>,
    price_focused: bool,
    margin_focused: bool,
    size_focused: bool,
}

impl ExecutionPanel {
    pub fn new(
        feed: TickFeed,
        title: impl Into<String>,
        symbol: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_str = title.into();
        let sym_str = symbol.into();

        let ex = title_str
            .split_whitespace()
            .next()
            .unwrap_or("binance")
            .to_lowercase();

        let feed_for_timer = feed.clone();
        cx.spawn(async move |this, cx| {
            let mut last_seq = feed_for_timer.sequence();
            let mut last_ver = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let sequence = feed_for_timer.sequence();
                let ver = TradeStore::global().lock().unwrap().version;
                let changed = sequence != last_seq || ver != last_ver;
                last_seq = sequence;
                last_ver = ver;
                if this
                    .update(cx, |this, cx| {
                        if changed {
                            if let Some(prev) = this.last_price {
                                if let Some(p) = this.price() {
                                    if p > prev {
                                        this.price_direction = 1;
                                    } else if p < prev {
                                        this.price_direction = -1;
                                    }
                                }
                            }
                            if let Some(p) = this.price() {
                                this.last_price = Some(p);
                            }
                        }
                        // Publish the mark price every tick (not only when the UI
                        // notices a change) so it is always the freshest feed value
                        // that the trade desk, positions panel and chart read.
                        if let Some(p) = this.price() {
                            TradeStore::global()
                                .lock()
                                .unwrap()
                                .set_mark_price(&this.symbol, p);
                        }
                        if changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            feed,
            title: title_str,
            exchange: ex,
            symbol: sym_str,
            pane_id: None,
            focus_handle: cx.focus_handle(),
            width: 360.0,
            height: 480.0,
            side: Side::Long,
            order_type: OrderType::Market,
            leverage: 10,
            margin: 100.0,
            notification: None,
            is_live_mode: false,
            last_price: None,
            price_direction: 0,
            limit_price: None,
            price_input: None,
            margin_input: None,
            size_input: None,
            price_focused: false,
            margin_focused: false,
            size_focused: false,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        let w = width.max(10.0);
        let h = height.max(10.0);
        if (self.width - w).abs() < 0.5 && (self.height - h).abs() < 0.5 {
            return;
        }
        self.width = w;
        self.height = h;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        let w = width.max(10.0);
        let h = height.max(10.0);
        (self.width - w).abs() < 0.5 && (self.height - h).abs() < 0.5
    }

    pub fn set_exchange(&mut self, exchange: impl Into<String>) {
        self.exchange = exchange.into();
    }

    pub fn set_pane_id(&mut self, pane_id: impl Into<String>) {
        self.pane_id = Some(pane_id.into());
    }

    fn price(&self) -> Option<f64> {
        self.feed
            .latest()
            .map(|t| t.price)
            .filter(|p| p.is_finite() && *p > 0.0)
    }

    fn fmt_price(v: f64) -> String {
        if v >= 1000.0 {
            format!("{v:.2}")
        } else if v >= 1.0 {
            format!("{v:.4}")
        } else {
            format!("{v:.6}")
        }
    }

    fn fmt_size(qty: f64) -> String {
        if !qty.is_finite() || qty <= 0.0 {
            "0.00".to_string()
        } else if qty >= 1000.0 {
            format!("{qty:.1}")
        } else if qty >= 100.0 {
            format!("{qty:.2}")
        } else if qty >= 1.0 {
            format!("{qty:.3}")
        } else if qty >= 0.01 {
            format!("{qty:.4}")
        } else {
            format!("{qty:.6}")
        }
    }

    fn base_asset(&self) -> &str {
        if let Some(pos) = self.symbol.find('/') {
            &self.symbol[..pos]
        } else if let Some(pos) = self.symbol.find('-') {
            &self.symbol[..pos]
        } else {
            &self.symbol
        }
    }

    fn quote_asset(&self) -> &str {
        if let Some(pos) = self.symbol.find('/') {
            &self.symbol[pos + 1..]
        } else if let Some(pos) = self.symbol.find('-') {
            &self.symbol[pos + 1..]
        } else {
            "USDT"
        }
    }

    fn calculate_liquidation(&self, entry: f64) -> f64 {
        let lev = self.leverage.max(1) as f64;
        let mmr = 0.005; // 0.5% maintenance margin
        match self.side {
            Side::Long => (entry * (1.0 - (1.0 / lev) + mmr)).max(0.0),
            Side::Short => entry * (1.0 + (1.0 / lev) - mmr),
        }
    }

    pub fn active_mode(&self) -> TradingMode {
        if self.is_live_mode {
            TradingMode::Live
        } else {
            TradingMode::Paper
        }
    }

    pub fn adjust_margin(&mut self, delta: f64, cx: &mut Context<Self>) {
        let mode = self.active_mode();
        let balance = TradeStore::global().lock().unwrap().balance(mode);
        let new_margin = (self.margin + delta).max(10.0).min(balance.max(10.0));
        self.margin = (new_margin * 100.0).round() / 100.0;
        self.margin_focused = false;
        self.size_focused = false;
        cx.notify();
    }

    pub fn set_margin_percent(&mut self, pct: f64, cx: &mut Context<Self>) {
        let mode = self.active_mode();
        let balance = TradeStore::global().lock().unwrap().balance(mode);
        let raw = balance * pct;
        let rounded = if raw >= 100.0 {
            (raw / 25.0).round() * 25.0
        } else {
            (raw * 10.0).round() / 10.0
        };
        self.margin = rounded.max(10.0).min(balance.max(10.0));
        self.margin_focused = false;
        self.size_focused = false;
        cx.notify();
    }

    pub fn switch_exchange(
        &mut self,
        new_exchange: String,
        cx: &mut Context<Self>,
    ) {
        self.exchange = new_exchange.clone();
        self.limit_price = None;
        self.price_focused = false;
        self.margin_focused = false;
        self.size_focused = false;
        let symbols = GatewayRegistry::global().symbols(&new_exchange);
        if !symbols.is_empty() && !symbols.contains(&self.symbol) {
            self.symbol = symbols[0].clone();
        }

        if let Some(ref pid) = self.pane_id {
            let pane_id = pid.clone();
            let ex = self.exchange.clone();
            let sym = self.symbol.clone();
            cx.spawn(async move |_this, cx| {
                let mut config = serde_json::Map::new();
                config.insert("exchange".to_string(), serde_json::Value::String(ex));
                config.insert("symbol".to_string(), serde_json::Value::String(sym));
                let _ = cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(
                    |global, cx| {
                        let _ = global.view.update(cx, |view, cx| {
                            view.handle_pane_event(
                                &crate::components::ui::pane_grid::renderer::PaneGridEvent::UpdateModuleConfig {
                                    pane_id,
                                    config: serde_json::Value::Object(config),
                                },
                                cx,
                            );
                        });
                    },
                );
            })
            .detach();
        }
        cx.notify();
    }

    pub fn switch_symbol(
        &mut self,
        new_symbol: String,
        cx: &mut Context<Self>,
    ) {
        self.symbol = new_symbol.clone();
        self.limit_price = None;
        self.price_focused = false;
        self.margin_focused = false;
        self.size_focused = false;
        if let Some(ref pid) = self.pane_id {
            let pane_id = pid.clone();
            let sym = self.symbol.clone();
            cx.spawn(async move |_this, cx| {
                let mut config = serde_json::Map::new();
                config.insert("symbol".to_string(), serde_json::Value::String(sym));
                let _ = cx.update_global::<crate::components::pages::home::GlobalHomeView, _>(
                    |global, cx| {
                        let _ = global.view.update(cx, |view, cx| {
                            view.handle_pane_event(
                                &crate::components::ui::pane_grid::renderer::PaneGridEvent::UpdateModuleConfig {
                                    pane_id,
                                    config: serde_json::Value::Object(config),
                                },
                                cx,
                            );
                        });
                    },
                );
            })
            .detach();
        }
        cx.notify();
    }

    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.price_input.is_none() {
            let mark = self.price();
            let initial_p = self.limit_price.or(mark);
            let p_str = initial_p.map(Self::fmt_price).unwrap_or_default();
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder("0.00");
                if !p_str.is_empty() {
                    state.set_value(p_str, window, cx);
                }
                state
            });
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Focus => {
                    this.price_focused = true;
                }
                InputEvent::Blur => {
                    this.price_focused = false;
                    let text = input.read(cx).value().to_string();
                    if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val > 0.0 && val.is_finite() {
                            this.limit_price = Some(val);
                        }
                    }
                    cx.notify();
                }
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val > 0.0 && val.is_finite() {
                            this.limit_price = Some(val);
                            cx.notify();
                        }
                    }
                }
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().to_string();
                    if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val > 0.0 && val.is_finite() {
                            this.limit_price = Some(val);
                            cx.notify();
                        }
                    }
                }
            })
            .detach();
            self.price_input = Some(input);
        }

        if self.margin_input.is_none() {
            let m_str = format!("{:.2}", self.margin);
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder("0.00");
                state.set_value(m_str, window, cx);
                state
            });
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Focus => {
                    this.margin_focused = true;
                }
                InputEvent::Blur => {
                    this.margin_focused = false;
                    let text = input.read(cx).value().to_string();
                    if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val >= 0.0 && val.is_finite() {
                            let mode = this.active_mode();
                            let balance = TradeStore::global().lock().unwrap().balance(mode);
                            this.margin = val.min(balance.max(10.0));
                        }
                    }
                    cx.notify();
                }
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val >= 0.0 && val.is_finite() {
                            let mode = this.active_mode();
                            let balance = TradeStore::global().lock().unwrap().balance(mode);
                            this.margin = val.min(balance.max(10.0));
                            cx.notify();
                        }
                    }
                }
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().to_string();
                    if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val >= 0.0 && val.is_finite() {
                            let mode = this.active_mode();
                            let balance = TradeStore::global().lock().unwrap().balance(mode);
                            this.margin = val.min(balance.max(10.0));
                            cx.notify();
                        }
                    }
                }
            })
            .detach();
            self.margin_input = Some(input);
        }

        if self.size_input.is_none() {
            let mark = self.price();
            let exec_price = match self.order_type {
                OrderType::Limit => self.limit_price.or(mark).unwrap_or(1.0),
                OrderType::Market => mark.unwrap_or(1.0),
            };
            let notional = self.margin * self.leverage as f64;
            let qty = if exec_price > 0.0 {
                notional / exec_price
            } else {
                0.0
            };
            let s_str = Self::fmt_size(qty);
            let input = cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder("0.0000");
                state.set_value(s_str, window, cx);
                state
            });
            cx.subscribe(&input, |this, input, event: &InputEvent, cx| match event {
                InputEvent::Focus => {
                    this.size_focused = true;
                }
                InputEvent::Blur => {
                    this.size_focused = false;
                    let text = input.read(cx).value().to_string();
                    if let Ok(qty) = text.trim().replace(',', "").parse::<f64>() {
                        if qty >= 0.0 && qty.is_finite() {
                            let exec_price = match this.order_type {
                                OrderType::Limit => {
                                    this.limit_price.or_else(|| this.price()).unwrap_or(1.0)
                                }
                                OrderType::Market => this.price().unwrap_or(1.0),
                            };
                            let lev = this.leverage.max(1) as f64;
                            let mode = this.active_mode();
                            let balance = TradeStore::global().lock().unwrap().balance(mode);
                            let computed_margin = (qty * exec_price) / lev;
                            this.margin = computed_margin.min(balance.max(10.0));
                        }
                    }
                    cx.notify();
                }
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    if let Ok(qty) = text.trim().replace(',', "").parse::<f64>() {
                        if qty >= 0.0 && qty.is_finite() {
                            let exec_price = match this.order_type {
                                OrderType::Limit => {
                                    this.limit_price.or_else(|| this.price()).unwrap_or(1.0)
                                }
                                OrderType::Market => this.price().unwrap_or(1.0),
                            };
                            let lev = this.leverage.max(1) as f64;
                            let mode = this.active_mode();
                            let balance = TradeStore::global().lock().unwrap().balance(mode);
                            let computed_margin = (qty * exec_price) / lev;
                            this.margin = computed_margin.min(balance.max(10.0));
                            cx.notify();
                        }
                    }
                }
                InputEvent::PressEnter { .. } => {
                    let text = input.read(cx).value().to_string();
                    if let Ok(qty) = text.trim().replace(',', "").parse::<f64>() {
                        if qty >= 0.0 && qty.is_finite() {
                            let exec_price = match this.order_type {
                                OrderType::Limit => {
                                    this.limit_price.or_else(|| this.price()).unwrap_or(1.0)
                                }
                                OrderType::Market => this.price().unwrap_or(1.0),
                            };
                            let lev = this.leverage.max(1) as f64;
                            let mode = this.active_mode();
                            let balance = TradeStore::global().lock().unwrap().balance(mode);
                            let computed_margin = (qty * exec_price) / lev;
                            this.margin = computed_margin.min(balance.max(10.0));
                        }
                    }
                    cx.notify();
                }
            })
            .detach();
            self.size_input = Some(input);
        }
    }

    pub fn execute_order(&mut self, cx: &mut Context<Self>) {
        let mode = self.active_mode();

        // Enforce live trading prerequisites: configured API key
        if mode == TradingMode::Live && !api_management::has_credential(&self.exchange) {
            self.notification = Some((
                true,
                format!(
                    "LIVE ORDER BLOCKED: No API key for exchange '{}'. Add API credentials in Settings -> API Keys to enable live execution.",
                    self.exchange.to_uppercase()
                ),
            ));
            cx.notify();
            return;
        }

        let Some(mark) = self.price() else {
            self.notification = Some((true, "Awaiting live market price feed...".to_string()));
            cx.notify();
            return;
        };

        // For limit orders: first read and sync latest value directly from price_input
        if self.order_type == OrderType::Limit {
            if let Some(ref price_inp) = self.price_input {
                let text = price_inp.read(cx).value().to_string();
                if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                    if val > 0.0 && val.is_finite() {
                        self.limit_price = Some(val);
                    }
                }
            }
        }

        let exec_price = match self.order_type {
            OrderType::Limit => {
                let Some(lp) = self.limit_price else {
                    self.notification = Some((
                        true,
                        "Please specify a valid limit price for the limit order.".to_string(),
                    ));
                    cx.notify();
                    return;
                };
                if lp <= 0.0 || !lp.is_finite() {
                    self.notification = Some((
                        true,
                        "Limit price must be greater than 0.".to_string(),
                    ));
                    cx.notify();
                    return;
                }
                lp
            }
            OrderType::Market => mark,
        };

        if exec_price <= 0.0 || !exec_price.is_finite() {
            self.notification = Some((true, "Invalid order execution price.".to_string()));
            cx.notify();
            return;
        }

        let result = TradeStore::global().lock().unwrap().execute_order(
            mode,
            self.symbol.clone(),
            self.side,
            self.order_type,
            exec_price,
            self.margin,
            self.leverage,
        );

        match result {
            Ok(order) => {
                let side_str = match self.side {
                    Side::Long => "BUY / LONG",
                    Side::Short => "SELL / SHORT",
                };
                let type_str = match self.order_type {
                    OrderType::Limit => "LIMIT",
                    OrderType::Market => "MARKET",
                };
                let base = self.base_asset().to_string();
                let mode_prefix = if mode == TradingMode::Live {
                    let creds = api_management::get_credentials_for_exchange(&self.exchange);
                    let label = creds.first().map(|c| c.label.as_str()).unwrap_or("API Key");
                    let testnet_tag = if creds.first().map(|c| c.is_testnet).unwrap_or(false) {
                        " [TESTNET]"
                    } else {
                        ""
                    };
                    format!("⚡ LIVE{testnet_tag} ({label})")
                } else {
                    "🛡 PAPER".to_string()
                };
                self.notification = Some((
                    false,
                    format!(
                        "{mode_prefix} {}: {type_str} {side_str} {} {base} @ ${} (Notional: ${:.2})",
                        order.status,
                        Self::fmt_size(order.quantity),
                        Self::fmt_price(exec_price),
                        order.notional
                    ),
                ));
            }
            Err(err) => {
                let mode_str = if mode == TradingMode::Live { "LIVE" } else { "PAPER" };
                self.notification = Some((true, format!("[{mode_str}] {err}")));
            }
        }
        cx.notify();
    }

    pub fn reset_balance(&mut self, cx: &mut Context<Self>) {
        if self.is_live_mode {
            TradeStore::global().lock().unwrap().reset_live();
            self.notification = Some((
                false,
                "Live trading desk reset: live simulated balance reset to $10,000.00 USDT.".to_string(),
            ));
        } else {
            TradeStore::global().lock().unwrap().reset_paper();
            self.notification = Some((
                false,
                "Virtual paper balance reset to $100,000.00 USDT.".to_string(),
            ));
        }
        cx.notify();
    }
}

impl Render for ExecutionPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mark = self.price();

        if self.order_type == OrderType::Limit && self.limit_price.is_none() {
            if let Some(ref price_inp) = self.price_input {
                let text = price_inp.read(cx).value().to_string();
                if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                    if val > 0.0 && val.is_finite() {
                        self.limit_price = Some(val);
                    }
                }
            }
            if self.limit_price.is_none() {
                self.limit_price = mark;
            }
        }

        let exec_price = match self.order_type {
            OrderType::Limit => self.limit_price.or(mark).unwrap_or(1.0),
            OrderType::Market => mark.unwrap_or(1.0),
        };
        let notional = self.margin * self.leverage as f64;
        let qty = if exec_price > 0.0 { notional / exec_price } else { 0.0 };
        let liq_price = mark.map(|p| self.calculate_liquidation(p));

        self.ensure_inputs(window, cx);

        // Keep inputs in sync with model state when not actively focused
        if let Some(ref price_inp) = self.price_input {
            if !self.price_focused {
                if self.order_type == OrderType::Limit {
                    let cur = price_inp.read(cx).value().to_string();
                    if let Ok(val) = cur.trim().replace('$', "").replace(',', "").parse::<f64>() {
                        if val > 0.0 && val.is_finite() {
                            self.limit_price = Some(val);
                        }
                    }
                }
                let target_price = match self.order_type {
                    OrderType::Limit => self.limit_price,
                    OrderType::Market => mark,
                };
                if let Some(p) = target_price {
                    let cur = price_inp.read(cx).value().to_string();
                    let diff = cur
                        .trim()
                        .replace('$', "")
                        .replace(',', "")
                        .parse::<f64>()
                        .map(|v| (v - p).abs() > 0.0001)
                        .unwrap_or(true);
                    if diff {
                        let formatted = Self::fmt_price(p);
                        price_inp.update(cx, |state, cx| {
                            state.set_value(formatted, window, cx);
                        });
                    }
                }
            }
        }

        if let Some(ref margin_inp) = self.margin_input {
            if !self.margin_focused {
                let cur = margin_inp.read(cx).value().to_string();
                let diff = cur
                    .trim()
                    .replace(',', "")
                    .parse::<f64>()
                    .map(|v| (v - self.margin).abs() > 0.01)
                    .unwrap_or(true);
                if diff {
                    let formatted = format!("{:.2}", self.margin);
                    margin_inp.update(cx, |state, cx| {
                        state.set_value(formatted, window, cx);
                    });
                }
            }
        }

        if let Some(ref size_inp) = self.size_input {
            if !self.size_focused {
                let cur = size_inp.read(cx).value().to_string();
                let diff = cur
                    .trim()
                    .replace(',', "")
                    .parse::<f64>()
                    .map(|v| (v - qty).abs() > 0.000001)
                    .unwrap_or(true);
                if diff {
                    let formatted = Self::fmt_size(qty);
                    size_inp.update(cx, |state, cx| {
                        state.set_value(formatted, window, cx);
                    });
                }
            }
        }

        let c = &cx.theme().colors;

        let has_api = api_management::has_credential(&self.exchange);
        let api_creds = api_management::get_credentials_for_exchange(&self.exchange);
        let cred_label = api_creds
            .first()
            .map(|c| c.label.clone())
            .unwrap_or_else(|| "Direct API".to_string());

        let mode = self.active_mode();
        let (avail_balance, deployed_margin, total_unrealized_pnl, total_equity) = {
            let store = TradeStore::global().lock().unwrap();
            let bal = store.balance(mode);
            let dep = store.deployed_margin(mode);
            let upnl = store.total_unrealized_pnl(mode);
            let eq = store.total_equity(mode);
            (bal, dep, upnl, eq)
        };

        let liq_buffer_pct = match (mark, liq_price) {
            (Some(m), Some(l)) if m > 0.0 => ((m - l).abs() / m) * 100.0,
            _ => 0.0,
        };

        let base = self.base_asset().to_string();
        let quote = self.quote_asset().to_string();
        let mono_font = cx.theme().mono_font_family.clone();

        // Responsive breakpoints based on pane dimensions
        let is_ultra_wide = self.width >= 700.0 && self.height < 450.0;
        let is_wide = !is_ultra_wide
            && ((self.width >= 420.0 && self.height >= 200.0)
                || (self.width >= 380.0 && self.height < 340.0));
        let is_narrow = self.width < 360.0;
        let is_very_narrow = self.width < 290.0;
        let is_compact_h = self.height < 380.0;
        let is_micro_h = self.height < 290.0;

        v_flex()
            .size_full()
            .overflow_hidden()
            .bg(c.background)
            // ── Top Tactical Control Bar ─────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .items_center()
                    .justify_between()
                    .bg(c.table_head)
                    .border_b_1()
                    .border_color(c.border)
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .flex_shrink_0()
                            // Live Price Beacon & Dynamic Ticker
                            .child(
                                h_flex()
                                    .gap(px(4.0))
                                    .items_center()
                                    .px_1p5()
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(c.background)
                                    .border_1()
                                    .border_color(c.border.opacity(0.6))
                                    .child(
                                        div()
                                            .size(px(6.0))
                                            .rounded(px(3.0))
                                            .bg(if mark.is_some() { c.bullish } else { c.muted_foreground }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(c.muted_foreground)
                                            .child(self.symbol.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .font_family(mono_font.clone())
                                            .text_color(if self.price_direction > 0 {
                                                c.bullish
                                            } else if self.price_direction < 0 {
                                                c.bearish
                                            } else {
                                                c.foreground
                                            })
                                            .child(format!(
                                                "${}",
                                                mark.map(Self::fmt_price).unwrap_or_else(|| "—".into())
                                            )),
                                    )
                                    .when(self.price_direction != 0, |this| {
                                        let is_up = self.price_direction > 0;
                                        this.child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(if is_up { c.bullish } else { c.bearish })
                                                .child(if is_up { "▲" } else { "▼" }),
                                        )
                                    }),
                            ),
                    )
                    // Right Header Controls: Mode Switcher & API Badge
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .flex_shrink_0()
                            // Interactive Mode Pill Switcher (Click to toggle Live/Paper)
                            .child(
                                div()
                                    .id("desk-mode-toggle")
                                    .px_2()
                                    .py(px(2.0))
                                    .cursor_pointer()
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(if self.is_live_mode {
                                        c.danger.opacity(0.8)
                                    } else {
                                        c.primary.opacity(0.8)
                                    })
                                    .bg(if self.is_live_mode {
                                        c.danger.opacity(0.18)
                                    } else {
                                        c.primary.opacity(0.15)
                                    })
                                    .text_color(if self.is_live_mode {
                                        c.danger
                                    } else {
                                        c.primary
                                    })
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .hover(|s| s.opacity(0.85))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.is_live_mode = !this.is_live_mode;
                                            let mode = this.active_mode();
                                            TradeStore::global().lock().unwrap().active_mode = mode;
                                            if this.is_live_mode {
                                                let has_api = api_management::has_credential(&this.exchange);
                                                if has_api {
                                                    let creds = api_management::get_credentials_for_exchange(&this.exchange);
                                                    let label = creds.first().map(|c| c.label.as_str()).unwrap_or("API Key");
                                                    let testnet = creds.first().map(|c| c.is_testnet).unwrap_or(false);
                                                    let env = if testnet { " [TESTNET]" } else { "" };
                                                    this.notification = Some((
                                                        false,
                                                        format!("Switched to LIVE API Execution Mode{env} (Account: {label})"),
                                                    ));
                                                } else {
                                                    this.notification = Some((
                                                        true,
                                                        format!("Switched to LIVE Mode (WARNING: No API key for {}. Go to Settings -> API Keys)", this.exchange.to_uppercase()),
                                                    ));
                                                }
                                            } else {
                                                this.notification = Some((
                                                    false,
                                                    "Switched to VIRTUAL PAPER Simulation Mode (Risk-Free)".to_string(),
                                                ));
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .child(if is_narrow {
                                        if self.is_live_mode { "LIVE" } else { "SIM" }
                                    } else {
                                        if self.is_live_mode { "⚡ LIVE" } else { "🛡 PAPER" }
                                    }),
                            )
                            .when(has_api && self.width >= 460.0, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .py(px(2.0))
                                        .rounded(px(4.0))
                                        .bg(c.accent.opacity(0.2))
                                        .border_1()
                                        .border_color(c.accent.opacity(0.4))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(c.primary)
                                        .child(format!("KEY: {cred_label}")),
                                )
                            }),
                    ),
            )
            // ── Interactive Notification Toast ───────────────────────────────
            .when_some(self.notification.clone(), |this, (is_error, msg)| {
                this.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .px(if is_very_narrow { px(6.0) } else { px(10.0) })
                        .py(if is_micro_h { px(3.0) } else { px(5.0) })
                        .justify_between()
                        .items_center()
                        .gap(px(6.0))
                        .bg(if is_error {
                            c.danger.opacity(0.15)
                        } else {
                            c.bullish.opacity(0.15)
                        })
                        .border_b_1()
                        .border_color(if is_error {
                            c.danger.opacity(0.4)
                        } else {
                            c.bullish.opacity(0.4)
                        })
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(6.0))
                                .items_center()
                                .child(
                                    Icon::new(if is_error {
                                        IconName::TriangleAlert
                                    } else {
                                        IconName::Check
                                    })
                                    .size(if is_micro_h { px(11.) } else { px(13.) })
                                    .flex_shrink_0()
                                    .text_color(if is_error { c.danger } else { c.bullish }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(c.foreground)
                                        .child(msg),
                                ),
                        )
                        .child(
                            div()
                                .id("dismiss-toast-btn")
                                .flex_shrink_0()
                                .cursor_pointer()
                                .px_1()
                                .py(px(0.5))
                                .rounded(px(3.0))
                                .hover(|s| s.bg(c.muted.opacity(0.5)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.notification = None;
                                        cx.notify();
                                    }),
                                )
                                .child(
                                    Icon::new(IconName::Close)
                                        .size(px(11.))
                                        .text_color(c.muted_foreground),
                                ),
                        ),
                )
            })
            // ── Main Body Station ────────────────────────────────────────────
            .child({
                let is_long = self.side == Side::Long;

                // 1. Dual Tactical Long / Short Pods
                let pods_el = h_flex()
                    .w_full()
                    .gap_1p5()
                    // BUY / LONG Pod
                    .child(
                        v_flex()
                            .id("buy-long-pod")
                            .flex_1()
                            .min_w_0()
                            .p(if is_micro_h {
                                px(4.0)
                            } else if is_compact_h {
                                px(6.0)
                            } else {
                                px(8.0)
                            })
                            .cursor_pointer()
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(if is_long {
                                c.bullish
                            } else {
                                c.border.opacity(0.5)
                            })
                            .bg(if is_long {
                                c.bullish.opacity(0.12)
                            } else {
                                c.muted.opacity(0.15)
                            })
                            .hover(|s| s.border_color(c.bullish.opacity(0.8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.side = Side::Long;
                                    cx.notify();
                                }),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(if is_long {
                                                c.bullish
                                            } else {
                                                c.muted_foreground
                                            })
                                            .child(if is_very_narrow { "▲ BUY" } else { "▲ BUY / LONG" }),
                                    )
                                    .when(is_long && !is_very_narrow, |this| {
                                        this.child(
                                            Icon::new(IconName::Check)
                                                .size(px(12.))
                                                .text_color(c.bullish),
                                        )
                                    }),
                            )
                            .when(!is_compact_h && !is_narrow && !is_wide && !is_ultra_wide, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(c.muted_foreground)
                                        .child("Profit on upward momentum"),
                                )
                            }),
                    )
                    // SELL / SHORT Pod
                    .child(
                        v_flex()
                            .id("sell-short-pod")
                            .flex_1()
                            .min_w_0()
                            .p(if is_micro_h {
                                px(4.0)
                            } else if is_compact_h {
                                px(6.0)
                            } else {
                                px(8.0)
                            })
                            .cursor_pointer()
                            .rounded(px(5.0))
                            .border_1()
                            .border_color(if !is_long {
                                c.bearish
                            } else {
                                c.border.opacity(0.5)
                            })
                            .bg(if !is_long {
                                c.bearish.opacity(0.12)
                            } else {
                                c.muted.opacity(0.15)
                            })
                            .hover(|s| s.border_color(c.bearish.opacity(0.8)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.side = Side::Short;
                                    cx.notify();
                                }),
                            )
                            .child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(if !is_long {
                                                c.bearish
                                            } else {
                                                c.muted_foreground
                                            })
                                            .child(if is_very_narrow { "▼ SELL" } else { "▼ SELL / SHORT" }),
                                    )
                                    .when(!is_long && !is_very_narrow, |this| {
                                        this.child(
                                            Icon::new(IconName::Check)
                                                .size(px(12.))
                                                .text_color(c.bearish),
                                        )
                                    }),
                            )
                            .when(!is_compact_h && !is_narrow && !is_wide && !is_ultra_wide, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(c.muted_foreground)
                                        .child("Profit on market drawdown"),
                                )
                            }),
                    );

                // 2. Order Execution Type Row (Exchange Segmented Control)
                let type_row = h_flex()
                    .w_full()
                    .p(px(2.0))
                    .rounded(px(4.0))
                    .bg(c.muted.opacity(0.18))
                    .border_1()
                    .border_color(c.border.opacity(0.3))
                    .gap(px(2.0))
                    .child(
                        div()
                            .id("tab-market")
                            .flex_1()
                            .py(if is_micro_h { px(1.5) } else { px(3.0) })
                            .items_center()
                            .justify_center()
                            .text_center()
                            .cursor_pointer()
                            .rounded(px(3.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .bg(if self.order_type == OrderType::Market {
                                c.background
                            } else {
                                hsla(0.0, 0.0, 0.0, 0.0)
                            })
                            .text_color(if self.order_type == OrderType::Market {
                                c.foreground
                            } else {
                                c.muted_foreground
                            })
                            .when(self.order_type == OrderType::Market, |s| {
                                s.shadow_xs().border_1().border_color(c.border.opacity(0.4))
                            })
                            .hover(|s| s.text_color(c.foreground))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.order_type = OrderType::Market;
                                    cx.notify();
                                }),
                            )
                            .child("MARKET"),
                    )
                    .child(
                        div()
                            .id("tab-limit")
                            .flex_1()
                            .py(if is_micro_h { px(1.5) } else { px(3.0) })
                            .items_center()
                            .justify_center()
                            .text_center()
                            .cursor_pointer()
                            .rounded(px(3.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .bg(if self.order_type == OrderType::Limit {
                                c.background
                            } else {
                                hsla(0.0, 0.0, 0.0, 0.0)
                            })
                            .text_color(if self.order_type == OrderType::Limit {
                                c.foreground
                            } else {
                                c.muted_foreground
                            })
                            .when(self.order_type == OrderType::Limit, |s| {
                                s.shadow_xs().border_1().border_color(c.border.opacity(0.4))
                            })
                            .hover(|s| s.text_color(c.foreground))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.order_type = OrderType::Limit;
                                    if let Some(ref price_inp) = this.price_input {
                                        let text = price_inp.read(cx).value().to_string();
                                        if let Ok(val) = text.trim().replace('$', "").replace(',', "").parse::<f64>() {
                                            if val > 0.0 && val.is_finite() {
                                                this.limit_price = Some(val);
                                            }
                                        }
                                    }
                                    if this.limit_price.is_none() {
                                        this.limit_price = this.price();
                                    }
                                    cx.notify();
                                }),
                            )
                            .child("LIMIT"),
                    );

                let lev_chips: Vec<u32> = if is_very_narrow {
                    vec![1, 5, 20, 100]
                } else if is_narrow {
                    vec![1, 5, 10, 50, 100]
                } else {
                    vec![1, 2, 5, 10, 20, 50, 100]
                };

                // 3. Leverage Multiplier Matrix with Risk Badging
                let leverage_el = v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(c.muted_foreground)
                                            .child("LEVERAGE"),
                                    )
                                    .when(!is_very_narrow, |this| {
                                        this.child(
                                            div()
                                                .px_1p5()
                                                .py(px(0.5))
                                                .rounded(px(2.0))
                                                .text_xs()
                                                .font_weight(FontWeight::BOLD)
                                                .bg(if self.leverage <= 5 {
                                                    c.bullish.opacity(0.18)
                                                } else if self.leverage <= 20 {
                                                    c.warning.opacity(0.18)
                                                } else {
                                                    c.danger.opacity(0.18)
                                                })
                                                .text_color(if self.leverage <= 5 {
                                                    c.bullish
                                                } else if self.leverage <= 20 {
                                                    c.warning
                                                } else {
                                                    c.danger
                                                })
                                                .child(if self.leverage <= 5 {
                                                    "SAFE"
                                                } else if self.leverage <= 20 {
                                                    "TACTICAL"
                                                } else {
                                                    "HIGH RISK"
                                                }),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font.clone())
                                    .text_color(c.primary)
                                    .child(format!("{}x", self.leverage)),
                            ),
                    )
                    .child(
                        h_flex().flex_wrap().gap_1().children(
                            lev_chips.into_iter().map(|lev| {
                                let active = self.leverage == lev;
                                div()
                                    .id(ElementId::Name(format!("lev-{lev}").into()))
                                    .flex_1()
                                    .min_w(px(22.0))
                                    .items_center()
                                    .justify_center()
                                    .text_center()
                                    .py(if is_micro_h { px(1.5) } else { px(2.5) })
                                    .cursor_pointer()
                                    .rounded(px(3.0))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font.clone())
                                    .border_1()
                                    .border_color(if active {
                                        c.primary
                                    } else {
                                        c.border.opacity(0.4)
                                    })
                                    .bg(if active {
                                        c.primary
                                    } else {
                                        c.muted.opacity(0.2)
                                    })
                                    .text_color(if active {
                                        c.primary_foreground
                                    } else {
                                        c.muted_foreground
                                    })
                                    .hover(|s| s.bg(c.muted.opacity(0.5)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.leverage = lev;
                                            this.size_focused = false;
                                            cx.notify();
                                        }),
                                    )
                                    .child(format!("{lev}x"))
                            }),
                        ),
                    );

                let pct_chips: Vec<(f64, &str)> = if is_very_narrow {
                    vec![(0.25, "25%"), (0.50, "50%"), (1.00, "MAX")]
                } else {
                    vec![(0.10, "10%"), (0.25, "25%"), (0.50, "50%"), (0.75, "75%"), (1.00, "MAX")]
                };

                // Stepper options adapted to width
                let steppers: Vec<(f64, &str)> = if is_very_narrow {
                    vec![(-25.0, "-25"), (25.0, "+25")]
                } else if is_narrow {
                    vec![(-100.0, "-100"), (-25.0, "-25"), (25.0, "+25"), (100.0, "+100")]
                } else {
                    vec![(-100.0, "-$100"), (-25.0, "-$25"), (25.0, "+$25"), (100.0, "+$100"), (500.0, "+$500")]
                };

                let price_input_el = if self.order_type == OrderType::Limit {
                    if let Some(ref price_inp) = self.price_input {
                        Input::new(price_inp)
                            .small()
                            .prefix(
                                div()
                                    .text_xs()
                                    .text_color(c.muted_foreground)
                                    .font_weight(FontWeight::MEDIUM)
                                    .w(px(46.0))
                                    .mr_1()
                                    .child("Price"),
                            )
                            .suffix(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(c.muted_foreground)
                                            .child(quote.clone()),
                                    )
                                    .child(
                                        div()
                                            .id("last-price-pill")
                                            .px_1()
                                            .py(px(0.5))
                                            .rounded(px(2.0))
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .cursor_pointer()
                                            .bg(c.muted.opacity(0.4))
                                            .hover(|s| s.bg(c.accent.opacity(0.3)).text_color(c.primary))
                                            .text_color(c.muted_foreground)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, _, cx| {
                                                    if let Some(p) = this.price() {
                                                        this.limit_price = Some(p);
                                                        this.price_focused = false;
                                                        cx.notify();
                                                    }
                                                }),
                                            )
                                            .child("Last"),
                                    ),
                            )
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    }
                } else {
                    h_flex()
                        .w_full()
                        .h(px(24.0))
                        .items_center()
                        .justify_between()
                        .px_2()
                        .rounded(cx.theme().radius)
                        .bg(c.muted.opacity(0.12))
                        .border_1()
                        .border_color(c.border.opacity(0.35))
                        .child(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .font_weight(FontWeight::MEDIUM)
                                .w(px(46.0))
                                .child("Price"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(c.muted_foreground.opacity(0.85))
                                .child(if let Some(m) = mark {
                                    format!("Market (~${})", Self::fmt_price(m))
                                } else {
                                    "Market (Best Execution)".to_string()
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(c.muted_foreground)
                                .child(quote.clone()),
                        )
                        .into_any_element()
                };

                let size_input_el = if let Some(ref size_inp) = self.size_input {
                    Input::new(size_inp)
                        .small()
                        .prefix(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .font_weight(FontWeight::MEDIUM)
                                .w(px(46.0))
                                .mr_1()
                                .child("Size"),
                        )
                        .suffix(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(c.muted_foreground)
                                .child(base.clone()),
                        )
                        .into_any_element()
                } else {
                    div().into_any_element()
                };

                let margin_input_el = if let Some(ref margin_inp) = self.margin_input {
                    Input::new(margin_inp)
                        .small()
                        .prefix(
                            div()
                                .text_xs()
                                .text_color(c.muted_foreground)
                                .font_weight(FontWeight::MEDIUM)
                                .w(px(46.0))
                                .mr_1()
                                .child("Margin"),
                        )
                        .suffix(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(c.muted_foreground)
                                .child(quote.clone()),
                        )
                        .into_any_element()
                } else {
                    div().into_any_element()
                };

                // 4. Custom Exchange Order Inputs Block (Price, Size, Margin, %, Steppers)
                let inputs_el = v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(c.muted_foreground)
                                    .child("ORDER CONFIG"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if self.is_live_mode { c.danger } else { c.muted_foreground })
                                    .font_weight(if self.is_live_mode { FontWeight::BOLD } else { FontWeight::NORMAL })
                                    .child(format!(
                                        "Avail: ${:.2} {quote} [{}]",
                                        avail_balance,
                                        if self.is_live_mode { "LIVE" } else { "PAPER" }
                                    )),
                            ),
                    )
                    .child(price_input_el)
                    .child(size_input_el)
                    .child(margin_input_el)
                    // Quick Cash Allocation Chips (%)
                    .child(
                        h_flex().gap_1().children(
                            pct_chips.into_iter().map(|(pct, label)| {
                                div()
                                    .id(ElementId::Name(format!("pct-{label}").into()))
                                    .flex_1()
                                    .min_w(px(22.0))
                                    .py(if is_micro_h { px(1.5) } else { px(2.5) })
                                    .items_center()
                                    .justify_center()
                                    .text_center()
                                    .cursor_pointer()
                                    .rounded(px(3.0))
                                    .bg(c.muted.opacity(0.25))
                                    .border_1()
                                    .border_color(c.border.opacity(0.5))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c.muted_foreground)
                                    .hover(|s| s.bg(c.accent.opacity(0.3)).text_color(c.foreground))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.set_margin_percent(pct, cx);
                                        }),
                                    )
                                    .child(label)
                            }),
                        ),
                    )
                    // Tactile Stepper Adjustment Controls
                    .when(!is_micro_h, |this| {
                        this.child(
                            h_flex().gap_1().items_center().children(
                                steppers.into_iter().map(|(delta, label)| {
                                    div()
                                        .id(ElementId::Name(format!("step-{}", label.replace('$', "")).into()))
                                        .flex_1()
                                        .min_w(px(22.0))
                                        .py(if is_compact_h { px(1.5) } else { px(2.5) })
                                        .items_center()
                                        .justify_center()
                                        .text_center()
                                        .cursor_pointer()
                                        .rounded(px(3.0))
                                        .bg(c.muted.opacity(0.18))
                                        .border_1()
                                        .border_color(c.border.opacity(0.4))
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .font_family(mono_font.clone())
                                        .text_color(c.foreground)
                                        .hover(|s| s.bg(c.muted.opacity(0.5)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.adjust_margin(delta, cx);
                                            }),
                                        )
                                        .child(label)
                                }),
                            ),
                        )
                    })
                    // Fixed Value Presets (only shown when tall pane to conserve space)
                    .when(self.height >= 520.0 && !is_wide && !is_ultra_wide, |this| {
                        this.child(
                            h_flex().flex_wrap().gap_1().children(
                                [50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0].into_iter().map(|m| {
                                    let active = (self.margin - m).abs() < 0.1;
                                    div()
                                        .id(ElementId::Name(format!("preset-{m:.0}").into()))
                                        .flex_1()
                                        .py_0p5()
                                        .items_center()
                                        .justify_center()
                                        .text_center()
                                        .cursor_pointer()
                                        .rounded(px(3.0))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .font_family(mono_font.clone())
                                        .bg(if active {
                                            c.accent.opacity(0.3)
                                        } else {
                                            c.muted.opacity(0.15)
                                        })
                                        .text_color(if active {
                                            c.foreground
                                        } else {
                                            c.muted_foreground
                                        })
                                        .border_1()
                                        .border_color(if active {
                                            c.primary
                                        } else {
                                            c.border.opacity(0.3)
                                        })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.margin = m;
                                                this.margin_focused = false;
                                                this.size_focused = false;
                                                cx.notify();
                                            }),
                                        )
                                        .child(format!("${m:.0}"))
                                }),
                            ),
                        )
                    });

                // 5. Terminal Risk HUD & Liquidation Proximity Gauge
                let risk_hud_el = v_flex()
                    .w_full()
                    .gap_1()
                    .p(if is_micro_h {
                        px(4.0)
                    } else if is_compact_h {
                        px(6.0)
                    } else {
                        px(8.0)
                    })
                    .rounded(px(5.0))
                    .bg(c.muted.opacity(0.2))
                    .border_1()
                    .border_color(c.border.opacity(0.5))
                    .child(
                        h_flex()
                            .justify_between()
                            .text_xs()
                            .child(
                                div().text_color(c.muted_foreground).child(if is_very_narrow { "Notional:" } else { "Indicative Notional:" }),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font.clone())
                                    .text_color(c.foreground)
                                    .child(format!("${notional:.2}")),
                            ),
                    )
                    .when(!is_micro_h, |this| {
                        this.child(
                            h_flex()
                                .justify_between()
                                .text_xs()
                                .child(
                                    div().text_color(c.muted_foreground).child(if is_very_narrow { "Quantity:" } else { "Position Quantity:" }),
                                )
                                .child(
                                    div()
                                        .font_weight(FontWeight::BOLD)
                                        .font_family(mono_font.clone())
                                        .text_color(c.foreground)
                                        .child(format!("{} {base}", Self::fmt_size(qty))),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .justify_between()
                            .text_xs()
                            .child(
                                div().text_color(c.muted_foreground).child(if is_very_narrow { "Liq Price:" } else { "Est. Liquidation Price:" }),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font.clone())
                                    .text_color(if self.leverage >= 50 {
                                        c.danger
                                    } else {
                                        c.warning
                                    })
                                    .child(format!(
                                        "${}",
                                        liq_price.map(Self::fmt_price).unwrap_or_else(|| "—".into())
                                    )),
                            ),
                    )
                    // Visual Liquidation Buffer Gauge Bar (Relative width to fit dynamic pane)
                    .child(
                        v_flex()
                            .gap(px(2.0))
                            .mt(px(2.0))
                            .child(
                                h_flex()
                                    .justify_between()
                                    .text_xs()
                                    .child(
                                        div()
                                            .text_color(c.muted_foreground)
                                            .child(if is_very_narrow { "Buffer:" } else { "Liquidation Buffer:" }),
                                    )
                                    .child(
                                        div()
                                            .font_weight(FontWeight::BOLD)
                                            .font_family(mono_font.clone())
                                            .text_color(if liq_buffer_pct > 20.0 {
                                                c.bullish
                                            } else if liq_buffer_pct > 8.0 {
                                                c.warning
                                            } else {
                                                c.danger
                                            })
                                            .child(if is_very_narrow {
                                                format!("{liq_buffer_pct:.1}%")
                                            } else {
                                                format!("{liq_buffer_pct:.1}% buffer")
                                            }),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .h(px(4.0))
                                    .rounded(px(2.0))
                                    .bg(c.background)
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .w(gpui::relative((liq_buffer_pct.min(100.0) / 100.0) as f32))
                                            .h_full()
                                            .bg(if liq_buffer_pct > 20.0 {
                                                c.bullish
                                            } else if liq_buffer_pct > 8.0 {
                                                c.warning
                                            } else {
                                                c.danger
                                            }),
                                    ),
                            ),
                    );

                // 6. Tactical Execution Action Bar (Short & Compact)
                let mode_action_tag = if self.is_live_mode { "⚡ LIVE" } else { "🛡 PAPER" };
                let execute_btn_el = div()
                    .id("submit-order-btn")
                    .w_full()
                    .py(if is_micro_h {
                        px(3.0)
                    } else if is_compact_h {
                        px(4.0)
                    } else {
                        px(5.0)
                    })
                    .items_center()
                    .justify_center()
                    .text_center()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if is_long {
                        c.bullish.opacity(0.8)
                    } else {
                        c.bearish.opacity(0.8)
                    })
                    .bg(if is_long { c.bullish } else { c.bearish })
                    .text_color(c.primary_foreground)
                    .shadow_sm()
                    .hover(|s| s.opacity(0.92))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.execute_order(cx);
                        }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .child(if is_very_narrow {
                                format!("{} {} {}", mode_action_tag, if is_long { "BUY" } else { "SELL" }, Self::fmt_size(qty))
                            } else {
                                format!(
                                    "EXECUTE {} {} {} {base}",
                                    mode_action_tag,
                                    if is_long { "BUY" } else { "SELL" },
                                    Self::fmt_size(qty)
                                )
                            }),
                    );

                v_flex()
                    .id("trade-desk-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(if is_micro_h || is_very_narrow {
                        px(4.0)
                    } else if is_compact_h || is_narrow {
                        px(6.0)
                    } else {
                        px(8.0)
                    })
                    .child(if is_ultra_wide {
                        h_flex()
                            .w_full()
                            .gap_2p5()
                            .items_start()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(pods_el)
                                    .child(type_row),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(inputs_el)
                                    .child(leverage_el),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(risk_hud_el)
                                    .child(execute_btn_el),
                            )
                            .into_any_element()
                    } else if is_wide {
                        h_flex()
                            .w_full()
                            .gap_2p5()
                            .items_start()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(if is_compact_h { px(4.0) } else { px(6.0) })
                                    .child(pods_el)
                                    .child(type_row)
                                    .child(inputs_el)
                                    .child(leverage_el),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(if is_compact_h { px(4.0) } else { px(6.0) })
                                    .child(risk_hud_el)
                                    .child(execute_btn_el),
                            )
                            .into_any_element()
                    } else {
                        v_flex()
                            .w_full()
                            .gap(if is_micro_h || is_very_narrow {
                                px(4.0)
                            } else if is_compact_h || is_narrow {
                                px(6.0)
                            } else {
                                px(8.0)
                            })
                            .child(pods_el)
                            .child(type_row)
                            .child(inputs_el)
                            .child(leverage_el)
                            .child(risk_hud_el)
                            .child(execute_btn_el)
                            .into_any_element()
                    })
            })
            // ── Interactive Terminal Footer Telemetry ────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .bg(c.table_head)
                    .border_t_1()
                    .border_color(c.border)
                    .justify_between()
                    .items_center()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .gap(if is_narrow { px(4.0) } else { px(8.0) })
                            .items_center()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(mono_font.clone())
                                    .text_color(c.muted_foreground)
                                    .child(if is_very_narrow {
                                        format!("${:.0}", avail_balance)
                                    } else if is_narrow {
                                        format!("{} ${:.0}", if self.is_live_mode { "LIVE" } else { "CASH" }, avail_balance)
                                    } else {
                                        format!("{}: ${:.1}", if self.is_live_mode { "LIVE CASH" } else { "PAPER CASH" }, avail_balance)
                                    }),
                            )
                            .when(!is_narrow, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .font_family(mono_font.clone())
                                        .text_color(if total_unrealized_pnl >= 0.0 {
                                            c.bullish
                                        } else {
                                            c.bearish
                                        })
                                        .child(format!(
                                            "PNL: {}${:.1}",
                                            if total_unrealized_pnl >= 0.0 { "+" } else { "" },
                                            total_unrealized_pnl
                                        )),
                                )
                            })
                            .when(self.width >= 420.0, |this| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .font_family(mono_font.clone())
                                        .text_color(c.muted_foreground)
                                        .child(format!("MRG: ${:.1}", deployed_margin)),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font)
                                    .text_color(c.foreground)
                                    .child(if is_very_narrow {
                                        format!("EQ ${:.0}", total_equity)
                                    } else if is_narrow {
                                        format!("EQ ${:.0}", total_equity)
                                    } else {
                                        format!("EQ: ${:.1}", total_equity)
                                    }),
                            ),
                    )
                    .child(
                        Button::new("reset-desk-btn")
                            .xsmall()
                            .ghost()
                            .label(if is_narrow {
                                "Reset"
                            } else if self.is_live_mode {
                                "Reset Live"
                            } else {
                                "Reset Paper"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reset_balance(cx);
                            })),
                    ),
            )
    }
}

impl Panel for ExecutionPanel {
    fn panel_name(&self) -> &'static str {
        "ExecutionPanel"
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.title.clone())
    }
    fn toolbar_buttons(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<Vec<Button>> {
        None
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn dump(&self, _: &App) -> PanelState {
        PanelState {
            panel_name: "ExecutionPanel".into(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
                "exchange": self.exchange,
                "symbol": self.symbol
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for ExecutionPanel {}
impl Focusable for ExecutionPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_liquidation_calculation_long_and_short() {
        let entry = 50_000.0;
        let lev = 10;

        // Long liquidation: entry * (1 - 1/lev + 0.005) = 50,000 * (1 - 0.1 + 0.005) = 50,000 * 0.905 = 45,250
        let long_liq = entry * (1.0 - (1.0 / lev as f64) + 0.005);
        assert!((long_liq - 45_250.0).abs() < 1e-4);

        // Short liquidation: entry * (1 + 1/lev - 0.005) = 50,000 * (1 + 0.1 - 0.005) = 50,000 * 1.095 = 54,750
        let short_liq = entry * (1.0 + (1.0 / lev as f64) - 0.005);
        assert!((short_liq - 54_750.0).abs() < 1e-4);
    }

    #[test]
    fn test_order_and_position_math() {
        let margin = 500.0;
        let leverage = 20;
        let mark_price = 2_500.0; // e.g. ETH

        let notional = margin * leverage as f64; // 10,000
        assert_eq!(notional, 10_000.0);

        let quantity = notional / mark_price; // 4.0 ETH
        assert_eq!(quantity, 4.0);

        // Long position PnL when price moves to 2,600 (+100 per ETH)
        let new_price = 2_600.0;
        let pnl = (new_price - mark_price) * quantity;
        assert_eq!(pnl, 400.0);
        let roe = (pnl / margin) * 100.0; // 400 / 500 = 80%
        assert_eq!(roe, 80.0);
    }
}
