/// Watchlist panel — multi-asset real-time market tracker.
///
/// Shows starred symbols by default and searches the catalog on demand.
/// Supports interactive column sorting (Asset, Price, 24h %, Volume)
/// and real-time market prices.
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex, Sizable};
use tokio_util::sync::CancellationToken;

use crate::data::client::tokio_runtime;
use crate::data::feed::TickFeed;

// ---------------------------------------------------------------------------
// Types & Categories
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortColumn {
    Asset,
    Price,
    Change24h,
    Volume24h,
}

#[derive(Clone)]
pub struct WatchlistEntry {
    pub symbol: String,
    pub base: String,
    pub name: String,
    pub exchange: String,
    pub category: &'static str,
    pub initial_price: f64,
    pub change_24h: f64,
    pub volume_24h_usd: f64,
    pub feed: Option<TickFeed>,
    pub sparkline: Vec<f64>,
}

// ---------------------------------------------------------------------------
// WatchlistPanel
// ---------------------------------------------------------------------------

pub struct WatchlistPanel {
    entries: Arc<Mutex<Vec<WatchlistEntry>>>,
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    search_query: String,
    search_input: Option<Entity<InputState>>,
    sort_column: SortColumn,
    sort_ascending: bool,
    favorites: HashSet<String>,
    last_catalog_refresh: Option<std::time::Instant>,
    tracked: Arc<Mutex<HashSet<String>>>,
    _cancel_token: CancellationToken,
}

