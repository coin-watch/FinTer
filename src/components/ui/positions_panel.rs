/// Positions panel — real-time position and order history tracker.
///
/// Displays open trading positions with entry price, current price, size,
/// margin, leverage, unrealized P&L, percentage gain/loss, and position close actions.
/// Also displays order execution history with timestamps, notional, and fill status.
/// Synchronized in real time with the Trade Desk via TradeStore.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex, Icon, IconName, Sizable};

use crate::data::feed::TickFeed;
use crate::data::trade_store::{
    DeskPosition, TradeOrder, TradeOrderType, TradeSide, TradeStore, TradingMode,
};

// ---------------------------------------------------------------------------
// Position — a single legacy/custom trading position
// ---------------------------------------------------------------------------

/// A single open trading position for custom or mock feeds.
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PositionsTab {
    Positions,
    History,
}

// ---------------------------------------------------------------------------
// PositionsPanel
// ---------------------------------------------------------------------------

pub struct PositionsPanel {
    /// Optional legacy positions passed at construction.
    positions: Vec<Position>,
    /// Display title.
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    active_tab: PositionsTab,
}

impl PositionsPanel {
    pub fn new(positions: Vec<Position>, title: impl Into<String>, cx: &mut Context<Self>) -> Self {
        let feeds: Vec<_> = positions.iter().map(|position| position.feed.clone()).collect();
        cx.spawn(async move |this, cx| {
            let mut last_feeds: Vec<u64> = feeds.iter().map(TickFeed::sequence).collect();
            let mut last_ver: u64 = 0;
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                let current_feeds: Vec<u64> = feeds.iter().map(TickFeed::sequence).collect();
                let current_ver = TradeStore::global().lock().unwrap().version;
                let changed = current_feeds != last_feeds || current_ver != last_ver;
                last_feeds = current_feeds;
                last_ver = current_ver;
                if this.update(cx, |_, cx| {
                    if changed {
                        cx.notify();
                    }
                }).is_err() {
                    break;
                }
            }
        }).detach();

