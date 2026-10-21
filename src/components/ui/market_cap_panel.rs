/// Market Cap panel — cryptocurrency market capitalization treemap & ranked table.
///
/// Fetches market data snapshot from `https://cap.coinwat.ch/market-caps`
/// and streams real-time updates via `wss://cap.coinwat.ch/ws`.
///
/// Features dynamic view switching (Heatmap Tiles vs Ranked Table),
/// color-coded 24h performance gradients, global telemetry (Total Market Cap,
/// 24h Global Volume, BTC Dominance), and instant asset search.
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use futures_util::StreamExt;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, Context, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px, relative,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{h_flex, v_flex};
use tokio_tungstenite::connect_async;
use tokio_util::sync::CancellationToken;

use crate::data::client::tokio_runtime;

// ---------------------------------------------------------------------------
// Data Models
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default)]
pub struct MarketCapAsset {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub rank: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "marketCapUsd")]
    pub market_cap_usd: String,
    #[serde(default, rename = "priceUsd")]
    pub price_usd: String,
    #[serde(default, rename = "volumeUsd24Hr")]
    pub volume_usd_24hr: String,
    #[serde(default, rename = "changePercent24Hr")]
    pub change_percent_24hr: Option<String>,
}

impl MarketCapAsset {
    pub fn mcap(&self) -> f64 {
        self.market_cap_usd.parse::<f64>().unwrap_or(0.0)
    }

    pub fn price(&self) -> f64 {
        self.price_usd.parse::<f64>().unwrap_or(0.0)
    }

    pub fn change_24h(&self) -> f64 {
        self.change_percent_24hr
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
    }

    pub fn vol_24h(&self) -> f64 {
        self.volume_usd_24hr.parse::<f64>().unwrap_or(0.0)
    }

    pub fn rank_num(&self) -> usize {
        self.rank.parse::<usize>().unwrap_or(9999)
    }
}

#[derive(serde::Deserialize, Debug)]
struct MarketCapWsMessage {
    #[serde(default, rename = "type")]
    msg_type: String,
    #[serde(default)]
    data: Vec<MarketCapAsset>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarketCapViewMode {
    Heatmap,
    Table,
}

// ---------------------------------------------------------------------------
// MarketCapPanel
// ---------------------------------------------------------------------------

pub struct MarketCapPanel {
    assets: Arc<Mutex<Vec<MarketCapAsset>>>,
    title: String,
    focus_handle: FocusHandle,
    width: f32,
    height: f32,
    view_mode: MarketCapViewMode,
    search_query: String,
    _cancel_token: CancellationToken,
}

impl MarketCapPanel {
    const HEADER_HEIGHT: f32 = 32.0;
    const COMPACT_HEADER_HEIGHT: f32 = 26.0;

    fn header_height(&self) -> f32 {
        if self.width < 300.0 || self.height < 120.0 {
            Self::COMPACT_HEADER_HEIGHT
        } else {
            Self::HEADER_HEIGHT
        }
    }