impl WatchlistPanel {
    pub fn new(title: impl Into<String>, cx: &mut App) -> Self {
        let default_entries = Arc::new(Mutex::new(Self::initial_catalog()));
        let mut favorites = HashSet::new();
        // Default favorites
        favorites.insert("binance:BTC/USDT".to_string());
        favorites.insert("binance:ETH/USDT".to_string());
        favorites.insert("binance:SOL/USDT".to_string());

        let tracked = Arc::new(Mutex::new(favorites.clone()));
        let cancel_token = CancellationToken::new();
        let entries_clone = Arc::clone(&default_entries);
        let tracked_clone = Arc::clone(&tracked);
        let token_clone = cancel_token.clone();

        tokio_runtime().spawn(async move {
            run_watchlist_syncer(entries_clone, tracked_clone, token_clone).await;
        });

        Self {
            entries: default_entries,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 0.0,
            height: 0.0,
            search_query: String::new(),
            search_input: None,
            sort_column: SortColumn::Volume24h,
            sort_ascending: false,
            favorites,
            tracked,
            last_catalog_refresh: None,
            _cancel_token: cancel_token,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
    }

    fn favorite_key(exchange: &str, symbol: &str) -> String {
        format!("{}:{}", exchange.to_lowercase(), symbol.to_uppercase())
    }

    pub fn is_favorite(&self, exchange: &str, symbol: &str) -> bool {
        let key = Self::favorite_key(exchange, symbol);
        self.favorites.contains(&key)
            || self.favorites.contains(&format!("{}:{}", exchange, symbol))
            || (exchange.eq_ignore_ascii_case("binance") && self.favorites.contains(symbol))
    }

    pub fn toggle_favorite(&mut self, exchange: &str, symbol: &str) {
        let key = Self::favorite_key(exchange, symbol);
        if self.is_favorite(exchange, symbol) {
            self.favorites.remove(&key);
            self.favorites.remove(&format!("{}:{}", exchange, symbol));
            self.favorites.remove(symbol);
            if let Ok(mut t) = self.tracked.lock() {
                t.remove(&key);
                t.remove(&format!("{}:{}", exchange, symbol));
                t.remove(symbol);
            }
        } else {
            self.favorites.insert(key.clone());
            if let Ok(mut t) = self.tracked.lock() {
                t.insert(key);
            }
        }
    }

    fn ensure_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search symbol or name"));
        cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_query = input.read(cx).value().to_string();
                cx.notify();
            }
        }).detach();
        self.search_input = Some(input);
    }

    pub fn toggle_sort(&mut self, col: SortColumn) {
        if self.sort_column == col {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_column = col;
            self.sort_ascending = false;
        }
    }

    fn initial_catalog() -> Vec<WatchlistEntry> {
        let registry = crate::data::gateway_registry::GatewayRegistry::global();
        registry.prefetch_all_symbols();
        let mut entries = Vec::new();
        for exchange in registry.exchanges() {
            for info in registry.symbol_infos(&exchange).into_iter().filter(|info| info.active) {
                let base = if info.base.is_empty() {
                    info.base_asset.clone().unwrap_or_default()
                } else {
                    info.base.clone()
                };
                entries.push(WatchlistEntry {
                    symbol: info.symbol,
                    base: base.clone(),
                    name: base,
                    exchange: exchange.clone(),
                    category: "Market",
                    initial_price: 0.0,
                    change_24h: 0.0,
                    volume_24h_usd: 0.0,
                    feed: None,
                    sparkline: Vec::new(),
                });
            }
        }
        entries
    }

    fn refresh_catalog(&mut self) {
        let registry = crate::data::gateway_registry::GatewayRegistry::global();
        registry.prefetch_all_symbols();
        let current = self.entries.lock().unwrap().clone();
        let mut existing = current
            .into_iter()
            .map(|entry| ((entry.exchange.clone(), entry.symbol.clone()), entry))
            .collect::<std::collections::HashMap<_, _>>();
        let mut refreshed = Vec::new();

        for exchange in registry.exchanges() {
            for info in registry.symbol_infos(&exchange).into_iter().filter(|info| info.active) {
                let key = (exchange.clone(), info.symbol.clone());
                if let Some(entry) = existing.remove(&key) {
                    refreshed.push(entry);
                    continue;
                }
                let base = if info.base.is_empty() {
                    info.base_asset.unwrap_or_default()
                } else {
                    info.base
                };
                refreshed.push(WatchlistEntry {
                    symbol: info.symbol,
                    base: base.clone(),
                    name: base,
                    exchange: exchange.clone(),
                    category: "Market",
                    initial_price: 0.0,
                    change_24h: 0.0,
                    volume_24h_usd: 0.0,
                    feed: None,
                    sparkline: Vec::new(),
                });
            }
        }

        *self.entries.lock().unwrap() = refreshed;
    }

    fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{:.2}", price)
        } else if price >= 1.0 {
            format!("{:.3}", price)
        } else if price >= 0.01 {
            format!("{:.4}", price)
        } else {
            format!("{:.7}", price)
        }
    }

    fn fmt_vol(vol: f64) -> String {
        if vol >= 1_000_000_000.0 {
            format!("${:.2}B", vol / 1_000_000_000.0)
        } else if vol >= 1_000_000.0 {
            format!("${:.1}M", vol / 1_000_000.0)
        } else if vol >= 1_000.0 {
            format!("${:.0}K", vol / 1_000.0)
        } else {
            format!("${:.0}", vol)
        }
    }
}

