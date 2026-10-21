/// Tape panel — real-time Time & Sales and Executed Trades display.
///
/// Renders actual executed trades from a `TickFeed` as a high-density,
/// responsive real-time tape with volume telemetry, delta pressure,
/// trade filtering (All, Buys, Sells, Whales), and pause/resume buffering.
///
/// # 60 FPS design
/// `render()` takes a snapshot of the current `TickFeed` ring buffer
/// and formats visible rows. No allocations occur beyond filtering.
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, relative,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};

use crate::data::feed::TickFeed;
use crate::data::tick::UnifiedTick;
use crate::data::terminal_audio::{AudioStatus, TerminalAudio, Tone};

// ---------------------------------------------------------------------------
// Display constants
// ---------------------------------------------------------------------------

const MAX_VISIBLE_ROWS: usize = 120;
const WHALE_THRESHOLD_USD: f64 = 10_000.0;
const MEGA_THRESHOLD_USD: f64 = 50_000.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TradeFilter {
    All,
    Buys,
    Sells,
    Whales,
}

// ---------------------------------------------------------------------------
// TapePanel
// ---------------------------------------------------------------------------

pub struct TapePanel {
    feed: TickFeed,
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    is_paused: bool,
    filter: TradeFilter,
    paused_snapshot: Option<Vec<Arc<UnifiedTick>>>,
    sound_enabled: bool,
    audio_cursor: Option<u64>,
    global_sound_enabled: bool,
}

impl TapePanel {
    pub fn new(feed: TickFeed, title: impl Into<String>, cx: &mut App) -> Self {
        Self {
            feed,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 0.0,
            height: 0.0,
            is_paused: false,
            filter: TradeFilter::All,
            paused_snapshot: None,
            sound_enabled: false,
            audio_cursor: None,
            global_sound_enabled: true,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
    }

    pub fn toggle_pause(&mut self) {
        self.is_paused = !self.is_paused;
        if self.is_paused {
            self.paused_snapshot = Some(self.feed.snapshot());
            self.audio_cursor = Some(self.feed.sequence());
        } else {
            self.paused_snapshot = None;
            // Skip everything accumulated while paused.
            self.audio_cursor = Some(self.feed.sequence());
        }
    }

    pub fn set_filter(&mut self, filter: TradeFilter) {
        self.filter = filter;
    }

    pub fn set_global_sound_enabled(&mut self, enabled: bool) {
        self.global_sound_enabled = enabled;
        if !enabled { self.sound_enabled = false; }
    }

    pub fn sound_enabled(&self) -> bool { self.sound_enabled }

    pub fn toggle_sound(&mut self) {
        if !self.global_sound_enabled { return; }
        self.sound_enabled = !self.sound_enabled;
        if self.sound_enabled {
            // Establish the cursor at the current tail before previewing. This
            // prevents enabling/resizing from replaying existing trades.
            self.audio_cursor = Some(self.feed.sequence());
            TerminalAudio::shared().play(Tone::Preview);
        }
    }

    fn process_new_audio(&mut self) {
        if !self.sound_enabled || !self.global_sound_enabled { return; }
        let (sequence, ticks) = self.feed.sequenced_snapshot();
        let cursor = self.audio_cursor.get_or_insert(sequence);
        if sequence <= *cursor { return; }
        let arrivals = sequence.saturating_sub(*cursor) as usize;
        let start = ticks.len().saturating_sub(arrivals);
        let tone = classify_audio_batch(&ticks[start..]);
        *cursor = sequence;
        if let Some(tone) = tone { TerminalAudio::shared().play(tone); }
    }

    /// Format a price for display. Precision adapts to magnitude.
    pub fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{price:.2}")
        } else if price >= 1.0 {
            format!("{price:.4}")
        } else {
            format!("{price:.6}")
        }
    }

    /// Format an amount for display.
    pub fn fmt_amount(amount: f64) -> String {
        if amount >= 100.0 {
            format!("{amount:.2}")
        } else if amount >= 1.0 {
            format!("{amount:.3}")
        } else {
            format!("{amount:.4}")
        }
    }

    /// Format USD value with suffix.
    pub fn fmt_usd(val: f64) -> String {
        if val >= 1_000_000.0 {
            format!("${:.2}M", val / 1_000_000.0)
        } else if val >= 1_000.0 {
            format!("${:.1}K", val / 1_000.0)
        } else {
            format!("${val:.2}")
        }
    }
}

