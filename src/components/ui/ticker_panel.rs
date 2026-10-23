use std::sync::Arc;

use gpui::{
    div, px, Context, IntoElement, ParentElement, Render, Styled, Window,
    prelude::FluentBuilder,
};
use gpui_component::{
    h_flex,
    chart::LineChart,
    theme::ActiveTheme,
};

use crate::data::bridge::{Bridge, BridgeConfig};
use crate::data::tick::UnifiedTick;
use crate::data::client::tokio_runtime;
use tokio::sync::broadcast;

/// Ticker panel — displays a live ticker summary with a sparkline chart.
///
/// Ports the visual layout of CoinWatch's `Ticker.svelte` native module.
pub struct TickerPanel {
    exchange: String,
    symbol: String,
    _bridge: Arc<Bridge>,
    tick_rx: broadcast::Receiver<Arc<UnifiedTick>>,
    latest_tick: Option<Arc<UnifiedTick>>,
    price_history: Vec<f64>,
    history_rx: Option<tokio::sync::mpsc::Receiver<Vec<f64>>>,
    is_loading: bool,
    width: f32,
    height: f32,
}

impl TickerPanel {
    pub fn new(exchange: &str, symbol: &str, cx: &mut Context<Self>) -> Self {
        // Coalesce external updates; weak access ends the task on panel close.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                if this.update(cx, |panel, cx| {
                    if panel.pump_events() { cx.notify(); }
                }).is_err() { break; }
            }
        }).detach();
        let mut config = BridgeConfig::default_dev(exchange, symbol);
        config.timeframe = "1h".to_string();
        config.ohlcv_limit = 168; // 168 hourly candles = last 7 days (parity with CoinWatch Ticker.svelte)
        if let Ok(Some(session)) = crate::data::session_store::load_session() {
            config.access_token = Some(session.access_token);
        }

        // A ticker panel owns exactly one bridge and consumes only its ticker
        // stream. Do not merge kline updates into this receiver: kline and
        // ticker packets arrive at different times and made the displayed
        // price jump back and forth (which looked like a WebSocket flicker).
        let bridge = Arc::new(Bridge::start(config.clone()).expect("failed to start bridge for ticker"));
        let tick_rx = bridge.subscribe();

        // One-time fetch of 1h historical chart data for sparkline, plus a 24h periodic refresh
        let (history_tx, history_rx) = tokio::sync::mpsc::channel(1);
        let fetch_config = config.clone();
        tokio_runtime().spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
            loop {
                interval.tick().await;
                if let Ok(candles) = crate::data::bridge::fetch_ohlcv_history(&fetch_config, None).await {
                    let closes: Vec<f64> = candles.into_iter().map(|c| c.close).collect();
                    let _ = history_tx.send(closes).await;
                }
            }
        });

        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            _bridge: bridge,
            tick_rx,
            latest_tick: None,
            price_history: Vec::with_capacity(168),
            history_rx: Some(history_rx),
            is_loading: true,
            width: 300.0,
            height: 180.0,
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

    /// Read any pending ticks to update state before rendering.
    fn pump_events(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = self.history_rx.as_mut() {
            while let Ok(history) = rx.try_recv() {
                changed |= self.price_history != history;
                self.price_history = history;
            }
        }
        loop {
            match self.tick_rx.try_recv() {
                Ok(tick) => {
                    changed |= self.latest_tick.as_ref().is_none_or(|last| !Arc::ptr_eq(last, &tick));
                    self.latest_tick = Some(tick);
                    self.is_loading = false;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        changed
    }
}

impl Render for TickerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.pump_events();



        let colors = &cx.theme().colors;

        if self.is_loading {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.background)
                .child(
                    div()
                        .text_sm()
                        .text_color(colors.muted_foreground)
                        .child("Waiting for connection..."),
                )
                .into_any_element();
        }

        let tick = self.latest_tick.as_ref().unwrap();
        let price = tick.price;

        let mut pct_change = 0.0;
        if !self.price_history.is_empty() {
            let idx_24h_ago = self.price_history.len().saturating_sub(24);
            let price_24h = self.price_history[idx_24h_ago];
            if price_24h > 0.0 {
                pct_change = (price - price_24h) / price_24h * 100.0;
            }
        }

        let is_positive = pct_change >= 0.0;

        let color = if is_positive {
            colors.bullish
        } else {
            colors.bearish
        };

        let badge_bg = if is_positive {
            colors.bullish.opacity(0.14)
        } else {
            colors.bearish.opacity(0.14)
        };

        let arrow = if is_positive { "▲" } else { "▼" };

        let price_str = if price >= 1000.0 {
            format!("{:.2}", price)
        } else if price >= 1.0 {
            format!("{:.4}", price)
        } else if price >= 0.0001 {
            format!("{:.6}", price)
        } else {
            format!("{:.8}", price)
        };

        let mut sparkline_data: Vec<(String, f64)> = self
            .price_history
            .iter()
            .enumerate()
            .map(|(i, &p)| (i.to_string(), p))
            .collect();

        if !sparkline_data.is_empty() {
            sparkline_data.push((sparkline_data.len().to_string(), price));
        }

        let is_compact = self.height < 95.0;
        let is_spacious = self.height >= 170.0 && self.width >= 240.0;

        let stroke_color = if is_positive {
            colors.bullish
        } else {
            colors.bearish
        };

        let chart_element = div()
            .flex_1()
            .w_full()
            .min_h_0()
            .overflow_hidden()
            .child(
                LineChart::new(sparkline_data)
                    .x(|(x, _)| x.clone())
                    .y(|(_, y)| *y)
                    .stroke(stroke_color)
                    .show_axes(false)
                    .show_grid(false)
                    .include_zero(false)
                    .natural(),
            );

        if is_compact {
            div()
                .size_full()
                .flex()
                .flex_col()
                .p_2()
                .bg(colors.background)
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(
                            h_flex()
                                .gap(px(6.0))
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(colors.foreground)
                                        .child(self.symbol.replace("-", "").to_uppercase()),
                                )
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(colors.foreground)
                                        .child(price_str),
                                ),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .rounded_sm()
                                .bg(badge_bg)
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(color)
                                .child(format!("{arrow} {pct_change:+.2}%")),
                        ),
                )
                .child(chart_element)
                .into_any_element()
        } else {
            let price_size = (self.width * 0.08).min(self.height * 0.22).clamp(18.0, 36.0);
            let meta_size = (price_size * 0.42).clamp(10.0, 13.0);
            let badge_size = (price_size * 0.38).clamp(9.0, 12.0);

            let high_24h = self.price_history.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(price);
            let low_24h = self.price_history.iter().copied().fold(f64::INFINITY, f64::min).min(price);

            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(px(10.0))
                .px(px(12.0))
                .pb_1()
                .bg(colors.background)
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .child(
                                    h_flex()
                                        .gap(px(6.0))
                                        .items_center()
                                        .child(
                                            div()
                                                .text_size(px(meta_size))
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(colors.foreground)
                                                .child(self.symbol.replace("-", "").to_uppercase()),
                                        )
                                        .child(
                                            div()
                                                .px(px(5.0))
                                                .py(px(1.0))
                                                .rounded_sm()
                                                .bg(colors.muted)
                                                .text_size(px(meta_size - 1.0))
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .text_color(colors.muted_foreground)
                                                .child(self.exchange.to_uppercase()),
                                        ),
                                )
                                .child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(2.0))
                                        .rounded_sm()
                                        .bg(badge_bg)
                                        .text_size(px(badge_size))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(color)
                                        .child(format!("{arrow} {pct_change:+.2}%")),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_baseline()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(price_size))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(colors.foreground)
                                        .child(price_str),
                                ),
                        )
                        .when(is_spacious && high_24h.is_finite() && low_24h.is_finite(), |this| {
                            this.child(
                                h_flex()
                                    .gap(px(12.0))
                                    .items_center()
                                    .text_size(px(10.0))
                                    .text_color(colors.muted_foreground)
                                    .child(format!("24h H: {:.2}", high_24h))
                                    .child(format!("24h L: {:.2}", low_24h)),
                            )
                        }),
                )
                .child(chart_element)
                .into_any_element()
        }
    }
}