impl Render for WatchlistPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        self.ensure_search_input(window, cx);
        if self.last_catalog_refresh.map_or(true, |last| last.elapsed() >= std::time::Duration::from_secs(2)) {
            self.refresh_catalog();
            self.last_catalog_refresh = Some(std::time::Instant::now());
        }
        let search_input = self.search_input.as_ref().unwrap().clone();
        let theme = cx.theme();
        let colors = &theme.colors;

        let is_compact = self.width < 340.0;
        let is_narrow = self.width < 270.0;

        // Filter items: watchlist table only lists starred symbols
        let query = self.search_query.trim().to_lowercase();
        let entries = self.entries.lock().unwrap().clone();
        let mut filtered: Vec<WatchlistEntry> = entries
            .iter()
            .filter(|item| self.is_favorite(&item.exchange, &item.symbol))
            .cloned()
            .collect();

        let mut search_matches: Vec<WatchlistEntry> = if query.is_empty() {
            Vec::new()
        } else {
            let words: Vec<&str> = query.split_whitespace().collect();
            entries
                .iter()
                .filter(|item| {
                    words.iter().all(|w| {
                        item.symbol.to_lowercase().contains(w)
                            || item.base.to_lowercase().contains(w)
                            || item.exchange.to_lowercase().contains(w)
                    })
                })
                .cloned()
                .collect()
        };
        search_matches.sort_by(|a, b| {
            let a_fav = self.is_favorite(&a.exchange, &a.symbol);
            let b_fav = self.is_favorite(&b.exchange, &b.symbol);
            b_fav.cmp(&a_fav)
                .then_with(|| a.symbol.cmp(&b.symbol))
                .then_with(|| a.exchange.cmp(&b.exchange))
        });
        search_matches.truncate(80);

        // Sort items
        filtered.sort_by(|a, b| {
            let ordering = match self.sort_column {
                SortColumn::Asset => a.symbol.cmp(&b.symbol),
                SortColumn::Price => a.initial_price.partial_cmp(&b.initial_price).unwrap_or(std::cmp::Ordering::Equal),
                SortColumn::Change24h => a.change_24h.partial_cmp(&b.change_24h).unwrap_or(std::cmp::Ordering::Equal),
                SortColumn::Volume24h => a.volume_24h_usd.partial_cmp(&b.volume_24h_usd).unwrap_or(std::cmp::Ordering::Equal),
            };
            if self.sort_ascending {
                ordering
            } else {
                ordering.reverse()
            }
        });

        v_flex()
            .size_full()
            .bg(colors.background)
            .overflow_hidden()
            // Search the catalog to find and star symbols
            .child(
                v_flex()
                    .w_full()
                    .bg(colors.table_head)
                    .border_b_1()
                    .border_color(colors.border)
                    .px_2()
                    .py_1p5()
                    .gap_1p5()
                    .child(Input::new(&search_input).small().cleanable(true))
                    .when(!query.is_empty(), |panel| {
                        panel.child(
                            v_flex()
                                .id("watchlist-search-dropdown")
                                .w_full()
                                .max_h(px(220.0))
                                .overflow_y_scroll()
                                .border_1()
                                .border_color(colors.border)
                                .bg(colors.background)
                                .shadow_md()
                                .children(search_matches.iter().enumerate().map(|(idx, item)| {
                                    self.render_search_result(item, idx, cx)
                                })),
                        )
                    }),
            )
            // Hide all live-data rows while searching; search is a symbol picker.
            .when(query.is_empty(), |panel| panel.child(
            // Table Column Headers
            div().child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .bg(colors.table_head)
                    .border_b_1()
                    .border_color(colors.border)
                    .gap_2()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    // Star spacer
                    .child(div().w(px(18.0)))
                    // Asset column
                    .child(
                        div()
                            .w(px(if is_narrow { 75.0 } else { 95.0 }))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _win, cx| {
                                    this.toggle_sort(SortColumn::Asset);
                                    cx.notify();
                                }),
                            )
                            .child(format!("ASSET {}", if self.sort_column == SortColumn::Asset { if self.sort_ascending { "▲" } else { "▼" } } else { "" })),
                    )
                    // Price column
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _win, cx| {
                                    this.toggle_sort(SortColumn::Price);
                                    cx.notify();
                                }),
                            )
                            .child(format!("PRICE {}", if self.sort_column == SortColumn::Price { if self.sort_ascending { "▲" } else { "▼" } } else { "" })),
                    )
                    // 24h Change column
                    .child(
                        div()
                            .w(px(if is_narrow { 60.0 } else { 70.0 }))
                            .text_right()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _win, cx| {
                                    this.toggle_sort(SortColumn::Change24h);
                                    cx.notify();
                                }),
                            )
                            .child(format!("24H % {}", if self.sort_column == SortColumn::Change24h { if self.sort_ascending { "▲" } else { "▼" } } else { "" })),
                    )
                    // Volume column
                    .when(!is_compact, |row| {
                        row.child(
                            div()
                                .w(px(70.0))
                                .text_right()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _win, cx| {
                                        this.toggle_sort(SortColumn::Volume24h);
                                        cx.notify();
                                    }),
                                )
                                .child(format!("24H VOL {}", if self.sort_column == SortColumn::Volume24h { if self.sort_ascending { "▲" } else { "▼" } } else { "" })),
                        )
                    }),
            )
            // Rows
            .when(filtered.is_empty(), |el| {
                el.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child(if query.is_empty() {
                            "No starred symbols yet. Use the search bar above to find and star symbols."
                        } else {
                            "No symbols match your search"
                        }),
                )
            })
            .when(!filtered.is_empty(), |el| {
                el.child(
                    v_flex()
                        .id("watchlist-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .children(
                            filtered
                                .into_iter()
                                .enumerate()
                                .map(|(idx, item)| self.render_item_row(&item, idx, is_compact, is_narrow, cx)),
                        ),
                )
            })
            ))
    }
}

