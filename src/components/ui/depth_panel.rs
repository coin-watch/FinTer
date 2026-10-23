/// Depth panel — real-time market depth chart visualization.
///
/// Subscribes to an `OrderBookFeed` and renders a stepped cumulative depth
/// curve for bids (buy wall) and asks (sell wall) meeting at the mid price.
///
/// # 60 FPS design
/// Takes a cheap `snapshot()` of the latest order book on each render pass.
/// No allocations occur on the hot path except formatting the visual steps.
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, div, px, relative,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};

use crate::data::orderbook_feed::OrderBookFeed;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const NUM_DEPTH_STEPS: usize = 28;

// ---------------------------------------------------------------------------
// DepthPanel
// ---------------------------------------------------------------------------

pub struct DepthPanel {
    feed: OrderBookFeed,
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    hovered_step: Option<(bool, f64, f64)>, // (is_bid, price, cum_vol)
}

impl DepthPanel {
    pub fn new(feed: OrderBookFeed, title: impl Into<String>, cx: &mut Context<Self>) -> Self {
        // Coalesce external updates; weak access ends the task on panel close.
        let feed_for_timer = feed.clone();
        cx.spawn(async move |this, cx| {
            let mut last = feed_for_timer.snapshot();
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                let latest = feed_for_timer.snapshot();
                let changed = match (&last, &latest) {
                    (Some(previous), Some(current)) => !std::sync::Arc::ptr_eq(previous, current),
                    (None, None) => false,
                    _ => true,
                };
                if changed { last = latest; }
                if this.update(cx, |_, cx| {
                    if changed { cx.notify(); }
                }).is_err() { break; }
            }
        }).detach();
        Self {
            feed,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 0.0,
            height: 0.0,
            hovered_step: None,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
    }

    fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{:.2}", price)
        } else if price >= 1.0 {
            format!("{:.4}", price)
        } else if price >= 0.0001 {
            format!("{:.6}", price)
        } else {
            format!("{:.8}", price)
        }
    }

    fn fmt_vol(vol: f64) -> String {
        if vol >= 1_000_000.0 {
            format!("{:.2}M", vol / 1_000_000.0)
        } else if vol >= 1_000.0 {
            format!("{:.2}K", vol / 1_000.0)
        } else if vol >= 1.0 {
            format!("{:.3}", vol)
        } else {
            format!("{:.4}", vol)
        }
    }
}

impl Render for DepthPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {


        let theme = cx.theme();
        let colors = &theme.colors;
        let snapshot = self.feed.snapshot();

        let is_compact = self.width < 340.0 || self.height < 220.0;