/// One cue per new arrival batch: a mega threshold execution outranks all
/// normal buy/sell ticks. Input order preserves equal-timestamp executions.
fn classify_audio_batch(ticks: &[Arc<UnifiedTick>]) -> Option<Tone> {
    if ticks.iter().any(|tick| tick.notional() >= MEGA_THRESHOLD_USD) { return Some(Tone::Whale); }
    ticks.first().map(|tick| if tick.side.is_buy() { Tone::Buy } else { Tone::Sell })
}

impl Render for TapePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_paused {
            self.process_new_audio();
            window.request_animation_frame();
        }

        let theme = cx.theme();
        let colors = &theme.colors;

        let is_compact = self.width < 340.0;
        let is_narrow = self.width < 270.0;

        // Retrieve ticks
        let mut raw_ticks = if self.is_paused {
            self.paused_snapshot.clone().unwrap_or_default()
        } else {
            self.feed.snapshot()
        };

        // Compute telemetry across available recent ticks
        let mut buy_vol = 0.0;
        let mut sell_vol = 0.0;
        let total_trades = raw_ticks.len();

        for t in &raw_ticks {
            let val = t.notional();
            if t.side.is_buy() {
                buy_vol += val;
            } else {
                sell_vol += val;
            }
        }

        let total_vol = (buy_vol + sell_vol).max(1e-6);
        let buy_ratio = (buy_vol / total_vol) * 100.0;
        let _sell_ratio = 100.0 - buy_ratio;
        let delta = buy_vol - sell_vol;

        // Newest ticks first
        raw_ticks.reverse();

        // Filter ticks
        let filtered_ticks: Vec<Arc<UnifiedTick>> = raw_ticks
            .into_iter()
            .filter(|t| match self.filter {
                TradeFilter::All => true,
                TradeFilter::Buys => t.side.is_buy(),
                TradeFilter::Sells => !t.side.is_buy(),
                TradeFilter::Whales => t.notional() >= WHALE_THRESHOLD_USD,
            })
            .take(MAX_VISIBLE_ROWS)
            .collect();

        v_flex()
            .size_full()
            .bg(colors.background)
            .overflow_hidden()
            // Top Controls & Telemetry Header
            .child(
                v_flex()
                    .w_full()
                    .bg(colors.table_head)
                    .border_b_1()
                    .border_color(colors.border)
                    // Row 1: Status, Filter Tabs, Pause Button
                    .child(
                        h_flex()
                            .w_full()
                            .px_2()
                            .py_1()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    // Status pill
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_1()
                                            .px_1p5()
                                            .py(px(2.0))
                                            .rounded(px(3.0))
                                            .bg(if self.is_paused {
                                                colors.warning.opacity(0.15)
                                            } else {
                                                colors.bullish.opacity(0.15)
                                            })
                                            .border_1()
                                            .border_color(if self.is_paused {
                                                colors.warning.opacity(0.3)
                                            } else {
                                                colors.bullish.opacity(0.3)
                                            })
                                            .child(
                                                div()
                                                    .size(px(6.0))
                                                    .rounded_full()
                                                    .bg(if self.is_paused {
                                                        colors.warning
                                                    } else {
                                                        colors.bullish
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(if self.is_paused {
                                                        colors.warning
                                                    } else {
                                                        colors.bullish
                                                    })
                                                    .child(if self.is_paused { "PAUSED" } else { "LIVE" }),
                                            ),
                                    )
                                    // Filter tabs
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap(px(2.0))
                                            .child(self.render_filter_btn(TradeFilter::All, "ALL", cx))
                                            .child(self.render_filter_btn(TradeFilter::Buys, "BUYS", cx))
                                            .child(self.render_filter_btn(TradeFilter::Sells, "SELLS", cx))
                                            .when(!is_narrow, |h| {
                                                h.child(self.render_filter_btn(TradeFilter::Whales, "WHALES", cx))
                                            }),
                                    ),
                            )
                            .child(
                                h_flex().gap_1().items_center()
                                    .child(self.render_sound_btn(cx))
                                    // Pause / Resume Toggle Button
                                    .child(
                                div()
                                    .id("tape-pause-btn")
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded(px(3.0))
                                    .bg(colors.muted.opacity(0.4))
                                    .border_1()
                                    .border_color(colors.border)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(colors.table_hover))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _win, cx| {
                                            this.toggle_pause();
                                            cx.notify();
                                        }),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.foreground)
                                            .child(if self.is_paused { "▶" } else { "⏸" }),
                                    ),
                                    ),
                            ),
                    )
                    // Row 2: Volume & Delta Telemetry Bar
                    .when(!is_compact, |v| {
                        v.child(
                            h_flex()
                                .w_full()
                                .px_2()
                                .pb_1()
                                .justify_between()
                                .items_center()
                                .text_size(px(10.0))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(colors.bullish)
                                                .child(format!("Buy: {}", Self::fmt_usd(buy_vol))),
                                        )
                                        .child(
                                            div()
                                                .w(px(70.0))
                                                .h(px(4.0))
                                                .rounded(px(2.0))
                                                .overflow_hidden()
                                                .bg(colors.bearish.opacity(0.85))
                                                .child(
                                                    div()
                                                        .h_full()
                                                        .w(relative(buy_ratio as f32 / 100.0))
                                                        .bg(colors.bullish),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(colors.bearish)
                                                .child(format!("Sell: {}", Self::fmt_usd(sell_vol))),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_color(colors.muted_foreground)
                                                .child(format!("{total_trades} trades")),
                                        )
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(if delta >= 0.0 {
                                                    colors.bullish
                                                } else {
                                                    colors.bearish
                                                })
                                                .child(format!("Δ {}{}", if delta >= 0.0 { "+" } else { "" }, Self::fmt_usd(delta))),
                                        ),
                                ),
                        )
                    }),
            )
            // Table Column Headers
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py(px(3.0))
                    .bg(colors.table_head)
                    .border_b_1()
                    .border_color(colors.border)
                    .gap_2()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .child(div().w(px(55.0)).child("TIME"))
                    .when(!is_narrow, |row| row.child(div().w(px(36.0)).child("SIDE")))
                    .child(div().flex_1().child("PRICE"))
                    .when(!is_compact, |row| row.child(div().w(px(65.0)).child("AMOUNT")))
                    .child(div().w(px(if is_narrow { 60.0 } else { 75.0 })).text_right().child("VALUE"))
                    .when(!is_compact, |row| row.child(div().w(px(60.0)).text_right().child("BADGE"))),
            )
            // Scrollable Rows or Empty State
            .when(filtered_ticks.is_empty(), |el| {
                el.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.muted_foreground)
                                .child("Waiting for trades…"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.muted_foreground.opacity(0.6))
                                .child("Streaming live executed trades from exchange"),
                        ),
                )
            })
            .when(!filtered_ticks.is_empty(), |el| {
                el.child(
                    v_flex()
                        .id("tape-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .children(
                            filtered_ticks
                                .into_iter()
                                .enumerate()
                                .map(|(idx, tick)| self.render_trade_row(&tick, idx, is_compact, is_narrow, cx)),
                        ),
                )
            })
    }
}

