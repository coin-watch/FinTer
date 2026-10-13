use std::sync::Arc;

use gpui::{
    div, px, App, Context, IntoElement, ParentElement, Render, Styled, Window,
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
}

impl TickerPanel {
    pub fn new(exchange: &str, symbol: &str, _cx: &mut App) -> Self {
        let mut config = BridgeConfig::default_dev(exchange, symbol);
        config.timeframe = "1h".to_string();
        config.ohlcv_limit = 168; // 168 hourly candles = last 7 days (parity with CoinWatch Ticker.svelte)
        
        let bridge = Arc::new(Bridge::start(config.clone()).expect("failed to start bridge for ticker"));
        let mut tick_rx = bridge.subscribe();
        let mut ohlcv_rx = bridge.subscribe_ohlcv();

        // Listen for either tick updates or kline updates (which carry live close price)
        let (merged_tx, merged_rx) = tokio::sync::broadcast::channel(64);
        let m_tx = merged_tx.clone();
        let ex = exchange.to_string();
        let sym = symbol.to_string();

        tokio_runtime().spawn(async move {
            loop {
                tokio::select! {
                    Ok(tick) = tick_rx.recv() => {
                        let _ = m_tx.send(tick);
                    }
                    Ok(candles) = ohlcv_rx.recv() => {
                        if let Some(c) = candles.last() {
                            let tick = UnifiedTick::new(
                                &ex,
                                &sym,
                                c.close,
                                c.volume,
                                crate::data::tick::TradeSide::Unknown,
                                c.timestamp,
                            );
                            let _ = m_tx.send(Arc::new(tick));
                        }
                    }
                    else => break,
                }
            }
        });

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
            tick_rx: merged_rx,
            latest_tick: None,
            price_history: Vec::with_capacity(168),
            history_rx: Some(history_rx),
            is_loading: true,
        }
    }

    /// Read any pending ticks to update state before rendering.
    fn pump_events(&mut self) {
        if let Some(rx) = self.history_rx.as_mut() {
            while let Ok(history) = rx.try_recv() {
                self.price_history = history;
            }
        }
        while let Ok(tick) = self.tick_rx.try_recv() {
            // We no longer append live prices to the sparkline array.
            // The sparkline is purely the static 1h history over 7 days.
            self.latest_tick = Some(tick);
            self.is_loading = false;
        }
    }
}

impl Render for TickerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.pump_events();

        // Ensure we keep polling ticks
        window.request_animation_frame();

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

        div()
            .size_full()
            .relative()
            .bg(colors.background)
            .child(
                h_flex()
                    .absolute()
                    .top(px(8.0))
                    .left(px(12.0))
                    .gap_2()
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
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(colors.foreground)
                            .child(format!("{:.2}", price)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(color)
                            .child(format!("{pct_change:+.2}%")),
                    ),
            )
            // LineChart component sparkline
            .child({
                let mut data: Vec<(String, f64)> = self
                    .price_history
                    .iter()
                    .enumerate()
                    .map(|(i, &p)| (i.to_string(), p))
                    .collect();
                
                // Append current live price to complete the sparkline
                if !data.is_empty() {
                    data.push((data.len().to_string(), price));
                }

                let stroke_color = if is_positive {
                    colors.bullish
                } else {
                    colors.bearish
                };

                div()
                    .size_full()
                    .child(
                        LineChart::new(data)
                            .x(|(x, _)| x.clone())
                            .y(|(_, y)| *y)
                            .stroke(stroke_color)
                            .show_axes(false)
                            .show_grid(false)
                            .include_zero(false)
                            .natural()
                    )
            })
            .into_any_element()
    }
}