impl WatchlistPanel {
    fn render_search_result(
        &self,
        item: &WatchlistEntry,
        idx: usize,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = &cx.theme().colors;
        let symbol = item.symbol.clone();
        let exchange = item.exchange.clone();
        let is_fav = self.is_favorite(&exchange, &symbol);
        h_flex()
            .id(ElementId::Name(format!("watchlist-search-{idx}").into()))
            .w_full()
            .px_2()
            .py_1p5()
            .items_center()
            .justify_between()
            .cursor_pointer()
            .border_b_1()
            .border_color(colors.border.opacity(0.5))
            .hover(|style| style.bg(colors.table_hover))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.toggle_favorite(&exchange, &symbol);
                cx.notify();
            }))
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().text_size(px(11.0)).font_weight(FontWeight::BOLD).text_color(colors.foreground).child(item.symbol.clone()))
                    .child(div().text_size(px(9.0)).text_color(colors.muted_foreground).child(item.exchange.to_uppercase())),
            )
            .child(div().text_size(px(13.0)).text_color(if is_fav { colors.warning } else { colors.muted_foreground }).child(if is_fav { "★" } else { "☆" }))
    }

    fn render_item_row(
        &self,
        item: &WatchlistEntry,
        idx: usize,
        is_compact: bool,
        is_narrow: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = &cx.theme().colors;
        let is_fav = self.is_favorite(&item.exchange, &item.symbol);
        let is_positive = item.change_24h >= 0.0;

        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        let sym_clone = item.symbol.clone();
        let exchange_clone = item.exchange.clone();

        h_flex()
            .id(ElementId::Integer(idx as u64))
            .w_full()
            .px_2()
            .py_1()
            .bg(row_bg)
            .gap_2()
            .items_center()
            .cursor_pointer()
            .hover(|s| s.bg(colors.table_hover))
            // Star Favorite Button
            .child(
                div()
                    .w(px(18.0))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _win, cx| {
                            this.toggle_favorite(&exchange_clone, &sym_clone);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(if is_fav {
                                colors.warning
                            } else {
                                colors.muted_foreground.opacity(0.4)
                            })
                            .child(if is_fav { "★" } else { "☆" }),
                    ),
            )
            // Asset badge & name
            .child(
                h_flex()
                    .w(px(if is_narrow { 75.0 } else { 95.0 }))
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child(item.base.clone()),
                    )
                    .when(!is_narrow, |h| {
                        h.child(
                            div()
                                .px_1()
                                .rounded(px(2.0))
                                .bg(colors.muted.opacity(0.4))
                                .text_size(px(8.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.muted_foreground)
                                .child(item.exchange.to_uppercase()),
                        )
                    }),
            )
            // Live Price
            .child(
                div()
                    .flex_1()
                    .text_right()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child(if item.initial_price > 0.0 {
                        format!("${}", Self::fmt_price(item.initial_price))
                    } else {
                        "—".to_string()
                    }),
            )
            // 24h Change %
            .child(
                h_flex()
                    .w(px(if is_narrow { 60.0 } else { 70.0 }))
                    .justify_end()
                    .child(
                        div()
                            .px_1p5()
                            .py(px(0.5))
                            .rounded(px(3.0))
                            .bg(if item.initial_price > 0.0 {
                                if is_positive {
                                    colors.bullish.opacity(0.15)
                                } else {
                                    colors.bearish.opacity(0.15)
                                }
                            } else {
                                colors.muted.opacity(0.2)
                            })
                            .text_size(px(10.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if item.initial_price > 0.0 {
                                if is_positive {
                                    colors.bullish
                                } else {
                                    colors.bearish
                                }
                            } else {
                                colors.muted_foreground
                            })
                            .child(if item.initial_price > 0.0 {
                                format!("{}{:.2}%", if is_positive { "+" } else { "" }, item.change_24h)
                            } else {
                                "—".to_string()
                            }),
                    ),
            )
            // 24h Volume
            .when(!is_compact, |row| {
                row.child(
                    div()
                        .w(px(70.0))
                        .text_right()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(colors.muted_foreground)
                        .child(if item.volume_24h_usd > 0.0 {
                            Self::fmt_vol(item.volume_24h_usd)
                        } else {
                            "—".to_string()
                        }),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

async fn run_watchlist_syncer(entries: Arc<Mutex<Vec<WatchlistEntry>>>, tracked: Arc<Mutex<HashSet<String>>>, cancel: CancellationToken) {
    let client = reqwest::Client::new();
    let base = crate::data::client::service_base_url();
    loop {
        let mut changed = false;
        let targets = {
            let tracked_set = tracked.lock().unwrap().clone();
            let all = entries.lock().unwrap().clone();
            all.into_iter()
                .filter(|e| {
                    let key = format!("{}:{}", e.exchange.to_lowercase(), e.symbol.to_uppercase());
                    tracked_set.contains(&key)
                        || tracked_set.contains(&e.symbol)
                        || (e.exchange.eq_ignore_ascii_case("binance") && tracked_set.contains(&e.symbol))
                })
                .collect::<Vec<_>>()
        };
        for entry in targets {
            if cancel.is_cancelled() { return; }
            let url = format!("{base}/api/v1/ticker?exchange={}&symbol={}", entry.exchange, entry.symbol);
            let mut req = client.get(&url).header("Accept", "application/json");
            if let Ok(Some(session)) = crate::data::session_store::load_session() {
                req = req.bearer_auth(session.access_token);
            }
            if let Ok(response) = req.send().await {
                if let Ok(value) = response.json::<serde_json::Value>().await {
                    let price = value.get("last").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if price > 0.0 {
                        let change = value.get("changePercent").or_else(|| value.get("percentage")).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let volume = value.get("quoteVolume").or_else(|| value.get("volume")).and_then(|v| v.as_f64()).unwrap_or(0.0);
                        if let Ok(mut current) = entries.lock() {
                            if let Some(row) = current.iter_mut().find(|row| row.symbol == entry.symbol && row.exchange == entry.exchange) {
                                row.initial_price = price;
                                row.change_24h = change;
                                row.volume_24h_usd = volume;
                                row.sparkline.push(price);
                                if row.sparkline.len() > 60 { row.sparkline.remove(0); }
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        if changed { /* next GPUI frame reads the shared snapshot */ }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }
    }
}

impl Drop for WatchlistPanel {
    fn drop(&mut self) { self._cancel_token.cancel(); }
}

impl Panel for WatchlistPanel {
    fn panel_name(&self) -> &'static str {
        "WatchlistPanel"
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.title.clone())
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
        false
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState {
            panel_name: "WatchlistPanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for WatchlistPanel {}

impl Focusable for WatchlistPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