impl TapePanel {
    fn render_sound_btn(&self, cx: &Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let is_device_unavailable = TerminalAudio::current_status() == AudioStatus::Unavailable;

        let (icon, tooltip_text, text_color, bg_color, border_color) = if !self.global_sound_enabled {
            (
                "icons/volume-x.svg",
                "Sound alerts are disabled in global settings",
                colors.muted_foreground.opacity(0.6),
                colors.muted.opacity(0.15),
                colors.border.opacity(0.4),
            )
        } else if is_device_unavailable && self.sound_enabled {
            (
                "icons/volume-x.svg",
                "No audio device detected on this system",
                colors.warning,
                colors.warning.opacity(0.15),
                colors.warning.opacity(0.35),
            )
        } else if self.sound_enabled {
            (
                "icons/volume-2.svg",
                "Mute trade ticks",
                colors.bullish,
                colors.bullish.opacity(0.15),
                colors.bullish.opacity(0.4),
            )
        } else {
            (
                "icons/volume-x.svg",
                "Enable trade ticks (buy/sell tones & whale chime)",
                colors.muted_foreground,
                colors.muted.opacity(0.25),
                colors.border.opacity(0.5),
            )
        };

        div()
            .id("tape-sound-toggle")
            .tooltip(move |win, cx| gpui_component::tooltip::Tooltip::new(tooltip_text).build(win, cx))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .size(px(22.0))
            .rounded(px(3.0))
            .bg(bg_color)
            .border_1()
            .border_color(border_color)
            .cursor_pointer()
            .hover(|s| s.bg(colors.table_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _win, cx| {
                    this.toggle_sound();
                    cx.notify();
                }),
            )
            .child(
                gpui::svg()
                    .path(icon)
                    .size(px(13.0))
                    .text_color(text_color),
            )
    }

