//! Lightweight inline candlestick chart for the AI Agent conversation.
//!
//! The Agent model emits a `chart` directive in its reply. `AgentPanel` parses
//! that directive and creates one of these entities, which fetches the
//! requested OHLCV snapshot through the same gateway used by the Chart module
//! and renders a static candlestick chart directly inside the chat message.

use gpui::*;
use gpui_component::chart::{CandleType, CandlestickChart, ChartState};
use gpui_component::theme::ActiveTheme;
use gpui_component::v_flex;

use crate::data::agent::{fetch_chart_candles, ChartRequest};
use crate::data::models::OhlcvCandle;
use crate::data::price_format::format_price;

/// Fixed height of the inline plot area (header excluded).
const CHART_HEIGHT: f32 = 220.0;

pub struct AgentChart {
    request: ChartRequest,
    candles: Vec<OhlcvCandle>,
    loading: bool,
    error: Option<String>,
    chart_state: ChartState,
}

impl AgentChart {
    pub fn new(request: ChartRequest, cx: &mut Context<Self>) -> Self {
        let mut chart = Self {
            request: request.normalized(),
            candles: Vec::new(),
            loading: true,
            error: None,
            chart_state: ChartState::new(),
        };
        chart.fetch_snapshot(cx);
        chart
    }

    fn fetch_snapshot(&mut self, cx: &mut Context<Self>) {
        if let Some(error) = &self.request.range_error {
            self.loading = false;
            self.error = Some(error.clone());
            return;
        }

        if !crate::data::client::is_api_url_configured() {
            self.loading = false;
            self.error = Some(
                "No API service URL configured. Configure it in Settings to load charts."
                    .to_string(),
            );
            return;
        }

        self.loading = true;
        self.error = None;
        let request = self.request.clone();

        cx.spawn(async move |this, cx| {
            let result = crate::data::client::tokio_runtime()
                .spawn(async move { fetch_chart_candles(&request).await })
                .await
                .unwrap_or_else(|error| Err(anyhow::anyhow!(error)));

            let _ = this.update(cx, |chart, cx| {
                chart.loading = false;
                match result {
                    Ok(candles) if !candles.is_empty() => {
                        chart.chart_state = ChartState::new();
                        // Show every requested candle at once instead of the
                        // component's default 100-candle viewport.
                        chart.chart_state.visible_count = candles.len().max(1);
                        chart.chart_state.viewport_offset = 0.0;
                        chart.chart_state.pinned_to_latest = true;
                        chart.candles = candles;
                        chart.error = None;
                    }
                    Ok(_) => {
                        chart.error = Some("No candles returned for this market.".to_string());
                    }
                    Err(error) => {
                        chart.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_header(&self, cx: &Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let candle_count = if self.candles.is_empty() {
            self.request.candle_limit() as usize
        } else {
            self.candles.len()
        };
        let subtitle = if let Some(range) = self.request.range_label() {
            format!("{range} · {candle_count} candles")
        } else {
            format!("Snapshot · {candle_count} candles")
        };

        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .px(px(10.0))
            .py(px(7.0))
            .border_b_1()
            .border_color(colors.border.opacity(0.4))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.foreground)
                    .truncate()
                    .child(self.request.display_title()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(subtitle),
            )
            .into_any_element()
    }
}

impl Render for AgentChart {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors.clone();

        let body = if self.loading {
            div()
                .h(px(CHART_HEIGHT))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(colors.muted_foreground)
                .child("Loading requested data…")
                .into_any_element()
        } else if let Some(error) = &self.error {
            div()
                .h(px(CHART_HEIGHT))
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(14.0))
                .text_xs()
                .text_color(colors.bearish)
                .child(error.clone())
                .into_any_element()
        } else {
            let candles = self.candles.clone();
            let candle_type = match self.request.style() {
                "area" => CandleType::Area,
                "ohlc" => CandleType::Ohlc,
                "hollow" => CandleType::Stroke,
                "heikin_ashi" => CandleType::HeikinAshi,
                _ => CandleType::Solid,
            };
            let chart = CandlestickChart::new(candles)
                .timestamp_ms(|candle: &OhlcvCandle| candle.timestamp)
                .open(|candle: &OhlcvCandle| candle.open)
                .high(|candle: &OhlcvCandle| candle.high)
                .low(|candle: &OhlcvCandle| candle.low)
                .close(|candle: &OhlcvCandle| candle.close)
                .volume(|candle: &OhlcvCandle| candle.volume)
                .candle_type(candle_type)
                .body_width_ratio(0.7)
                // The volume pane occupies subpane slot 0. The chart renderer
                // indexes `subpane_ratios[0]`, so reserve it explicitly.
                .subpane_count(1)
                .show_volume_pane(true)
                // Inline charts are a static snapshot: no last-price ticker
                // line, badge, or live countdown.
                .show_last_price(false)
                .chart_state(self.chart_state.clone())
                .format_price(|price: f64| format_price(price, None).into())
                .format_timestamp_short(|timestamp, best_interval, previous_timestamp| {
                    let date =
                        chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp / 1_000, 0)
                            .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
                    let is_new_day = previous_timestamp
                        .map(|previous| {
                            let previous = chrono::DateTime::<chrono::Utc>::from_timestamp(
                                previous / 1_000,
                                0,
                            )
                            .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
                            previous.date_naive() != date.date_naive()
                        })
                        .unwrap_or(false);
                    let format = if best_interval >= 30.0 * 86_400_000.0 {
                        "%Y-%m"
                    } else if best_interval >= 86_400_000.0 || is_new_day {
                        "%m-%d"
                    } else if previous_timestamp.is_none() {
                        "%m-%d %H:%M"
                    } else {
                        "%H:%M"
                    };
                    date.format(format).to_string().into()
                })
                .format_timestamp_long(|timestamp| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp / 1_000, 0)
                        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
                        .format("%b %d %H:%M UTC")
                        .to_string()
                        .into()
                });

            div()
                .h(px(CHART_HEIGHT))
                .w_full()
                .relative()
                .overflow_hidden()
                .child(chart)
                .into_any_element()
        };

        v_flex()
            .w_full()
            .min_w_0()
            .rounded(px(8.0))
            .border_1()
            .border_color(colors.border.opacity(0.5))
            .bg(colors.muted.opacity(0.12))
            .overflow_hidden()
            .child(self.render_header(cx))
            .child(body)
    }
}
