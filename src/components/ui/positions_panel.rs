/// Positions panel — real-time position and P&L tracker.
///
/// Displays open trading positions with entry price, current price, size,
/// unrealized P&L, and percentage gain/loss. Updates in real-time from tick data.
///
/// # 60 FPS design
/// Each position row calculates unrealized P&L based on the latest tick price.
/// The panel requests a repaint on every frame for smooth updates.
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

// ---------------------------------------------------------------------------
// Position — a single trading position
// ---------------------------------------------------------------------------

/// A single open trading position.
#[derive(Clone)]
pub struct Position {
    /// Trading symbol (e.g. "BTC/USDT").
    pub symbol: String,
    /// Position side: long (+) or short (-).
    pub side: PositionSide,
    /// Entry price.
    pub entry_price: f64,
    /// Position size (in base currency).
    pub size: f64,
    /// Tick feed for real-time price updates.
    pub feed: TickFeed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Long,
    Short,
}

impl Position {
    pub fn new(
        symbol: &str,
        side: PositionSide,
        entry_price: f64,
        size: f64,
        feed: TickFeed,
    ) -> Self {
        Self {
            symbol: symbol.to_string(),
            side,
            entry_price,
            size,
            feed,
        }
    }

    /// Get the latest price from the tick feed.
    fn current_price(&self) -> Option<f64> {
        self.feed.snapshot().last().map(|t| t.price)
    }

    /// Calculate unrealized P&L based on current price.
    fn unrealized_pnl(&self, current_price: f64) -> f64 {
        let price_diff = current_price - self.entry_price;
        match self.side {
            PositionSide::Long => price_diff * self.size,
            PositionSide::Short => -price_diff * self.size,
        }
    }

    /// Calculate P&L percentage.
    fn pnl_percentage(&self, current_price: f64) -> f64 {
        let price_diff = current_price - self.entry_price;
        match self.side {
            PositionSide::Long => (price_diff / self.entry_price) * 100.0,
            PositionSide::Short => (-price_diff / self.entry_price) * 100.0,
        }
    }

    /// Position notional value.
    #[allow(dead_code)]
    fn notional(&self, current_price: f64) -> f64 {
        self.size * current_price
    }
}

// ---------------------------------------------------------------------------
// PositionsPanel
// ---------------------------------------------------------------------------

pub struct PositionsPanel {
    /// List of open positions.
    positions: Vec<Position>,
    /// Display title.
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
}

impl PositionsPanel {
    pub fn new(positions: Vec<Position>, title: impl Into<String>, cx: &mut App) -> Self {
        Self {
            positions,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 0.0,
            height: 0.0,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
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

    /// Format P&L for display with sign.
    fn fmt_pnl(pnl: f64) -> String {
        if pnl >= 0.0 {
            format!("+{:.2}", pnl)
        } else {
            format!("{:.2}", pnl)
        }
    }

    /// Format percentage for display.
    fn fmt_pct(pct: f64) -> String {
        if pct >= 0.0 {
            format!("+{:.2}%", pct)
        } else {
            format!("{:.2}%", pct)
        }
    }

    /// Format size for display.
    fn fmt_size(size: f64) -> String {
        if size >= 100.0 {
            format!("{:.2}", size)
        } else {
            format!("{:.4}", size)
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
                    .w(px(100.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SYMBOL"),
            )
            // Side column
            .child(
                div()
                    .w(px(50.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIDE"),
            )
            // Size column
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIZE"),
            )
            // Entry column
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("ENTRY"),
            )
            // Current column
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("CURRENT"),
            )
            // P&L column
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("UNREALIZED P&L"),
            )
    }

    /// Render a single position row.
    fn render_row(&self, position: &Position, idx: usize, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let current_price = position.current_price();

        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        // Side display
        let (side_str, side_color) = match position.side {
            PositionSide::Long => ("LONG", colors.bullish),
            PositionSide::Short => ("SHORT", colors.bearish),
        };

        // Calculate P&L if we have a current price
        let (current_str, pnl_str, pct_str, pnl_color) = if let Some(price) = current_price {
            let pnl = position.unrealized_pnl(price);
            let pct = position.pnl_percentage(price);
            let color = if pnl >= 0.0 {
                colors.bullish
            } else {
                colors.bearish
            };
            (
                Self::fmt_price(price),
                Self::fmt_pnl(pnl),
                Self::fmt_pct(pct),
                color,
            )
        } else {
            (
                "—".to_string(),
                "—".to_string(),
                "—".to_string(),
                colors.muted_foreground,
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
                    .w(px(100.0))
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .child(position.symbol.clone()),
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
            // Size
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .text_color(colors.foreground)
                    .child(Self::fmt_size(position.size)),
            )
            // Entry
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(Self::fmt_price(position.entry_price)),
            )
            // Current
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .text_color(colors.foreground)
                    .child(current_str),
            )
            // P&L
            .child(
                h_flex()
                    .flex_1()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(pnl_color)
                            .child(pnl_str),
                    )
                    .child(div().text_xs().text_color(pnl_color).child(pct_str)),
            )
    }

    /// Render total P&L summary.
    fn render_summary(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let total_pnl: f64 = self
            .positions
            .iter()
            .filter_map(|p| p.current_price().map(|price| p.unrealized_pnl(price)))
            .sum();

        let pnl_color = if total_pnl >= 0.0 {
            colors.bullish
        } else {
            colors.bearish
        };

        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .bg(colors.table_head)
            .border_t_1()
            .border_color(colors.border)
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child(format!("TOTAL ({} positions)", self.positions.len())),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(pnl_color)
                    .child(Self::fmt_pnl(total_pnl)),
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
            .p_4()
            .text_center()
            .child("Account positions unavailable · No account position API is connected.")
    }
}

impl Render for PositionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Schedule a repaint on the next frame for live P&L updates.
        window.request_animation_frame();
        let colors = &cx.theme().colors;

        v_flex()
            .size_full()
            .bg(colors.background)
            .child(Self::render_header(cx))
            .when(self.positions.is_empty(), |el: gpui::Div| {
                el.child(Self::render_empty(cx))
            })
            .when(!self.positions.is_empty(), |el: gpui::Div| {
                el.child(
                    v_flex()
                        .id("positions-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .children(
                            self.positions
                                .iter()
                                .enumerate()
                                .map(|(i, pos)| self.render_row(pos, i, cx)),
                        ),
                )
                .child(self.render_summary(cx))
            })
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

impl Panel for PositionsPanel {
    fn panel_name(&self) -> &'static str {
        "PositionsPanel"
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
            panel_name: "PositionsPanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for PositionsPanel {}

impl Focusable for PositionsPanel {
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
        assert_eq!(PositionsPanel::fmt_price(50_000.0), "50000.00");
        assert_eq!(PositionsPanel::fmt_price(1.5), "1.5000");
        assert_eq!(PositionsPanel::fmt_price(0.000123), "0.000123");
    }

    #[test]
    fn fmt_pnl_shows_sign() {
        assert_eq!(PositionsPanel::fmt_pnl(100.0), "+100.00");
        assert_eq!(PositionsPanel::fmt_pnl(-50.0), "-50.00");
        assert_eq!(PositionsPanel::fmt_pnl(0.0), "+0.00");
    }

    #[test]
    fn fmt_pct_shows_sign_and_percent() {
        assert_eq!(PositionsPanel::fmt_pct(5.5), "+5.50%");
        assert_eq!(PositionsPanel::fmt_pct(-2.3), "-2.30%");
    }
}