    fn render_filter_btn(&self, target: TradeFilter, label: &'static str, cx: &Context<Self>) -> impl IntoElement {
        let is_selected = self.filter == target;
        let colors = &cx.theme().colors;

        div()
            .id(ElementId::Name(format!("filter-btn-{label}").into()))
            .px_1p5()
            .py(px(1.0))
            .rounded(px(3.0))
            .cursor_pointer()
            .bg(if is_selected {
                colors.primary
            } else {
                gpui::transparent_black()
            })
            .hover(|s| {
                if !is_selected {
                    s.bg(colors.muted.opacity(0.4))
                } else {
                    s
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _win, cx| {
                    this.set_filter(target);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .text_size(px(9.5))
                    .font_weight(if is_selected {
                        FontWeight::BOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if is_selected {
                        colors.primary_foreground
                    } else {
                        colors.muted_foreground
                    })
                    .child(label),
            )
    }

    fn render_trade_row(
        &self,
        tick: &Arc<UnifiedTick>,
        idx: usize,
        is_compact: bool,
        is_narrow: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let is_buy = tick.side.is_buy();
        let colors = &cx.theme().colors;

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

        let notional = tick.notional();
        let notional_str = Self::fmt_usd(notional);

        let is_whale = notional >= WHALE_THRESHOLD_USD;
        let is_mega = notional >= MEGA_THRESHOLD_USD;

        // Timestamp: HH:MM:SS
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
            .items_center()
            .hover(|s| s.bg(colors.table_hover))
            .when(is_whale, |r| {
                r.border_l_2().border_color(if is_mega {
                    colors.warning
                } else {
                    side_color
                })
            })
            // Time
            .child(
                div()
                    .w(px(55.0))
                    .text_size(px(10.5))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(colors.muted_foreground)
                    .child(time_str),
            )
            // Side
            .when(!is_narrow, |r| {
                r.child(
                    div()
                        .w(px(36.0))
                        .text_size(px(10.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(side_color)
                        .child(side_str),
                )
            })
            // Price
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(side_color)
                    .child(price_str),
            )
            // Amount
            .when(!is_compact, |r| {
                r.child(
                    div()
                        .w(px(65.0))
                        .text_size(px(10.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.foreground)
                        .child(amount_str),
                )
            })
            // Notional Value
            .child(
                div()
                    .w(px(if is_narrow { 60.0 } else { 75.0 }))
                    .text_size(px(10.5))
                    .font_weight(if is_whale {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if is_whale {
                        colors.foreground
                    } else {
                        colors.muted_foreground
                    })
                    .text_right()
                    .child(notional_str),
            )
            // Whale Badge
            .when(!is_compact, |r| {
                r.child(
                    div()
                        .w(px(60.0))
                        .text_right()
                        .child(if is_mega {
                            div()
                                .px_1()
                                .py(px(0.5))
                                .rounded(px(2.0))
                                .bg(colors.warning.opacity(0.18))
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.warning)
                                .child("⚡ MEGA")
                        } else if is_whale {
                            div()
                                .px_1()
                                .py(px(0.5))
                                .rounded(px(2.0))
                                .bg(colors.muted.opacity(0.4))
                                .text_size(px(9.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.foreground)
                                .child("WHALE")
                        } else {
                            div()
                        }),
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
        let buy = TradeSide::Buy;
        assert!(buy.is_buy());
        let sell = TradeSide::Sell;
        assert!(!sell.is_buy());
    }

    #[test]
    fn audio_classification_whale_priority_and_side() {
        // Regular small buy
        let regular_buy = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 60_000.0, 0.1, TradeSide::Buy, 1_700_000_000_000));
        assert_eq!(classify_audio_batch(&[Arc::clone(&regular_buy)]), Some(Tone::Buy));

        // Regular small sell
        let regular_sell = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 60_000.0, 0.1, TradeSide::Sell, 1_700_000_000_000));
        assert_eq!(classify_audio_batch(&[Arc::clone(&regular_sell)]), Some(Tone::Sell));

        // Whale trade ($60,000 notional >= $50,000 MEGA_THRESHOLD_USD)
        let whale_trade = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 60_000.0, 1.0, TradeSide::Buy, 1_700_000_000_000));
        assert_eq!(classify_audio_batch(&[Arc::clone(&whale_trade)]), Some(Tone::Whale));

        // Mixed batch where regular trade is first, but a whale trade exists in the batch: Whale has priority!
        assert_eq!(classify_audio_batch(&[Arc::clone(&regular_sell), Arc::clone(&whale_trade)]), Some(Tone::Whale));

        // Empty batch
        assert_eq!(classify_audio_batch(&[]), None);
    }

    #[tokio::test]
    async fn equal_timestamp_trades_sequenced_and_not_replayed() {
        let (tx, rx) = tokio::sync::broadcast::channel(64);
        let feed = TickFeed::new(rx);

        // Send two distinct trades at the exact same millisecond timestamp
        let t1 = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 60_000.0, 0.1, TradeSide::Buy, 1_700_000_000_000));
        let t2 = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 60_005.0, 0.2, TradeSide::Sell, 1_700_000_000_000));
        tx.send(t1).unwrap();
        tx.send(t2).unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        let (seq, ticks) = feed.sequenced_snapshot();
        assert_eq!(seq, 2);
        assert_eq!(ticks.len(), 2);

        // When initializing sound on a panel, audio_cursor is set to current feed sequence
        let mut cursor = seq;
        let arrivals = seq.saturating_sub(cursor) as usize;
        assert_eq!(arrivals, 0, "No new arrivals to play for historical buffer trades");

        // Next trade arrives
        let t3 = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 60_010.0, 0.5, TradeSide::Buy, 1_700_000_000_001));
        tx.send(t3).unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        let (seq2, ticks2) = feed.sequenced_snapshot();
        assert_eq!(seq2, 3);
        let arrivals2 = seq2.saturating_sub(cursor) as usize;
        assert_eq!(arrivals2, 1, "Exactly 1 new trade detected");

        let start = ticks2.len().saturating_sub(arrivals2);
        let batch = &ticks2[start..];
        assert_eq!(batch.len(), 1);
        assert_eq!(classify_audio_batch(batch), Some(Tone::Buy));
        cursor = seq2;

        // Submitting same frame again without new arrivals plays nothing
        let arrivals3 = seq2.saturating_sub(cursor) as usize;
        assert_eq!(arrivals3, 0);
    }
}