        v_flex()
            .size_full()
            .bg(colors.background)
            .overflow_hidden()
            .when(snapshot.is_none(), |el| {
                el.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.muted_foreground)
                                .child("Connecting to Depth Stream…"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.muted_foreground.opacity(0.6))
                                .child("Aggregating live bid and ask levels"),
                        ),
                )
            })
            .when_some(snapshot, |el, ob| {
                let best_bid = ob.best_bid().unwrap_or(0.0);
                let best_ask = ob.best_ask().unwrap_or(0.0);
                let mid_price = if best_bid > 0.0 && best_ask > 0.0 {
                    (best_bid + best_ask) / 2.0
                } else if best_bid > 0.0 {
                    best_bid
                } else {
                    best_ask
                };

                let spread = if best_bid > 0.0 && best_ask > 0.0 {
                    (best_ask - best_bid).max(0.0)
                } else {
                    0.0
                };

                let spread_pct = if mid_price > 0.0 {
                    (spread / mid_price) * 100.0
                } else {
                    0.0
                };

                // Compute cumulative depth curves
                // Bids sorted descending
                let mut bid_levels = ob.bids.clone();
                bid_levels.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap_or(std::cmp::Ordering::Equal));

                let mut ask_levels = ob.asks.clone();
                ask_levels.sort_by(|a, b| a.price.partial_cmp(&b.price).unwrap_or(std::cmp::Ordering::Equal));

                let mut bid_cum = 0.0;
                let mut bid_points: Vec<(f64, f64)> = Vec::with_capacity(bid_levels.len());
                for lvl in &bid_levels {
                    bid_cum += lvl.quantity;
                    bid_points.push((lvl.price, bid_cum));
                }

                let mut ask_cum = 0.0;
                let mut ask_points: Vec<(f64, f64)> = Vec::with_capacity(ask_levels.len());
                for lvl in &ask_levels {
                    ask_cum += lvl.quantity;
                    ask_points.push((lvl.price, ask_cum));
                }

                let total_bid_vol = bid_cum;
                let total_ask_vol = ask_cum;
                let total_depth = (total_bid_vol + total_ask_vol).max(1e-6);
                let bid_ratio = (total_bid_vol / total_depth) * 100.0;
                let ask_ratio = 100.0 - bid_ratio;

                let max_cum_vol = total_bid_vol.max(total_ask_vol).max(1e-6);

                // Sample discrete steps for smooth UI rendering
                let steps = NUM_DEPTH_STEPS;
                let mut bid_bars: Vec<(f64, f64, f32)> = Vec::with_capacity(steps);
                let mut ask_bars: Vec<(f64, f64, f32)> = Vec::with_capacity(steps);

                // For bids: sample from furthest price to best bid (left to right, towards mid)
                if !bid_points.is_empty() {
                    let total_pts = bid_points.len();
                    for i in 0..steps {
                        // idx goes from end of points (lowest price, highest cum) to 0 (highest price, lowest cum)
                        let progress = (steps - 1 - i) as f64 / (steps.max(1) as f64);
                        let idx = ((progress * (total_pts - 1) as f64).round() as usize).min(total_pts - 1);
                        let (p, c) = bid_points[idx];
                        let height_pct = ((c / max_cum_vol) as f32).clamp(0.02, 1.0);
                        bid_bars.push((p, c, height_pct));
                    }
                }

                // For asks: sample from best ask to furthest ask (left to right, away from mid)
                if !ask_points.is_empty() {
                    let total_pts = ask_points.len();
                    for i in 0..steps {
                        let progress = i as f64 / (steps.max(1) as f64);
                        let idx = ((progress * (total_pts - 1) as f64).round() as usize).min(total_pts - 1);
                        let (p, c) = ask_points[idx];
                        let height_pct = ((c / max_cum_vol) as f32).clamp(0.02, 1.0);
                        ask_bars.push((p, c, height_pct));
                    }
                }

                el
                    // Header telemetry
                    .child(
                        h_flex()
                            .w_full()
                            .px_3()
                            .py_1p5()
                            .bg(colors.table_head)
                            .border_b_1()
                            .border_color(colors.border)
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    // Status pill
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_1()
                                            .px_1p5()
                                            .py(px(2.0))
                                            .rounded(px(3.0))
                                            .bg(colors.bullish.opacity(0.12))
                                            .border_1()
                                            .border_color(colors.bullish.opacity(0.25))
                                            .child(
                                                div()
                                                    .size(px(6.0))
                                                    .rounded_full()
                                                    .bg(colors.bullish),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.bullish)
                                                    .child("DEPTH"),
                                            ),
                                    )
                                    // Mid Price
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.foreground)
                                            .child(format!("${}", Self::fmt_price(mid_price))),
                                    )
                                    // Spread
                                    .when(!is_compact, |row| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.0))
                                                .rounded(px(3.0))
                                                .bg(colors.muted.opacity(0.4))
                                                .text_size(px(11.0))
                                                .text_color(colors.muted_foreground)
                                                .child(format!("Spread: ${} ({:.3}%)", Self::fmt_price(spread), spread_pct)),
                                        )
                                    }),
                            )
                            // Imbalance ratio bar
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .when(!is_compact, |row| {
                                        row.child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(colors.bullish)
                                                .child(format!("{:.1}%", bid_ratio)),
                                        )
                                    })
                                    .child(
                                        h_flex()
                                            .w(px(if is_compact { 80.0 } else { 120.0 }))
                                            .h(px(6.0))
                                            .rounded(px(3.0))
                                            .overflow_hidden()
                                            .bg(colors.bearish.opacity(0.85))
                                            .child(
                                                div()
                                                    .h_full()
                                                    .w(relative(bid_ratio as f32 / 100.0))
                                                    .bg(colors.bullish),
                                            ),
                                    )
                                    .when(!is_compact, |row| {
                                        row.child(
                                            div()
                                                .text_size(px(11.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(colors.bearish)
                                                .child(format!("{:.1}%", ask_ratio)),
                                        )
                                    }),
                            ),
                    )
                    // Depth Chart Visualization
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .relative()
                            .overflow_hidden()
                            // Horizontal grid guides (25%, 50%, 75%)
                            .child(
                                div()
                                    .absolute()
                                    .top(relative(0.25))
                                    .left_0()
                                    .right_0()
                                    .h(px(1.0))
                                    .bg(colors.border.opacity(0.35)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(relative(0.50))
                                    .left_0()
                                    .right_0()
                                    .h(px(1.0))
                                    .bg(colors.border.opacity(0.35)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(relative(0.75))
                                    .left_0()
                                    .right_0()
                                    .h(px(1.0))
                                    .bg(colors.border.opacity(0.35)),
                            )
                            // Mid-price vertical center separator
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(relative(0.5))
                                    .w(px(1.0))
                                    .bg(colors.border.opacity(0.8)),
                            )
                            // Stepped Depth Walls Container
                            .child(
                                h_flex()
                                    .size_full()
                                    .items_end()
                                    // BIDS WALL (Left half, green)
                                    .child(
                                        h_flex()
                                            .w(relative(0.5))
                                            .h_full()
                                            .items_end()
                                            .justify_between()
                                            .gap(px(1.0))
                                            .children(
                                                bid_bars.into_iter().enumerate().map(|(i, (p, _c, h_pct))| {
                                                    let is_hovered = self.hovered_step.map(|(b, hp, _)| b && (hp - p).abs() < 1e-5).unwrap_or(false);
                                                    div()
                                                        .id(ElementId::Integer(1000 + i as u64))
                                                        .flex_1()
                                                        .h(relative(h_pct))
                                                        .bg(if is_hovered { colors.bullish.opacity(0.7) } else { colors.bullish.opacity(0.32) })
                                                        .border_t_2()
                                                        .border_color(colors.bullish)
                                                        .cursor_pointer()
                                                        .hover(|s| s.bg(colors.bullish.opacity(0.55)))
                                                }),
                                            ),
                                    )
                                    // ASKS WALL (Right half, red)
                                    .child(
                                        h_flex()
                                            .w(relative(0.5))
                                            .h_full()
                                            .items_end()
                                            .justify_between()
                                            .gap(px(1.0))
                                            .children(
                                                ask_bars.into_iter().enumerate().map(|(i, (p, _c, h_pct))| {
                                                    let is_hovered = self.hovered_step.map(|(b, hp, _)| !b && (hp - p).abs() < 1e-5).unwrap_or(false);
                                                    div()
                                                        .id(ElementId::Integer(2000 + i as u64))
                                                        .flex_1()
                                                        .h(relative(h_pct))
                                                        .bg(if is_hovered { colors.bearish.opacity(0.7) } else { colors.bearish.opacity(0.32) })
                                                        .border_t_2()
                                                        .border_color(colors.bearish)
                                                        .cursor_pointer()
                                                        .hover(|s| s.bg(colors.bearish.opacity(0.55)))
                                                }),
                                            ),
                                    ),
                            )
                            // Top corner labels (Max Depth volume)
                            .child(
                                div()
                                    .absolute()
                                    .top_2()
                                    .left_3()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.bullish.opacity(0.85))
                                    .child(format!("BIDS: {}", Self::fmt_vol(total_bid_vol))),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_2()
                                    .right_3()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.bearish.opacity(0.85))
                                    .child(format!("ASKS: {}", Self::fmt_vol(total_ask_vol))),
                            ),
                    )
                    // Bottom price axis
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(22.0))
                            .px_3()
                            .bg(colors.table_head)
                            .border_t_1()
                            .border_color(colors.border)
                            .justify_between()
                            .items_center()
                            .text_size(px(10.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.muted_foreground)
                            .child(
                                div()
                                    .text_color(colors.bullish.opacity(0.85))
                                    .child(format!("${}", Self::fmt_price(mid_price * 0.98))),
                            )
                            .child(
                                div()
                                    .text_color(colors.bullish.opacity(0.85))
                                    .child(format!("${}", Self::fmt_price(mid_price * 0.99))),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.foreground)
                                    .child(format!("${}", Self::fmt_price(mid_price))),
                            )
                            .child(
                                div()
                                    .text_color(colors.bearish.opacity(0.85))
                                    .child(format!("${}", Self::fmt_price(mid_price * 1.01))),
                            )
                            .child(
                                div()
                                    .text_color(colors.bearish.opacity(0.85))
                                    .child(format!("${}", Self::fmt_price(mid_price * 1.02))),
                            ),
                    )
            })
    }
}

impl Panel for DepthPanel {
    fn panel_name(&self) -> &'static str {
        "DepthPanel"
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
            panel_name: "DepthPanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for DepthPanel {}

impl Focusable for DepthPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
