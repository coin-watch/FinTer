/// Order Book panel — real-time Level-2 depth ladder.
///
/// Renders the latest `OrderBook` snapshot as a stacked depth ladder:
/// asks above the spread, bids below.  Each row shows a proportional size
/// bar behind the price/quantity text for instant visual depth reading.
///
/// # 60 FPS design
/// `render()` calls `OrderBookFeed::snapshot()` (brief mutex lock) and lays
/// out the result.  No heavy computation on the render thread.
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, ElementId, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Styled, Window,
    div, px,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};

use crate::data::orderbook::{OrderBook, PriceLevel};
use crate::data::orderbook_feed::OrderBookFeed;

// ---------------------------------------------------------------------------
// Display constants
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// OrderBookPanel
// ---------------------------------------------------------------------------

pub struct OrderBookPanel {
    feed: OrderBookFeed,
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
}

impl OrderBookPanel {
    pub fn new(feed: OrderBookFeed, title: impl Into<String>, cx: &mut App) -> Self {
        Self {
            feed,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 300.0,
            height: 350.0,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        if (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5 {
            return;
        }
        self.width = width;
        self.height = height;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
    }

    /// Format a price for display, adapting precision to magnitude.
    fn fmt_price(price: f64) -> String {
        if price >= 10_000.0 {
            format!("{price:.1}")
        } else if price >= 1_000.0 {
            format!("{price:.2}")
        } else if price >= 1.0 {
            format!("{price:.4}")
        } else {
            format!("{price:.6}")
        }
    }

    /// Format a quantity for display.
    fn fmt_qty(qty: f64) -> String {
        if qty >= 100.0 {
            format!("{qty:.2}")
        } else if qty >= 1.0 {
            format!("{qty:.4}")
        } else {
            format!("{qty:.6}")
        }
    }

    /// Render a single level row with an inline size bar.
    ///
    /// `bar_frac` is the fraction [0, 1] of the maximum quantity on either
    /// side — used to set the proportional width of the background size bar.
    fn render_level_row(
        level: &PriceLevel,
        idx: usize,
        bar_frac: f64,
        price_color: gpui::Hsla,
        bar_color: gpui::Hsla,
        id_prefix: &'static str,
        row_h: f32,
        qty_w: f32,
        font_size: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let price_str = Self::fmt_price(level.price);
        let qty_str = Self::fmt_qty(level.quantity);

        div()
            .id(ElementId::Name(format!("{id_prefix}-{idx}").into()))
            .w_full()
            .h(px(row_h))
            .relative()
            // Size bar dynamically spans proportional percentage of the module width
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui::relative(bar_frac.clamp(0.01, 1.0) as f32))
                    .bg(bar_color),
            )
            // Text row (relative, on top of bar)
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .px_2()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    // Price
                    .child(
                        div()
                            .flex_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(font_size))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(price_color)
                            .child(price_str),
                    )
                    // Quantity
                    .child(
                        div()
                            .w(px(qty_w))
                            .text_align(gpui::TextAlign::Right)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(font_size))
                            .text_color(colors.muted_foreground)
                            .child(qty_str),
                    ),
            )
    }

    /// Render the column header.
    fn render_header(qty_w: f32, font_size: f32, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        h_flex()
            .w_full()
            .px_2()
            .py(px(3.0))
            .bg(colors.table_head)
            .border_b_1()
            .border_color(colors.border)
            .gap_2()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex_1()
                    .text_size(px(font_size))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("PRICE"),
            )
            .child(
                div()
                    .w(px(qty_w))
                    .text_align(gpui::TextAlign::Right)
                    .text_size(px(font_size))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("QUANTITY"),
            )
    }

    /// Render the spread row between asks and bids.
    fn render_spread(book: &Arc<OrderBook>, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let spread_str = book.spread().map_or("—".to_string(), |s| {
            if s < 1.0 {
                format!("Spread: {s:.4}")
            } else {
                format!("Spread: {s:.2}")
            }
        });

        let mid_str = book
            .mid_price()
            .map_or(String::new(), |m| format!("Mid: {}", Self::fmt_price(m)));

        h_flex()
            .w_full()
            .px_2()
            .py(px(3.0))
            .bg(Hsla {
                a: 0.12,
                ..colors.warning
            })
            .border_y_1()
            .border_color(colors.border.opacity(0.6))
            .gap_2()
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.warning)
                    .child(spread_str),
            )
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(11.0))
                    .text_color(colors.muted_foreground)
                    .child(mid_str),
            )
    }

    /// Render the full depth ladder dynamically fitted to the pane dimensions.
    fn render_book(&self, book: &Arc<OrderBook>, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let row_h = if self.height < 240.0 { 16.0 } else { 18.0 };
        let qty_w = if self.width < 220.0 { 60.0 } else { 80.0 };
        let font_size = if self.width < 220.0 { 10.5 } else { 11.5 };

        let is_dual_column = self.width >= 380.0 && self.height < 320.0;

        if is_dual_column {
            // Dual-column mode for wide short panes: BIDS on left, ASKS on right
            let depth = (((self.height - 52.0) / row_h).floor() as usize).clamp(3, 30);
            let max_qty = book.max_quantity(depth).max(f64::EPSILON);
            let asks = book.top_asks(depth);
            let bids = book.top_bids(depth);

            v_flex()
                .size_full()
                .overflow_hidden()
                .child(Self::render_spread(book, cx))
                .child(
                    h_flex()
                        .flex_1()
                        .w_full()
                        .min_h_0()
                        .child(
                            // Bids column (green)
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .border_r_1()
                                .border_color(colors.border)
                                .child(Self::render_header(qty_w, font_size - 1.0, cx))
                                .children(bids.iter().enumerate().map(|(i, level)| {
                                    let frac = level.quantity / max_qty;
                                    Self::render_level_row(
                                        level,
                                        i,
                                        frac,
                                        colors.bullish,
                                        Hsla { a: 0.22, ..colors.bullish },
                                        "bid",
                                        row_h,
                                        qty_w,
                                        font_size,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            // Asks column (red)
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .child(Self::render_header(qty_w, font_size - 1.0, cx))
                                .children(asks.iter().enumerate().map(|(i, level)| {
                                    let frac = level.quantity / max_qty;
                                    Self::render_level_row(
                                        level,
                                        i,
                                        frac,
                                        colors.bearish,
                                        Hsla { a: 0.22, ..colors.bearish },
                                        "ask",
                                        row_h,
                                        qty_w,
                                        font_size,
                                        cx,
                                    )
                                })),
                        ),
                )
        } else {
            // Stacked depth ladder: Asks at top, Spread in center, Bids at bottom
            let available_h = (self.height - 24.0 - 24.0).max(36.0);
            let depth = (((available_h / (row_h * 2.0))).floor() as usize).clamp(3, 35);

            let max_qty = book.max_quantity(depth).max(f64::EPSILON);
            let asks = book.top_asks(depth);
            let bids = book.top_bids(depth);
            let asks_reversed: Vec<&PriceLevel> = asks.iter().rev().collect();

            v_flex()
                .size_full()
                .overflow_hidden()
                // Asks section (reversed — worst ask at top)
                .children(asks_reversed.iter().enumerate().map(|(i, level)| {
                    let frac = level.quantity / max_qty;
                    Self::render_level_row(
                        level,
                        i,
                        frac,
                        colors.bearish,
                        Hsla {
                            a: 0.22,
                            ..colors.bearish
                        },
                        "ask",
                        row_h,
                        qty_w,
                        font_size,
                        cx,
                    )
                }))
                // Spread / mid price separator
                .child(Self::render_spread(book, cx))
                // Bids section
                .children(bids.iter().enumerate().map(|(i, level)| {
                    let frac = level.quantity / max_qty;
                    Self::render_level_row(
                        level,
                        i,
                        frac,
                        colors.bullish,
                        Hsla {
                            a: 0.22,
                            ..colors.bullish
                        },
                        "bid",
                        row_h,
                        qty_w,
                        font_size,
                        cx,
                    )
                }))
        }
    }

    /// Render empty-state placeholder.
    fn render_empty(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .text_color(colors.muted_foreground)
            .child("Waiting for order book…")
    }
}

impl Render for OrderBookPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep repainting at 60 FPS.
        window.request_animation_frame();

        let snapshot = self.feed.snapshot();
        let qty_w = if self.width < 220.0 { 60.0 } else { 80.0 };
        let font_size = if self.width < 220.0 { 10.5 } else { 11.5 };
        let is_dual_column = self.width >= 380.0 && self.height < 320.0;

        v_flex()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().colors.background)
            .when(!is_dual_column, |this| this.child(Self::render_header(qty_w, font_size, cx)))
            .when(snapshot.is_none(), |el: gpui::Div| {
                el.child(Self::render_empty(cx))
            })
            .when_some(snapshot, |el, book| {
                el.child(
                    div()
                        .size_full()
                        .overflow_hidden()
                        .child(self.render_book(&book, cx)),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

impl Panel for OrderBookPanel {
    fn panel_name(&self) -> &'static str {
        "OrderBookPanel"
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
            panel_name: "OrderBookPanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for OrderBookPanel {}

impl Focusable for OrderBookPanel {
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
        assert_eq!(OrderBookPanel::fmt_price(67_123.5), "67123.5");
        assert_eq!(OrderBookPanel::fmt_price(1_234.56), "1234.56");
        assert_eq!(OrderBookPanel::fmt_price(12.3456), "12.3456");
        assert_eq!(OrderBookPanel::fmt_price(0.001234), "0.001234");
    }

    #[test]
    fn fmt_qty_adapts_precision() {
        assert_eq!(OrderBookPanel::fmt_qty(500.0), "500.00");
        assert_eq!(OrderBookPanel::fmt_qty(1.5), "1.5000");
        assert_eq!(OrderBookPanel::fmt_qty(0.0001), "0.000100");
    }

    #[test]
    fn spread_display_uses_book() {
        use crate::data::orderbook::{OrderBook, PriceLevel};
        let book = Arc::new(OrderBook::new(
            "binance",
            "BTC/USDT",
            vec![PriceLevel::new(67_000.0, 1.0)],
            vec![PriceLevel::new(67_010.0, 1.0)],
            0,
        ));
        assert!((book.spread().unwrap() - 10.0).abs() < f64::EPSILON);
        assert!((book.mid_price().unwrap() - 67_005.0).abs() < f64::EPSILON);
    }
}