    pub fn new(title: impl Into<String>, cx: &mut Context<Self>) -> Self {
        let assets = Arc::new(Mutex::new(Vec::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let cancel_token = CancellationToken::new();

        let assets_clone = Arc::clone(&assets);
        let revision_clone = Arc::clone(&revision);
        let token_clone = cancel_token.clone();
        tokio_runtime().spawn(async move {
            run_market_cap_syncer(assets_clone, revision_clone, token_clone).await;
        });

        // Poll only the cheap atomic revision rather than forcing every panel to
        // render at display refresh rate. Stream bursts are coalesced to 4 Hz.
        let revision_watcher = Arc::clone(&revision);
        let watcher_cancel = cancel_token.clone();
        cx.spawn(async move |this, cx| {
            let mut seen = 0;
            loop {
                if watcher_cancel.is_cancelled() {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let current = revision_watcher.load(Ordering::Relaxed);
                if current != seen {
                    seen = current;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();

        Self {
            assets,
            title: title.into(),
            focus_handle: cx.focus_handle(),
            width: 0.0,
            height: 0.0,
            view_mode: MarketCapViewMode::Heatmap,
            search_query: String::new(),
            _cancel_token: cancel_token,
        }
    }

    pub fn set_size(&mut self, width: f32, height: f32) {
        self.width = width.max(0.0);
        self.height = height.max(0.0);
    }

    pub fn has_size(&self, width: f32, height: f32) -> bool {
        (self.width - width).abs() < 0.5 && (self.height - height).abs() < 0.5
    }

    fn fmt_usd(val: f64) -> String {
        if val >= 1_000_000_000_000.0 {
            format!("${:.2}T", val / 1_000_000_000_000.0)
        } else if val >= 1_000_000_000.0 {
            format!("${:.2}B", val / 1_000_000_000.0)
        } else if val >= 1_000_000.0 {
            format!("${:.1}M", val / 1_000_000.0)
        } else {
            format!("${:.0}", val)
        }
    }

    fn fmt_price(price: f64) -> String {
        if price >= 1_000.0 {
            format!("{:.2}", price)
        } else if price >= 1.0 {
            format!("{:.3}", price)
        } else if price >= 0.01 {
            format!("{:.4}", price)
        } else {
            format!("{:.7}", price)
        }
    }
}

impl Render for MarketCapPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Data changes call `cx.notify()` through the coalesced revision watcher;
        // do not request a perpetual animation frame (especially costly for table rows).
        let _ = window;
        let theme = cx.theme();
        let colors = &theme.colors;

        let is_compact = self.width < 380.0;
        let is_narrow = self.width < 280.0;

        let raw_assets = self.assets.lock().unwrap().clone();

        // Cap the table data independently. Large streamed snapshots should not
        // cause a full layout of every asset on every update.
        let table_asset_limit = 50;

        // Calculate global metrics
        let mut total_mcap = 0.0;
        let mut total_vol = 0.0;
        let mut btc_mcap = 0.0;

        for a in &raw_assets {
            let m = a.mcap();
            total_mcap += m;
            total_vol += a.vol_24h();
            if a.symbol.eq_ignore_ascii_case("BTC") {
                btc_mcap = m;
            }
        }

        let btc_dom = if total_mcap > 0.0 {
            (btc_mcap / total_mcap) * 100.0
        } else {
            0.0
        };

        let is_empty = raw_assets.is_empty();

        // Filter assets
        let query = self.search_query.to_lowercase();
        let filtered: Vec<MarketCapAsset> = raw_assets
            .into_iter()
            .filter(|a| {
                if query.is_empty() {
                    true
                } else {
                    a.symbol.to_lowercase().contains(&query)
                        || a.name.to_lowercase().contains(&query)
                }
            })
            .take(if self.view_mode == MarketCapViewMode::Table {
                table_asset_limit
            } else {
                usize::MAX
            })
            .collect();

        v_flex()
            .size_full()
            .bg(colors.background)
            .overflow_hidden()
            // Global Header
            .child(
                v_flex()
                    .w_full()
                    .h(px(self.header_height()))
                    .flex_shrink_0()
                    .bg(colors.table_head)
                    .border_b_1()
                    .border_color(colors.border)
                    .overflow_hidden()
                    .px_2()
                    .py_1()
                    .gap_1()
                    // Row 1: Global metrics
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .gap_1()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .flex_1()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(10.5))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.foreground)
                                            .child(format!("MCap: {}", Self::fmt_usd(total_mcap))),
                                    )
                                    .when(!is_narrow, |h| {
                                        h.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(colors.muted_foreground)
                                                .truncate()
                                                .child(format!("24h Vol: {}", Self::fmt_usd(total_vol))),
                                        )
                                        .when(self.width >= 470.0, |h| {
                                            h.child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(colors.muted_foreground)
                                                    .truncate()
                                                    .child(format!("BTC Dom: {:.1}%", btc_dom)),
                                            )
                                        })
                                    }),
                            )
                            // View Switcher (Treemap vs Table)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .child(self.render_mode_btn(
                                        MarketCapViewMode::Heatmap,
                                        "TREEMAP",
                                        cx,
                                    ))
                                    .child(self.render_mode_btn(
                                        MarketCapViewMode::Table,
                                        "TABLE",
                                        cx,
                                    )),
                            ),
                    ),
            )
            // Main Content Area
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .overflow_hidden()
                    .when(is_empty, |el| {
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
                                        .child("Connecting to market cap stream…"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.muted_foreground.opacity(0.6))
                                        .child("Streaming live capitalization from cap.coinwat.ch"),
                                ),
                        )
                    })
                    .when(!is_empty, |el| {
                        el.child(match self.view_mode {
                            MarketCapViewMode::Heatmap => self
                                .render_heatmap(&filtered, is_compact, is_narrow, cx)
                                .into_any_element(),
                            MarketCapViewMode::Table => self
                                .render_table(&filtered, is_compact, is_narrow, cx)
                                .into_any_element(),
                        })
                    }),
            )
    }
}