        Self {
            positions,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 0.0,
            height: 0.0,
            active_tab: PositionsTab::Positions,
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
    pub fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{:.2}", price)
        } else if price >= 1.0 {
            format!("{:.4}", price)
        } else {
            format!("{:.6}", price)
        }
    }

    /// Format P&L for display with sign.
    pub fn fmt_pnl(pnl: f64) -> String {
        if pnl >= 0.0 {
            format!("+{:.2}", pnl)
        } else {
            format!("{:.2}", pnl)
        }
    }

    /// Format percentage for display.
    pub fn fmt_pct(pct: f64) -> String {
        if pct >= 0.0 {
            format!("+{:.2}%", pct)
        } else {
            format!("{:.2}%", pct)
        }
    }

    /// Format size for display.
    pub fn fmt_size(size: f64) -> String {
        if size >= 100.0 {
            format!("{:.2}", size)
        } else {
            format!("{:.4}", size)
        }
    }

    /// Format timestamp into HH:MM:SS.
    pub fn fmt_time(ts: u64) -> String {
        if ts == 0 {
            return "--:--:--".to_string();
        }
        if let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp(ts as i64, 0) {
            dt.format("%H:%M:%S").to_string()
        } else {
            "--:--:--".to_string()
        }
    }

    // ── Tab Navigation Header ───────────────────────────────────────────────

    fn render_tabs_header(
        &self,
        positions_count: usize,
        orders_count: usize,
        total_pnl: f64,
        deployed_margin: f64,
        is_narrow: bool,
        is_very_narrow: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();

        h_flex()
            .w_full()
            .px_3()
            .py_1p5()
            .justify_between()
            .items_center()
            .bg(colors.table_head)
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    // Positions Tab Button
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .px_2()
                            .py_0p5()
                            .cursor_pointer()
                            .rounded(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .bg(if self.active_tab == PositionsTab::Positions {
                                colors.muted
                            } else {
                                colors.background
                            })
                            .text_color(if self.active_tab == PositionsTab::Positions {
                                colors.foreground
                            } else {
                                colors.muted_foreground
                            })
                            .hover(|s| s.bg(colors.muted.opacity(0.6)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.active_tab = PositionsTab::Positions;
                                    cx.notify();
                                }),
                            )
                            .child(if is_very_narrow { "POS" } else { "POSITIONS" })
                            .child(
                                div()
                                    .px_1()
                                    .py(px(0.5))
                                    .rounded(px(8.0))
                                    .bg(if positions_count == 0 {
                                        colors.muted.opacity(0.5)
                                    } else if total_pnl >= 0.0 {
                                        colors.bullish.opacity(0.2)
                                    } else {
                                        colors.bearish.opacity(0.2)
                                    })
                                    .text_color(if positions_count == 0 {
                                        colors.muted_foreground
                                    } else if total_pnl >= 0.0 {
                                        colors.bullish
                                    } else {
                                        colors.bearish
                                    })
                                    .child(positions_count.to_string()),
                            ),
                    )
                    // History Tab Button
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .px_2()
                            .py_0p5()
                            .cursor_pointer()
                            .rounded(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .bg(if self.active_tab == PositionsTab::History {
                                colors.muted
                            } else {
                                colors.background
                            })
                            .text_color(if self.active_tab == PositionsTab::History {
                                colors.foreground
                            } else {
                                colors.muted_foreground
                            })
                            .hover(|s| s.bg(colors.muted.opacity(0.6)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.active_tab = PositionsTab::History;
                                    cx.notify();
                                }),
                            )
                            .child(if is_very_narrow { "HIST" } else { "ORDER HISTORY" })
                            .child(
                                div()
                                    .px_1()
                                    .py(px(0.5))
                                    .rounded(px(8.0))
                                    .bg(colors.muted.opacity(0.5))
                                    .text_color(colors.muted_foreground)
                                    .child(orders_count.to_string()),
                            ),
                    ),
            )
            // Telemetry & Actions on Right
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(self.active_tab == PositionsTab::Positions && positions_count > 0, |this| {
                        this.when(!is_narrow, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .font_family(mono_font.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(format!("MRG: ${:.1}", deployed_margin)),
                            )
                        })
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground)
                                        .child("UNREALIZED:"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .font_family(mono_font.clone())
                                        .text_color(if total_pnl >= 0.0 {
                                            colors.bullish
                                        } else {
                                            colors.bearish
                                        })
                                        .child(Self::fmt_pnl(total_pnl)),
                                ),
                        )
                    })
                    .when(self.active_tab == PositionsTab::History && orders_count > 0, |this| {
                        this.child(
                            Button::new("clear-orders-btn")
                                .xsmall()
                                .ghost()
                                .label(if is_narrow { "Clear" } else { "Clear History" })
                                .on_click(cx.listener(|_, _, _, cx| {
                                    TradeStore::global().lock().unwrap().clear_orders();
                                    cx.notify();
                                })),
                        )
                    }),
            )
    }

    // ── Positions Table Header ──────────────────────────────────────────────

    fn render_positions_header(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .bg(colors.table_head)
            .border_b_1()
            .border_color(colors.border)
            .gap_2()
            .items_center()
            .child(
                div()
                    .w(px(145.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SYMBOL"),
            )
            .child(
                div()
                    .w(px(60.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIDE"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIZE"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("ENTRY"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("MARK"),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("UNREALIZED P&L"),
            )
            .child(
                div()
                    .w(px(90.0))
                    .text_right()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("ACTION"),
            )
    }

    // ── Single DeskPosition Row (Wide Table) ────────────────────────────────

    fn render_desk_position_row(
        &self,
        pos: &DeskPosition,
        mark: f64,
        idx: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
        let pos_id = pos.id.clone();

        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        let is_long = pos.side == TradeSide::Long;
        let (side_str, side_color) = if is_long {
            ("LONG", colors.bullish)
        } else {
            ("SHORT", colors.bearish)
        };

        let pnl = match pos.side {
            TradeSide::Long => (mark - pos.entry_price) * pos.quantity,
            TradeSide::Short => (pos.entry_price - mark) * pos.quantity,
        };
        let roe = if pos.margin > 0.0 {
            (pnl / pos.margin) * 100.0
        } else {
            0.0
        };
        let pnl_color = if pnl >= 0.0 {
            colors.bullish
        } else {
            colors.bearish
        };

        h_flex()
            .id(ElementId::Name(format!("desk-pos-{}", pos.id).into()))
            .w_full()
            .px_3()
            .py_2()
            .bg(row_bg)
            .gap_2()
            .items_center()
            .hover(|style| style.bg(colors.table_hover))
            // Symbol, Mode & Leverage
            .child(
                h_flex()
                    .w(px(145.0))
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .px_1()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(if pos.mode == TradingMode::Live {
                                colors.danger.opacity(0.18)
                            } else {
                                colors.primary.opacity(0.15)
                            })
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if pos.mode == TradingMode::Live {
                                colors.danger
                            } else {
                                colors.primary
                            })
                            .child(if pos.mode == TradingMode::Live { "LIVE" } else { "PAPER" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child(pos.symbol.clone()),
                    )
                    .child(
                        div()
                            .px_1()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(colors.accent.opacity(0.2))
                            .text_xs()
                            .font_family(mono_font.clone())
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.primary)
                            .child(format!("{}x", pos.leverage)),
                    ),
            )
            // Side
            .child(
                div()
                    .w(px(60.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(side_color)
                    .child(side_str),
            )
            // Size & Margin
            .child(
                v_flex()
                    .w(px(80.0))
                    .child(
                        div()
                            .text_xs()
                            .font_family(mono_font.clone())
                            .text_color(colors.foreground)
                            .child(Self::fmt_size(pos.quantity)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(mono_font.clone())
                            .text_color(colors.muted_foreground)
                            .child(format!("${:.0}", pos.margin)),
                    ),
            )
            // Entry Price
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.muted_foreground)
                    .child(Self::fmt_price(pos.entry_price)),
            )
            // Mark Price
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.foreground)
                    .child(Self::fmt_price(mark)),
            )
            // Unrealized P&L
            .child(
                h_flex()
                    .flex_1()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .font_family(mono_font.clone())
                            .text_color(pnl_color)
                            .child(Self::fmt_pnl(pnl)),
                    )
                    .child(
                        div()
                            .px_1()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(pnl_color.opacity(0.18))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .font_family(mono_font.clone())
                            .text_color(pnl_color)
                            .child(format!("{:+.1}%", roe)),
                    ),
            )
            // Action Button
            .child(
                div()
                    .w(px(90.0))
                    .flex()
                    .justify_end()
                    .child(
                        Button::new(ElementId::Name(format!("btn-close-{}", pos.id).into()))
                            .xsmall()
                            .outline()
                            .label("Close")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = TradeStore::global().lock().unwrap().close_position(&pos_id, None);
                                cx.notify();
                            })),
                    ),
            )
    }

    // ── Single DeskPosition Card (Narrow Mode) ──────────────────────────────

    fn render_desk_position_card(
        &self,
        pos: &DeskPosition,
        mark: f64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
        let pos_id = pos.id.clone();

        let is_long = pos.side == TradeSide::Long;
        let pnl = match pos.side {
            TradeSide::Long => (mark - pos.entry_price) * pos.quantity,
            TradeSide::Short => (pos.entry_price - mark) * pos.quantity,
        };
        let roe = if pos.margin > 0.0 {
            (pnl / pos.margin) * 100.0
        } else {
            0.0
        };
        let is_green = pnl >= 0.0;
        let pnl_color = if is_green {
            colors.bullish
        } else {
            colors.bearish
        };

        v_flex()
            .w_full()
            .p_2p5()
            .rounded(px(5.0))
            .bg(colors.muted.opacity(0.18))
            .border_1()
            .border_color(if is_green {
                colors.bullish.opacity(0.3)
            } else {
                colors.bearish.opacity(0.3)
            })
            .gap_1p5()
            // Top Row: Side badge, Symbol, Leverage, Close Button
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
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(if pos.mode == TradingMode::Live {
                                        colors.danger.opacity(0.18)
                                    } else {
                                        colors.primary.opacity(0.15)
                                    })
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if pos.mode == TradingMode::Live {
                                        colors.danger
                                    } else {
                                        colors.primary
                                    })
                                    .child(if pos.mode == TradingMode::Live { "LIVE" } else { "PAPER" }),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(if is_long {
                                        colors.bullish.opacity(0.2)
                                    } else {
                                        colors.bearish.opacity(0.2)
                                    })
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if is_long {
                                        colors.bullish
                                    } else {
                                        colors.bearish
                                    })
                                    .child(if is_long { "LONG" } else { "SHORT" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.foreground)
                                    .child(pos.symbol.clone()),
                            )
                            .child(
                                div()
                                    .px_1()
                                    .py(px(0.5))
                                    .rounded(px(2.0))
                                    .bg(colors.accent.opacity(0.2))
                                    .text_xs()
                                    .font_family(mono_font.clone())
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.primary)
                                    .child(format!("{}x", pos.leverage)),
                            ),
                    )
                    .child(
                        Button::new(ElementId::Name(format!("btn-card-close-{}", pos.id).into()))
                            .xsmall()
                            .outline()
                            .label("Close")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = TradeStore::global().lock().unwrap().close_position(&pos_id, None);
                                cx.notify();
                            })),
                    ),
            )
            // Mid Row: Entry, Mark, Size, Margin
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_xs()
                    .child(
                        div()
                            .font_family(mono_font.clone())
                            .text_color(colors.muted_foreground)
                            .child(format!(
                                "Entry: ${} · Mark: ${}",
                                Self::fmt_price(pos.entry_price),
                                Self::fmt_price(mark)
                            )),
                    )
                    .child(
                        div()
                            .font_family(mono_font.clone())
                            .text_color(colors.muted_foreground)
                            .child(format!("{} (Mrg ${:.0})", Self::fmt_size(pos.quantity), pos.margin)),
                    ),
            )
            // Bottom Row: Unrealized PnL & ROE
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.muted_foreground)
                            .child("UNREALIZED:"),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font.clone())
                                    .text_color(pnl_color)
                                    .child(Self::fmt_pnl(pnl)),
                            )
                            .child(
                                div()
                                    .px_1()
                                    .py(px(0.5))
                                    .rounded(px(2.0))
                                    .bg(pnl_color.opacity(0.18))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .font_family(mono_font.clone())
                                    .text_color(pnl_color)
                                    .child(format!("{:+.1}% ROE", roe)),
                            ),
                    ),
            )
    }

    // ── Legacy Position Row (Wide Table) ────────────────────────────────────

    fn render_legacy_position_row(
        &self,
        position: &Position,
        idx: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
        let current_price = position.current_price();

        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        let (side_str, side_color) = match position.side {
            PositionSide::Long => ("LONG", colors.bullish),
            PositionSide::Short => ("SHORT", colors.bearish),
        };

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
            .items_center()
            .hover(|style| style.bg(colors.table_hover))
            .child(
                div()
                    .w(px(145.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child(position.symbol.clone()),
            )
            .child(
                div()
                    .w(px(60.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(side_color)
                    .child(side_str),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.foreground)
                    .child(Self::fmt_size(position.size)),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.muted_foreground)
                    .child(Self::fmt_price(position.entry_price)),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.foreground)
                    .child(current_str),
            )
            .child(
                h_flex()
                    .flex_1()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .font_family(mono_font.clone())
                            .text_color(pnl_color)
                            .child(pnl_str),
                    )
                    .child(
                        div()
                            .px_1()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(pnl_color.opacity(0.18))
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .font_family(mono_font)
                            .text_color(pnl_color)
                            .child(pct_str),
                    ),
            )
            .child(
                div()
                    .w(px(90.0))
                    .text_right()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("Live Feed"),
            )
    }

    // ── Order History Table Header ──────────────────────────────────────────

    fn render_history_header(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .bg(colors.table_head)
            .border_b_1()
            .border_color(colors.border)
            .gap_2()
            .items_center()
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("TIME"),
            )
            .child(
                div()
                    .w(px(55.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SIDE"),
            )
            .child(
                div()
                    .w(px(145.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("SYMBOL"),
            )
            .child(
                div()
                    .w(px(65.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("TYPE"),
            )
            .child(
                div()
                    .w(px(85.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("PRICE"),
            )
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("QTY"),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("NOTIONAL"),
            )
            .child(
                div()
                    .w(px(75.0))
                    .text_right()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child("STATUS"),
            )
    }

    // ── Single Order Row (Wide Table) ───────────────────────────────────────

    fn render_order_row(&self, ord: &TradeOrder, idx: usize, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();

        let row_bg = if idx.is_multiple_of(2) {
            colors.table_even
        } else {
            colors.table
        };

        let is_buy = ord.side == TradeSide::Long;
        let (side_str, side_color) = if is_buy {
            ("BUY", colors.bullish)
        } else {
            ("SELL", colors.bearish)
        };

        let type_str = match ord.order_type {
            TradeOrderType::Market => "MARKET",
            TradeOrderType::Limit => "LIMIT",
        };

        let is_filled = ord.status == "FILLED";

        h_flex()
            .id(ElementId::Name(format!("ord-row-{}", ord.id).into()))
            .w_full()
            .px_3()
            .py_2()
            .bg(row_bg)
            .gap_2()
            .items_center()
            .hover(|style| style.bg(colors.table_hover))
            // Time
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.muted_foreground)
                    .child(Self::fmt_time(ord.timestamp)),
            )
            // Side
            .child(
                div()
                    .w(px(55.0))
                    .px_1p5()
                    .py(px(0.5))
                    .rounded(px(2.0))
                    .bg(if is_buy {
                        colors.bullish.opacity(0.18)
                    } else {
                        colors.bearish.opacity(0.18)
                    })
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(side_color)
                    .child(side_str),
            )
            // Symbol & Mode
            .child(
                h_flex()
                    .w(px(145.0))
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .px_1()
                            .py(px(0.5))
                            .rounded(px(2.0))
                            .bg(if ord.mode == TradingMode::Live {
                                colors.danger.opacity(0.18)
                            } else {
                                colors.primary.opacity(0.15)
                            })
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(if ord.mode == TradingMode::Live {
                                colors.danger
                            } else {
                                colors.primary
                            })
                            .child(if ord.mode == TradingMode::Live { "LIVE" } else { "PAPER" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child(ord.symbol.clone()),
                    ),
            )
            // Type
            .child(
                div()
                    .w(px(65.0))
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(type_str),
            )
            // Price
            .child(
                div()
                    .w(px(85.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.foreground)
                    .child(format!("${:.2}", ord.price)),
            )
            // Qty
            .child(
                div()
                    .w(px(80.0))
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.muted_foreground)
                    .child(Self::fmt_size(ord.quantity)),
            )
            // Notional
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(colors.foreground)
                    .child(format!("${:.2}", ord.notional)),
            )
            // Status / Action
            .child(
                div()
                    .w(px(75.0))
                    .flex()
                    .justify_end()
                    .when(ord.status == "OPEN", |s| {
                        let ord_id = ord.id.clone();
                        s.child(
                            Button::new(ElementId::Name(format!("btn-cancel-ord-{}", ord.id).into()))
                                .xsmall()
                                .outline()
                                .label("Cancel")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    let _ = TradeStore::global().lock().unwrap().cancel_order(&ord_id);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(ord.status != "OPEN", |s| {
                        s.child(
                            div()
                                .px_1p5()
                                .py(px(0.5))
                                .rounded(px(2.0))
                                .bg(if is_filled {
                                    colors.accent.opacity(0.2)
                                } else {
                                    colors.muted.opacity(0.3)
                                })
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(if is_filled {
                                    colors.primary
                                } else {
                                    colors.muted_foreground
                                })
                                .child(ord.status),
                        )
                    }),
            )
    }

    // ── Single Order Card (Narrow Mode) ─────────────────────────────────────

    fn render_order_card(&self, ord: &TradeOrder, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();

        let is_buy = ord.side == TradeSide::Long;
        let is_filled = ord.status == "FILLED";

        v_flex()
            .w_full()
            .p_2p5()
            .rounded(px(5.0))
            .bg(colors.muted.opacity(0.15))
            .border_1()
            .border_color(colors.border.opacity(0.4))
            .gap_1p5()
            // Top Row: Side, Symbol, Type, Status
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
                                    .px_1p5()
                                    .py(px(0.5))
                                    .rounded(px(2.0))
                                    .bg(if ord.mode == TradingMode::Live {
                                        colors.danger.opacity(0.18)
                                    } else {
                                        colors.primary.opacity(0.15)
                                    })
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if ord.mode == TradingMode::Live {
                                        colors.danger
                                    } else {
                                        colors.primary
                                    })
                                    .child(if ord.mode == TradingMode::Live { "LIVE" } else { "PAPER" }),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(0.5))
                                    .rounded(px(2.0))
                                    .bg(if is_buy {
                                        colors.bullish.opacity(0.2)
                                    } else {
                                        colors.bearish.opacity(0.2)
                                    })
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if is_buy {
                                        colors.bullish
                                    } else {
                                        colors.bearish
                                    })
                                    .child(if is_buy { "BUY" } else { "SELL" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.foreground)
                                    .child(ord.symbol.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(match ord.order_type {
                                        TradeOrderType::Market => "MKT",
                                        TradeOrderType::Limit => "LMT",
                                    }),
                            ),
                    )
                    .when(ord.status == "OPEN", |s| {
                        let ord_id = ord.id.clone();
                        s.child(
                            Button::new(ElementId::Name(format!("btn-card-cancel-ord-{}", ord.id).into()))
                                .xsmall()
                                .outline()
                                .label("Cancel")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    let _ = TradeStore::global().lock().unwrap().cancel_order(&ord_id);
                                    cx.notify();
                                })),
                        )
                    })
                    .when(ord.status != "OPEN", |s| {
                        s.child(
                            div()
                                .px_1p5()
                                .py(px(0.5))
                                .rounded(px(2.0))
                                .bg(if is_filled {
                                    colors.accent.opacity(0.2)
                                } else {
                                    colors.muted.opacity(0.3)
                                })
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(if is_filled {
                                    colors.primary
                                } else {
                                    colors.muted_foreground
                                })
                                .child(ord.status),
                        )
                    }),
            )
            // Bottom Row: Price, Qty, Notional, Time
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .text_xs()
                    .child(
                        div()
                            .font_family(mono_font.clone())
                            .text_color(colors.muted_foreground)
                            .child(format!("@ ${:.2} · Qty {}", ord.price, Self::fmt_size(ord.quantity))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_family(mono_font.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.foreground)
                                    .child(format!("${:.2}", ord.notional)),
                            )
                            .child(
                                div()
                                    .font_family(mono_font)
                                    .text_color(colors.muted_foreground.opacity(0.7))
                                    .child(Self::fmt_time(ord.timestamp)),
                            ),
                    ),
            )
    }

    // ── Empty State ─────────────────────────────────────────────────────────

    fn render_positions_empty(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        v_flex()
            .size_full()
            .py_12()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::SquareTerminal)
                    .size(px(32.))
                    .text_color(colors.muted_foreground.opacity(0.6)),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child("NO OPEN POSITIONS"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("Execute an order ticket on the Trade Desk to monitor real-time positions and live PnL."),
            )
    }

    fn render_history_empty(cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        v_flex()
            .size_full()
            .py_12()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                Icon::new(IconName::Calendar)
                    .size(px(32.))
                    .text_color(colors.muted_foreground.opacity(0.6)),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.foreground)
                    .child("NO ORDER HISTORY"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child("Orders executed on the Trade Desk will be logged here."),
            )
    }

    // ── Summary Footer ──────────────────────────────────────────────────────

    fn render_summary(
        &self,
        positions_count: usize,
        deployed_margin: f64,
        total_pnl: f64,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let mono_font = cx.theme().mono_font_family.clone();
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
            .items_center()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.muted_foreground)
                            .child(format!("TOTAL ({} positions)", positions_count)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(mono_font.clone())
                            .text_color(colors.muted_foreground)
                            .child(format!("MRG: ${:.1}", deployed_margin)),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .font_family(mono_font)
                    .text_color(pnl_color)
                    .child(Self::fmt_pnl(total_pnl)),
            )
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for PositionsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let is_narrow = self.width > 0.0 && self.width < 500.0;
        let is_very_narrow = self.width > 0.0 && self.width < 340.0;

        let (store_positions, store_orders, total_pnl, deployed_margin) = {
            let store = TradeStore::global().lock().unwrap();
            // Pair each position with the exact mark price used for its PnL so the
            // rows and the header aggregates can never disagree.
            let pos: Vec<(DeskPosition, f64)> = store
                .positions
                .iter()
                .map(|p| (p.clone(), store.position_mark_price(p)))
                .collect();
            let ord = store.orders.clone();
            let upnl: f64 = pos
                .iter()
                .map(|(p, _)| store.position_unrealized_pnl(p))
                .sum();
            let dep = store.deployed_margin_all();
            (pos, ord, upnl, dep)
        };

        let total_positions_count = store_positions.len() + self.positions.len();
        let total_orders_count = store_orders.len();

        v_flex()
            .size_full()
            .bg(colors.background)
            // Top Navigation & Telemetry
            .child(self.render_tabs_header(
                total_positions_count,
                total_orders_count,
                total_pnl,
                deployed_margin,
                is_narrow,
                is_very_narrow,
                cx,
            ))
            // Content Body
            .child(match self.active_tab {
                PositionsTab::Positions => {
                    if total_positions_count == 0 {
                        div().size_full().child(Self::render_positions_empty(cx)).into_any_element()
                    } else if is_narrow {
                        // Narrow Card List
                        let pos_elements: Vec<_> = store_positions
                            .iter()
                            .map(|(pos, mark)| {
                                self.render_desk_position_card(pos, *mark, cx).into_any_element()
                            })
                            .collect();

                        v_flex()
                            .id("positions-narrow-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_2()
                            .gap_2()
                            .children(pos_elements)
                            .into_any_element()
                    } else {
                        // Wide Table
                        let desk_rows: Vec<_> = store_positions
                            .iter()
                            .enumerate()
                            .map(|(i, (pos, mark))| {
                                self.render_desk_position_row(pos, *mark, i, cx).into_any_element()
                            })
                            .collect();

                        let legacy_rows: Vec<_> = self
                            .positions
                            .iter()
                            .enumerate()
                            .map(|(i, pos)| {
                                self.render_legacy_position_row(pos, store_positions.len() + i, cx)
                                    .into_any_element()
                            })
                            .collect();

                        v_flex()
                            .size_full()
                            .child(Self::render_positions_header(cx))
                            .child(
                                v_flex()
                                    .id("positions-wide-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .children(desk_rows)
                                    .children(legacy_rows),
                            )
                            .into_any_element()
                    }
                }
                PositionsTab::History => {
                    if total_orders_count == 0 {
                        div().size_full().child(Self::render_history_empty(cx)).into_any_element()
                    } else if is_narrow {
                        // Narrow Order Cards
                        let order_elements: Vec<_> = store_orders
                            .iter()
                            .map(|ord| self.render_order_card(ord, cx).into_any_element())
                            .collect();

                        v_flex()
                            .id("history-narrow-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_2()
                            .gap_2()
                            .children(order_elements)
                            .into_any_element()
                    } else {
                        // Wide Table
                        let order_rows: Vec<_> = store_orders
                            .iter()
                            .enumerate()
                            .map(|(i, ord)| self.render_order_row(ord, i, cx).into_any_element())
                            .collect();

                        v_flex()
                            .size_full()
                            .child(Self::render_history_header(cx))
                            .child(
                                v_flex()
                                    .id("history-wide-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .children(order_rows),
                            )
                            .into_any_element()
                    }
                }
            })
            // Footer Summary (when on Positions tab and positions exist)
            .when(self.active_tab == PositionsTab::Positions && total_positions_count > 0, |this| {
                this.child(self.render_summary(total_positions_count, deployed_margin, total_pnl, cx))
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
