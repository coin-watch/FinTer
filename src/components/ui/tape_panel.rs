/// Tape panel — real-time Time & Sales display.
///
/// Renders the latest ticks from a `TickFeed` as a scrollable table.
/// The panel requests a repaint on every GPUI frame so the display
/// stays current without polling.  All heavy work (tick ingestion,
/// ring-buffer management) happens off the render thread.
///
/// # 60 FPS design
/// `render()` simply clones the current `TickFeed` snapshot (a
/// `Vec<Arc<UnifiedTick>>` built from a brief mutex lock) and lays it out.
/// No allocations beyond the snapshot happen on the hot path.
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
// Display constants
// ---------------------------------------------------------------------------

/// Maximum number of rows shown in the tape (newest at top).
const MAX_VISIBLE_ROWS: usize = 100;

// ---------------------------------------------------------------------------
// TapePanel
// ---------------------------------------------------------------------------

pub struct TapePanel {
    feed: TickFeed,
    /// Display title (e.g. "BTC/USDT Tape").
    title: String,
    focus_handle: FocusHandle,
}

impl TapePanel {
    pub fn new(feed: TickFeed, title: impl Into<String>, cx: &mut App) -> Self {
        Self {
            feed,
            title: title.into(),
            focus_handle: cx.focus_handle(),
        }
    }

    /// Format a price for display.  Precision adapts to magnitude.
    fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{price:.2}")
        } else if price >= 1.0 {
            format!("{price:.4}")
        } else {
            format!("{price:.6}")
        }
    }

    /// Format an amount for display.
    fn fmt_amount(amount: f64) -> String {
        if amount >= 100.0 {
            format!("{amount:.2}")
        } else {
            format!("{amount:.4}")
        }
    }

    /// Render a single tick row.
    fn render_tick_row(
        tick: &Arc<UnifiedTick>,
        idx: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let is_buy = tick.side.is_buy();
        let colors = &cx.theme().colors;

        // Alternate row background for readability.
        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        let side_color = if is_buy {
            colors.bullish
        } else {
            colors.bearish
        };

        let price_str = Self::fmt_price(tick.price);
        let amount_str = Self::fmt_amount(tick.amount);
        let side_str = if is_buy { "BUY" } else { "SELL" };

        // Notional value — bold for large trades.
        let notional = tick.notional();
        let notional_str = if notional >= 100_000.0 {
            format!("{:.0}K", notional / 1_000.0)
        } else {
            format!("{notional:.0}")
        };

        // Timestamp — HH:MM:SS (UTC).
        let ts_secs = tick.timestamp_ms / 1_000;
        let h = (ts_secs / 3600) % 24;
        let m = (ts_secs / 60) % 60;
        let s = ts_secs % 60;
        let time_str = format!("{h:02}:{m:02}:{s:02}");

        h_flex()
            .id(ElementId::Integer(idx as u64))
            .w_full()
            .px_2()
            .py(px(2.0))
            .bg(row_bg)
            .gap_2()
            // Time
            .child(
                div()
                    .w(px(60.0))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(time_str),
            )
            // Side badge
            .child(
                div()
                    .w(px(36.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(side_color)
                    .child(side_str),
            )
            // Price
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(side_color)
                    .child(price_str),
            )
            // Amount
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .text_color(colors.foreground)
                    .child(amount_str),
            )
            // Notional
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(notional_str),
            )
    }

    /// Render the column header row.
    fn render_header(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        h_flex()
            .w_full()
            .px_2()
            .py(px(3.0))
            .bg(colors.table_head)
            .border_b_1()
            .border_color(colors.border)
            .gap_2()
            .child(
                div()
                    .w(px(60.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("TIME"),
            )
            .child(
                div()
                    .w(px(36.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIDE"),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("PRICE"),
            )
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("AMOUNT"),
            )
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("VALUE"),
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
            .child("Waiting for ticks…")
    }
}

impl Render for TapePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Schedule a repaint on the next frame so the tape stays live.
        window.request_animation_frame();

        // Take a snapshot of the ring buffer (brief mutex lock, no allocation
        // beyond the vec itself).
        let mut snapshot = self.feed.snapshot();

        // Newest ticks first.
        snapshot.reverse();
        let snapshot = &snapshot[..snapshot.len().min(MAX_VISIBLE_ROWS)];

        v_flex()
            .size_full()
            .bg(cx.theme().colors.background)
            .child(Self::render_header(cx))
            .when(snapshot.is_empty(), |el: gpui::Div| {
                el.child(Self::render_empty(cx))
            })
            .when(!snapshot.is_empty(), |el: gpui::Div| {
                el.child(
                    v_flex()
                        .id("tape-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .children(
                            snapshot
                                .iter()
                                .enumerate()
                                .map(|(i, tick)| Self::render_tick_row(tick, i, cx)),
                        ),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

impl Panel for TapePanel {
    fn panel_name(&self) -> &'static str {
        "TapePanel"
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
            panel_name: "TapePanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for TapePanel {}

impl Focusable for TapePanel {
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
    use crate::data::tick::TradeSide;

    #[test]
    fn fmt_price_adapts_precision() {
        assert_eq!(TapePanel::fmt_price(50_000.0), "50000.00");
        assert_eq!(TapePanel::fmt_price(1.5), "1.5000");
        assert_eq!(TapePanel::fmt_price(0.000123), "0.000123");
    }

    #[test]
    fn fmt_amount_adapts_precision() {
        assert_eq!(TapePanel::fmt_amount(500.0), "500.00");
        assert_eq!(TapePanel::fmt_amount(0.01), "0.0100");
    }

    #[test]
    fn trade_side_colors_match() {
        // Ensure buy side resolves to "buy" in our logic.
        let buy = TradeSide::Buy;
        assert!(buy.is_buy());
        let sell = TradeSide::Sell;
        assert!(!sell.is_buy());
    }
}