impl MarketCapPanel {
    fn render_mode_btn(
        &self,
        mode: MarketCapViewMode,
        label: &'static str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let is_selected = self.view_mode == mode;
        let colors = &cx.theme().colors;

        div()
            .id(ElementId::Name(format!("mode-btn-{label}").into()))
            .px_2()
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
                    this.view_mode = mode;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .text_size(px(9.5))
                    .font_weight(if is_selected {
                        FontWeight::BOLD
                    } else {
                        FontWeight::SEMIBOLD
                    })
                    .text_color(if is_selected {
                        colors.primary_foreground
                    } else {
                        colors.muted_foreground
                    })
                    .child(label),
            )
    }

    fn render_heatmap(
        &self,
        assets: &[MarketCapAsset],
        _is_compact: bool,
        _is_narrow: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;
        // Compute the partition at the main content area's aspect ratio. Tiles
        // are rendered as normalized rectangles, so they continue to fill the
        // viewport exactly throughout a live pane resize.
        let width = self.width.max(1.0);
        let height = (self.height - self.header_height()).max(1.0);

        let mut items: Vec<&MarketCapAsset> = assets.iter().filter(|a| a.mcap() > 0.0).collect();
        items.sort_by(|a, b| {
            b.mcap()
                .partial_cmp(&a.mcap())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // Limit to top 24 assets when not searching to keep layout legible and performant
        if self.search_query.is_empty() {
            // Coinwatch renders the twenty largest assets in its treemap.
            items.truncate(20);
        }

        let rects = Self::treemap_layout(&items, width, height);

        div()
            .id("market-cap-treemap")
            .relative()
            .w_full()
            .h_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(colors.background)
            .children(rects.into_iter().enumerate().map(|(tile_index, (asset, x, y, w, h))| {
                let chg = asset.change_24h();
                let bg_color = if chg >= 5.0 {
                    gpui::rgb(0x16a34a) // Coinwatch strong green
                } else if chg > 0.0 {
                    gpui::rgb(0x86efac) // Coinwatch mint green
                } else if chg <= -5.0 {
                    gpui::rgb(0xdc2626) // Coinwatch deep red
                } else if chg < 0.0 {
                    gpui::rgb(0xfca5a5) // Coinwatch pastel red
                } else {
                    gpui::rgb(0x475569) // Coinwatch slate neutral
                };

                // WCAG contrast matching Coinwatch getTextColor
                let text_color = if (chg > 0.0 && chg < 5.0) || (chg < 0.0 && chg > -5.0) {
                    gpui::rgb(0x020617)
                } else {
                    gpui::rgb(0xf8fafc)
                };
                let text_hsla = gpui::Hsla::from(text_color);

                // Coinwatch only displays content when it can fit in the cell.
                let is_large = w >= 90.0 && h >= 70.0;
                let is_medium = w >= 55.0 && h >= 30.0;
                let is_small = w >= 42.0 && h >= 26.0;

                let symbol_size = if is_large {
                    ((w - 16.0) / (asset.symbol.len().max(1) as f32 * 0.6)).clamp(13.0, 22.0)
                } else if is_medium {
                    ((w - 12.0) / (asset.symbol.len().max(1) as f32 * 0.7)).clamp(10.5, 14.5)
                } else {
                    ((w - 8.0) / (asset.symbol.len().max(1) as f32 * 0.75)).clamp(8.0, 11.0)
                };

                let chg_str = format!("{}{:.2}%", if chg >= 0.0 { "+" } else { "" }, chg);

                div()
                    .id(ElementId::Name(format!("tile-{}-{}", asset.symbol, tile_index).into()))
                    .absolute()
                    // Partition the actual viewport, not the pane grid's
                    // estimated pixel size. A separate inset keeps gutters fixed
                    // at 2px even as this outer rectangle stretches.
                    .left(relative(x / width))
                    .top(relative(y / height))
                    .w(relative(w / width))
                    .h(relative(h / height))
                    .overflow_hidden()
                    .p(px(1.0))
                    .child(div()
                    .size_full()
                    .overflow_hidden()
                    .p(px(if is_large { 6.0 } else if is_medium { 3.0 } else { 0.0 }))
                    .rounded(px(2.0))
                    .bg(bg_color)
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.88))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    // Tiny rectangles remain visible without overflowing labels.
                    .when(is_small, |col| col.child(
                        div()
                            .text_size(px(symbol_size))
                            .font_weight(FontWeight::BOLD)
                            .text_color(text_hsla)
                            .child(asset.symbol.clone()),
                    ))
                    // Coinwatch's tile payload: symbol/icon, then market cap and
                    // change. Price is intentionally omitted from the treemap.
                    .when(is_large, |col| {
                        col.child(
                            div()
                                .text_size(px((symbol_size * 0.72).clamp(9.5, 13.0)))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_hsla.opacity(0.95))
                                .child(format!("{} · {}", Self::fmt_usd(asset.mcap()), chg_str)),
                        )
                    })
                    .when(is_medium && !is_large, |col| {
                        col.child(
                            div()
                                .text_size(px(8.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_hsla.opacity(0.9))
                                .child(chg_str.clone()),
                        )
                    })
                    // Fallback change % for small tiles
                    .when(!is_medium && is_small, |col| {
                        col.child(
                            div()
                                .text_size(px(7.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(text_hsla.opacity(0.9))
                                .child(chg_str),
                        )
                    }),
                )
            }))
    }

    /// Squarified treemap layout algorithm (Bruls, Huizing, van Wijk).
    ///
    /// Subdivides the available 2D viewport `[0, width] x [0, height]` into
    /// non-overlapping rectangles where each rectangle's area is strictly
    /// proportional to its cryptocurrency market capitalization.
    fn treemap_layout<'a>(
        items: &[&'a MarketCapAsset],
        width: f32,
        height: f32,
    ) -> Vec<(&'a MarketCapAsset, f32, f32, f32, f32)> {
        if items.is_empty() || width <= 4.0 || height <= 4.0 {
            return Vec::new();
        }

        let total: f64 = items.iter().map(|a| a.mcap()).sum();
        if total <= 0.0 {
            return Vec::new();
        }

        fn worst(row: &[(usize, f64)], side: f64) -> f64 {
            if row.is_empty() || side <= 0.0 {
                return f64::INFINITY;
            }
            let sum: f64 = row.iter().map(|(_, v)| *v).sum();
            if sum <= 0.0 {
                return f64::INFINITY;
            }
            let max = row.iter().map(|(_, v)| *v).fold(0.0, f64::max);
            let min = row.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
            let s2 = side * side;
            let sum2 = sum * sum;
            let ratio1 = (s2 * max) / sum2;
            let ratio2 = sum2 / (s2 * min.max(1e-9));
            ratio1.max(ratio2)
        }

        let scale = (width as f64 * height as f64) / total;
        let mut remaining: Vec<(usize, f64)> = items
            .iter()
            .enumerate()
            .map(|(i, a)| (i, a.mcap() * scale))
            .collect();

        let mut out = Vec::with_capacity(items.len());
        let mut x = 0.0_f32;
        let mut y = 0.0_f32;
        let mut w = width;
        let mut h = height;

        // Keep partition boundaries continuous. Independently rounding origins
        // and widths creates overlaps/gaps which grow when normalized. Adjacent
        // cells must instead use the same endpoint; GPUI handles pixel rounding.

        while !remaining.is_empty() {
            let side = w.min(h) as f64;
            let mut row: Vec<(usize, f64)> = Vec::new();

            while let Some(candidate) = remaining.first().copied() {
                let mut proposed = row.clone();
                proposed.push(candidate);
                if row.is_empty() || worst(&proposed, side) <= worst(&row, side) {
                    row.push(candidate);
                    remaining.remove(0);
                } else {
                    break;
                }
            }

            let row_area: f64 = row.iter().map(|(_, v)| *v).sum();
            if row_area <= 0.0 {
                break;
            }

            if w >= h {
                // Shorter side is h. Place a column spanning height h with width cw = row_area / h.
                let right = if remaining.is_empty() {
                    x + w
                } else {
                    x + ((row_area / h as f64) as f32).min(w)
                };
                let mut cursor_y = y;
                let row_len = row.len();
                for (position, (i, area)) in row.into_iter().enumerate() {
                    let next_y = if position + 1 == row_len {
                        y + h
                    } else {
                        cursor_y + ((area / row_area) * h as f64) as f32
                    };
                    out.push((items[i], x, cursor_y, right - x, next_y - cursor_y));
                    cursor_y = next_y;
                }
                w = (x + w - right).max(0.0);
                x = right;
            } else {
                // Shorter side is w. Place a row spanning width w with height rh = row_area / w.
                let bottom = if remaining.is_empty() {
                    y + h
                } else {
                    y + ((row_area / w as f64) as f32).min(h)
                };
                let mut cursor_x = x;
                let row_len = row.len();
                for (position, (i, area)) in row.into_iter().enumerate() {
                    let next_x = if position + 1 == row_len {
                        x + w
                    } else {
                        cursor_x + ((area / row_area) * w as f64) as f32
                    };
                    out.push((items[i], cursor_x, y, next_x - cursor_x, bottom - y));
                    cursor_x = next_x;
                }
                h = (y + h - bottom).max(0.0);
                y = bottom;
            }

            // Sub-pixel cells still belong to the partition. Dropping them
            // leaves an unfilled strip and makes the gutters look uneven.
            if !remaining.is_empty() && (w <= 0.0 || h <= 0.0) {
                break;
            }
        }

        out
    }

    fn render_table(
        &self,
        assets: &[MarketCapAsset],
        is_compact: bool,
        is_narrow: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let colors = &cx.theme().colors;

        v_flex()
            .size_full()
            .child(
                // Column headers
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .bg(colors.table_head)
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(px(10.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(colors.muted_foreground)
                    .gap_2()
                    .child(div().w(px(25.0)).child("#"))
                    .child(
                        div()
                            .w(px(if is_narrow { 80.0 } else { 110.0 }))
                            .child("ASSET"),
                    )
                    .child(div().flex_1().text_right().child("PRICE"))
                    .child(div().w(px(65.0)).text_right().child("24H %"))
                    .when(!is_compact, |row| {
                        row.child(div().w(px(75.0)).text_right().child("24H VOL"))
                            .child(div().w(px(85.0)).text_right().child("MARKET CAP"))
                    }),
            )
            .child(
                v_flex()
                    .id("market-cap-table-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .children(assets.iter().enumerate().map(|(idx, a)| {
                        let chg = a.change_24h();
                        let is_positive = chg >= 0.0;
                        let row_bg = if idx.is_multiple_of(2) {
                            colors.table_even
                        } else {
                            colors.table
                        };

                        h_flex()
                            .id(ElementId::Name(format!("market-row-{}", a.id).into()))
                            .w_full()
                            .px_2()
                            .py_1()
                            .bg(row_bg)
                            .gap_2()
                            .items_center()
                            .hover(|s| s.bg(colors.table_hover))
                            .child(
                                div()
                                    .w(px(25.0))
                                    .text_size(px(10.0))
                                    .text_color(colors.muted_foreground)
                                    .child(a.rank.clone()),
                            )
                            .child(
                                h_flex()
                                    .w(px(if is_narrow { 80.0 } else { 110.0 }))
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.foreground)
                                            .child(a.symbol.clone()),
                                    )
                                    .when(!is_narrow, |h| {
                                        h.child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(colors.muted_foreground)
                                                .truncate()
                                                .child(a.name.clone()),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_right()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.foreground)
                                    .child(format!("${}", Self::fmt_price(a.price()))),
                            )
                            .child(
                                div()
                                    .w(px(65.0))
                                    .text_right()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(if is_positive {
                                        colors.bullish
                                    } else {
                                        colors.bearish
                                    })
                                    .child(format!(
                                        "{}{:.2}%",
                                        if is_positive { "+" } else { "" },
                                        chg
                                    )),
                            )
                            .when(!is_compact, |row| {
                                row.child(
                                    div()
                                        .w(px(75.0))
                                        .text_right()
                                        .text_size(px(10.0))
                                        .text_color(colors.muted_foreground)
                                        .child(Self::fmt_usd(a.vol_24h())),
                                )
                                .child(
                                    div()
                                        .w(px(85.0))
                                        .text_right()
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.foreground)
                                        .child(Self::fmt_usd(a.mcap())),
                                )
                            })
                    })),
            )
    }
}

