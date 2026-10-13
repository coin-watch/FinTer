/// Watchlist panel — multi-symbol real-time price list.
///
/// Displays a list of trading pairs with their current prices, 24h change,
/// and volume. Prices update in real-time from tick data.
///
/// # 60 FPS design
/// Each symbol row subscribes to its own tick feed and displays the latest
/// price. The panel requests a repaint on every frame for smooth updates.
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, ElementId, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};

use crate::data::feed::TickFeed;
use crate::data::tick::UnifiedTick;

// ---------------------------------------------------------------------------
// WatchlistItem — a single symbol in the watchlist
// ---------------------------------------------------------------------------

/// Configuration for a single watchlist item.
#[derive(Clone)]
pub struct WatchlistItem {
    /// Trading symbol (e.g. "BTC/USDT").
    pub symbol: String,
    /// Exchange ID (e.g. "binance").
    pub exchange: String,
    /// Tick feed for real-time price updates.
    pub feed: TickFeed,
}

impl WatchlistItem {
    pub fn new(symbol: &str, exchange: &str, feed: TickFeed) -> Self {
        Self {
            symbol: symbol.to_string(),
            exchange: exchange.to_string(),
            feed,
        }
    }

    /// Get the latest price from the tick feed.
    #[allow(dead_code)]
    fn latest_price(&self) -> Option<f64> {
        self.feed.snapshot().last().map(|t| t.price)
    }

    /// Get the latest tick from the feed.
    fn latest_tick(&self) -> Option<Arc<UnifiedTick>> {
        self.feed.snapshot().last().cloned()
    }
}

// ---------------------------------------------------------------------------
// WatchlistPanel
// ---------------------------------------------------------------------------

pub struct WatchlistPanel {
    /// List of symbols to watch.
    items: Vec<WatchlistItem>,
    /// Display title.
    title: String,
    focus_handle: FocusHandle,
}

impl WatchlistPanel {
    pub fn new(items: Vec<WatchlistItem>, title: impl Into<String>, cx: &mut App) -> Self {
        Self {
            items,
            title: title.into(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Format a price for display.
    fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{:.2}", price)
        } else if price >= 1.0 {
            format!("{:.4}", price)
        } else {
            format!("{:.6}", price)
        }
    }

    /// Render the column header row.
    fn render_header(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .bg(colors.table_head)
            .border_b_1()
            .border_color(colors.border)
            .gap_2()
            // Symbol column
            .child(
                div()
                    .w(px(120.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SYMBOL"),
            )
            // Price column
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("PRICE"),
            )
            // Side column (last trade direction)
            .child(
                div()
                    .w(px(50.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIDE"),
            )
            // Time column
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("TIME"),
            )
    }

    /// Render a single watchlist row.
    fn render_row(&self, item: &WatchlistItem, idx: usize, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let tick = item.latest_tick();

        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        // Default values if no tick data
        let (price_str, side_str, side_color, time_str) = if let Some(tick) = tick {
            let is_buy = tick.side.is_buy();
            let color = if is_buy {
                colors.bullish
            } else {
                colors.bearish
            };
            let side = if is_buy { "BUY" } else { "SELL" };

            // Format time as HH:MM:SS
            let ts_secs = tick.timestamp_ms / 1_000;
            let h = (ts_secs / 3600) % 24;
            let m = (ts_secs / 60) % 60;
            let s = ts_secs % 60;
            let time = format!("{:02}:{:02}:{:02}", h, m, s);

            (Self::fmt_price(tick.price), side.to_string(), color, time)
        } else {
            (
                "—".to_string(),
                "—".to_string(),
                colors.muted_foreground,
                "—".to_string(),
            )
        };

        h_flex()
            .id(ElementId::Integer(idx as u64))
            .w_full()
            .px_3()
            .py_2()
            .bg(row_bg)
            .gap_2()
            .cursor_pointer()
            .hover(|style| style.bg(colors.table_hover))
            // Symbol
            .child(
                div()
                    .w(px(120.0))
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .child(item.symbol.clone()),
            )
            // Price
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(side_color)
                    .child(price_str),
            )
            // Side
            .child(
                div()
                    .w(px(50.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(side_color)
                    .child(side_str),
            )
            // Time
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(time_str),
            )
    }

    /// Render an empty-state placeholder.
    fn render_empty(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(colors.muted_foreground)
            .child("No symbols in watchlist")
    }
}

impl Render for WatchlistPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Schedule a repaint on the next frame for live updates.
        window.request_animation_frame();

        v_flex()
            .size_full()
            .bg(cx.theme().colors.background)
            .child(Self::render_header(cx))
            .when(self.items.is_empty(), |el: gpui::Div| {
                el.child(Self::render_empty(cx))
            })
            .when(!self.items.is_empty(), |el: gpui::Div| {
                el.child(
                    v_flex()
                        .id("watchlist-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .children(
                            self.items
                                .iter()
                                .enumerate()
                                .map(|(i, item)| self.render_row(item, i, cx)),
                        ),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

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
        let symbols: Vec<String> = self.items.iter().map(|i| i.symbol.clone()).collect();
        PanelState {
            panel_name: "WatchlistPanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
                "symbols": symbols,
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

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_price_adapts_precision() {
        assert_eq!(WatchlistPanel::fmt_price(50_000.0), "50000.00");
        assert_eq!(WatchlistPanel::fmt_price(1.5), "1.5000");
        assert_eq!(WatchlistPanel::fmt_price(0.000123), "0.000123");
    }
}