// ---------------------------------------------------------------------------
// Background Data Syncer
// ---------------------------------------------------------------------------

async fn run_market_cap_syncer(
    assets_target: Arc<Mutex<Vec<MarketCapAsset>>>,
    revision: Arc<AtomicU64>,
    cancel_token: CancellationToken,
) {
    // REST is a snapshot fallback only; race it against panel cancellation.
    let client = reqwest::Client::new();
    let fetch = async {
        if let Ok(res) = client
            .get("https://cap.coinwat.ch/market-caps?limit=50")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                if let Some(arr) = json.get("data").and_then(|v| v.as_array()) {
                    let parsed: Vec<MarketCapAsset> = arr
                        .iter()
                        .filter_map(|val| {
                            serde_json::from_value::<MarketCapAsset>(val.clone()).ok()
                        })
                        .collect();
                    if !parsed.is_empty() {
                        if let Ok(mut lock) = assets_target.lock() {
                            *lock = parsed;
                            revision.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
    };
    tokio::select! {
        _ = cancel_token.cancelled() => return,
        _ = fetch => {}
    }

    // 2. Connect to WebSocket stream for live updates
    loop {
        if cancel_token.is_cancelled() {
            break;
        }

        let connection = tokio::select! {
            _ = cancel_token.cancelled() => break,
            result = connect_async("wss://cap.coinwat.ch/ws") => result,
        };
        match connection {
            Ok((ws_stream, _)) => {
                let (_, mut read) = ws_stream.split();
                loop {
                    let msg = tokio::select! {
                        _ = cancel_token.cancelled() => return,
                        msg = read.next() => msg,
                    };
                    let Some(msg) = msg else { break };
                    if let Ok(tokio_tungstenite::tungstenite::Message::Text(text)) = msg {
                        if let Ok(parsed) = serde_json::from_str::<MarketCapWsMessage>(&text) {
                            if !parsed.data.is_empty() {
                                if let Ok(mut lock) = assets_target.lock() {
                                    if parsed.msg_type == "snapshot" {
                                        *lock = parsed.data;
                                    } else {
                                        // Update delta map
                                        let mut current_map: HashMap<String, MarketCapAsset> =
                                            lock.drain(..).map(|a| (a.id.clone(), a)).collect();
                                        for update in parsed.data {
                                            if let Some(existing) = current_map.get_mut(&update.id)
                                            {
                                                if !update.price_usd.is_empty() {
                                                    existing.price_usd = update.price_usd;
                                                }
                                                if !update.market_cap_usd.is_empty() {
                                                    existing.market_cap_usd = update.market_cap_usd;
                                                }
                                                if update.change_percent_24hr.is_some() {
                                                    existing.change_percent_24hr =
                                                        update.change_percent_24hr;
                                                }
                                                if !update.volume_usd_24hr.is_empty() {
                                                    existing.volume_usd_24hr =
                                                        update.volume_usd_24hr;
                                                }
                                            } else {
                                                current_map.insert(update.id.clone(), update);
                                            }
                                        }
                                        let mut updated_list: Vec<MarketCapAsset> =
                                            current_map.into_values().collect();
                                        updated_list.sort_by_key(|a| a.rank_num());
                                        *lock = updated_list;
                                    }
                                    revision.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }
            }
            Err(error) => {
                tracing::debug!(%error, "market cap stream connection failed");
            }
        }
        // Back off after both failed connections and clean/abrupt disconnects.
        tokio::select! {
            _ = cancel_token.cancelled() => break,
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(3)) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// GPUI dock integration
// ---------------------------------------------------------------------------

impl Drop for MarketCapPanel {
    fn drop(&mut self) {
        self._cancel_token.cancel();
    }
}

impl Panel for MarketCapPanel {
    fn panel_name(&self) -> &'static str {
        "MarketCapPanel"
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
            panel_name: "MarketCapPanel".to_string(),
            children: vec![],
            info: PanelInfo::Panel(serde_json::json!({
                "title": self.title,
            })),
        }
    }
}

impl EventEmitter<PanelEvent> for MarketCapPanel {}

impl Focusable for MarketCapPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_treemap_layout_bounds() {
        let assets = vec![
            MarketCapAsset {
                symbol: "BTC".into(),
                market_cap_usd: "1200000000000".into(),
                ..Default::default()
            },
            MarketCapAsset {
                symbol: "ETH".into(),
                market_cap_usd: "400000000000".into(),
                ..Default::default()
            },
            MarketCapAsset {
                symbol: "SOL".into(),
                market_cap_usd: "80000000000".into(),
                ..Default::default()
            },
            MarketCapAsset {
                symbol: "BNB".into(),
                market_cap_usd: "70000000000".into(),
                ..Default::default()
            },
        ];
        let refs: Vec<&MarketCapAsset> = assets.iter().collect();
        let (w, h) = (400.0, 300.0);
        let layout = MarketCapPanel::treemap_layout(&refs, w, h);
        assert_eq!(layout.len(), 4);

        for (asset, x, y, tile_w, tile_h) in layout {
            assert!(x >= 0.0, "x should be non-negative: {x}");
            assert!(y >= 0.0, "y should be non-negative: {y}");
            assert!(tile_w > 0.0, "width should be positive: {tile_w}");
            assert!(tile_h > 0.0, "height should be positive: {tile_h}");
            assert!(
                x + tile_w <= w + 1.5,
                "tile {} overflows width: {} + {} > {}",
                asset.symbol,
                x,
                tile_w,
                w
            );
            assert!(
                y + tile_h <= h + 1.5,
                "tile {} overflows height: {} + {} > {}",
                asset.symbol,
                y,
                tile_h,
                h
            );

            // Relative positioning bounds check
            let rx = x / w;
            let ry = y / h;
            let rw = tile_w / w;
            let rh = tile_h / h;
            assert!(rx >= 0.0 && rx <= 1.0);
            assert!(ry >= 0.0 && ry <= 1.0);
            assert!(rw > 0.0 && rw <= 1.0);
            assert!(rh > 0.0 && rh <= 1.0);
            assert!(rx + rw <= 1.01);
            assert!(ry + rh <= 1.01);
        }
    }
}

