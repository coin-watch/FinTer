/// Historical raw trades and their aggregation into order-flow bars.
///
/// CoinWatch exposes the persisted trade tape through a cursor-paginated route:
///
/// ```text
/// GET {rest}/api/v1/trades/history?exchange&symbol&since&until&limit&type&order&cursor
/// ```
///
/// The backend keeps 14 days of raw trades. Pages are requested newest first
/// (`order=desc`) and followed through `next_cursor` until the requested
/// `since` is covered, the cursor is exhausted, or the local page/row cap is
/// reached. Aggregation runs once per explicit user load on a Tokio task; the
/// render thread only merges the resulting bars.
use std::collections::HashSet;

use serde::{Deserialize, Deserializer};

use crate::data::bridge::BridgeConfig;
use crate::data::orderflow_feed::{FlowBar, PriceFlow};
use crate::data::tick::TradeSide;

/// Maximum rows the relay accepts per request.
pub const TRADE_HISTORY_PAGE_LIMIT: usize = 20_000;
/// Hard page cap for one chart load (12 × 20k = 240k trades).
pub const TRADE_HISTORY_MAX_PAGES: usize = 12;
/// The backend retains 14 days of raw trades; older `since` values are rejected.
pub const TRADE_HISTORY_RETENTION_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// Seconds below this magnitude are treated as epoch seconds and multiplied by
/// 1000. Current epoch milliseconds are ~1.7e12.
const EPOCH_MS_CUTOFF: i64 = 10_000_000_000;

/// One persisted trade print.
///
/// `ts` is the canonical exchange event time in epoch milliseconds. The decoder
/// also accepts `timestamp`/`exchange_ts` aliases, numeric strings, and a
/// numeric `id` so older gateway payloads keep working.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradePrint {
    pub trade_id: Option<String>,
    /// Exchange event time in epoch milliseconds.
    pub exchange_ts: i64,
    /// Canonical event time used for bucketing (alias of `exchange_ts`).
    pub ts: i64,
    pub price: f64,
    pub amount: f64,
    pub cost: Option<f64>,
    /// Taker side. `None` when the venue/storage omitted it.
    pub side: Option<TradeSide>,
    /// `buy: +amount`, `sell: -amount`, `None`/unknown: `0`.
    pub signed_amount: Option<f64>,
}

impl TradePrint {
    pub fn key(&self) -> (i64, String) {
        let id = self
            .trade_id
            .clone()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                // Preserve rows without an exchange id instead of collapsing
                // every id-less print in the same millisecond.
                format!(
                    "{}:{:x}:{:x}",
                    self.ts,
                    self.price.to_bits(),
                    self.amount.to_bits()
                )
            });
        (self.exchange_ts, id)
    }
}

/// One page of the cursor-paginated trade-tape history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeHistoryPage {
    pub instrument: Option<serde_json::Value>,
    pub exchange: String,
    pub symbol: String,
    pub source: String,
    pub since: i64,
    pub until: i64,
    pub limit: u32,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub partial: bool,
    pub queued: bool,
    pub data: Vec<TradePrint>,
}

impl TradeHistoryPage {
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Failure modes the chart distinguishes for the order-flow history note.
#[derive(Clone, Debug, thiserror::Error)]
pub enum TradeHistoryError {
    /// Missing or expired credentials; the live feed still works.
    #[error("sign-in required")]
    Auth,
    /// The account ran out of credits.
    #[error("{0}")]
    Credits(String),
    /// The requested window is not covered (for example, entirely inside the
    /// relay's short data lag).
    #[error("data is not available yet")]
    NotAvailable,
    /// Network, configuration, parse, or other failure.
    #[error("{0}")]
    Other(String),
}

/// Result reported to the chart after an explicit history load.
#[derive(Clone, Debug, PartialEq)]
pub enum TradeHistoryOutcome {
    /// Aggregated bars are ready to merge behind the live feed.
    Loaded {
        bars: Vec<FlowBar>,
        trades: usize,
        partial: bool,
    },
    /// Missing/expired credentials. The live feed is unaffected.
    AuthRequired,
    /// Credit exhaustion; the message is user-facing.
    Credits(String),
    /// The relay has no coverage for the requested window.
    NotAvailable,
    /// Network/parse/other failure.
    Error(String),
}

#[derive(Deserialize)]
struct TradePrintWire {
    #[serde(default)]
    trade_id: Option<serde_json::Value>,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    exchange_ts: Option<serde_json::Value>,
    #[serde(default)]
    ts: Option<serde_json::Value>,
    #[serde(default)]
    timestamp: Option<serde_json::Value>,
    #[serde(default)]
    price: Option<serde_json::Value>,
    #[serde(default)]
    amount: Option<serde_json::Value>,
    #[serde(default)]
    qty: Option<serde_json::Value>,
    #[serde(default)]
    size: Option<serde_json::Value>,
    #[serde(default)]
    cost: Option<serde_json::Value>,
    #[serde(default)]
    side: Option<serde_json::Value>,
    #[serde(default)]
    signed_amount: Option<serde_json::Value>,
}

impl<'de> Deserialize<'de> for TradePrint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TradePrintWire::deserialize(deserializer)?;
        // Canonical `ts` wins; fall back to `exchange_ts` and the legacy
        // `timestamp` spelling.
        let ts = wire
            .ts
            .as_ref()
            .and_then(epoch_ms_from_value)
            .or_else(|| wire.exchange_ts.as_ref().and_then(epoch_ms_from_value))
            .or_else(|| wire.timestamp.as_ref().and_then(epoch_ms_from_value))
            .unwrap_or(0);
        let exchange_ts = wire
            .exchange_ts
            .as_ref()
            .and_then(epoch_ms_from_value)
            .unwrap_or(ts);
        let trade_id = wire
            .trade_id
            .as_ref()
            .or(wire.id.as_ref())
            .and_then(string_from_value)
            .filter(|id| !id.is_empty());
        let price = wire.price.as_ref().and_then(number).unwrap_or(0.0);
        let amount = wire
            .amount
            .as_ref()
            .or(wire.qty.as_ref())
            .or(wire.size.as_ref())
            .and_then(number)
            .unwrap_or(0.0);
        let cost = wire
            .cost
            .as_ref()
            .and_then(number)
            .filter(|cost| *cost != 0.0);
        let side = wire.side.as_ref().and_then(side_from_value);
        let signed_amount = wire.signed_amount.as_ref().and_then(number);
        // A missing timestamp stays `0` and is dropped by `dedupe_trades`
        // instead of failing the whole page.
        Ok(Self {
            trade_id,
            exchange_ts,
            ts,
            price,
            amount,
            cost,
            side,
            signed_amount,
        })
    }
}

#[derive(Deserialize)]
struct TradeHistoryWire {
    #[serde(default)]
    instrument: Option<serde_json::Value>,
    #[serde(default)]
    exchange: String,
    #[serde(default)]
    symbol: String,
    #[serde(default)]
    source: String,
    #[serde(default, deserialize_with = "de_i64")]
    since: i64,
    #[serde(default, deserialize_with = "de_i64")]
    until: i64,
    #[serde(default, deserialize_with = "de_u32")]
    limit: u32,
    #[serde(default)]
    has_more: bool,
    #[serde(default, deserialize_with = "de_opt_string")]
    next_cursor: Option<String>,
    #[serde(default)]
    partial: bool,
    #[serde(default)]
    queued: bool,
    #[serde(default)]
    data: Vec<TradePrint>,
}

impl<'de> Deserialize<'de> for TradeHistoryPage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TradeHistoryWire::deserialize(deserializer)?;
        Ok(Self {
            instrument: wire.instrument,
            exchange: wire.exchange,
            symbol: wire.symbol,
            source: wire.source,
            since: wire.since,
            until: wire.until,
            limit: wire.limit,
            has_more: wire.has_more,
            next_cursor: wire.next_cursor.filter(|cursor| !cursor.is_empty()),
            partial: wire.partial,
            queued: wire.queued,
            data: wire.data,
        })
    }
}

/// Decode a canonical or legacy (`{success, data}`) trade-history payload.
///
/// Unknown metadata fields are ignored. A bare data array, or a payload with
/// the array nested under `data.data`, is also accepted.
pub fn parse_trade_history(value: serde_json::Value) -> TradeHistoryPage {
    serde_json::from_value(normalize_envelope(value)).unwrap_or_default()
}

fn normalize_envelope(value: serde_json::Value) -> serde_json::Value {
    if value.is_array() {
        return serde_json::json!({ "data": value });
    }
    if !value.is_object() {
        return serde_json::Value::Object(Default::default());
    }
    let nested = value
        .get("data")
        .filter(|data| data.is_object())
        .and_then(|data| data.get("data"))
        .filter(|data| data.is_array())
        .cloned();
    if let Some(rows) = nested {
        let mut object = value;
        if let Some(map) = object.as_object_mut() {
            map.insert("data".to_string(), rows);
        }
        return object;
    }
    if value.get("data").is_none() {
        if let Some(rows) = value
            .get("trades")
            .or_else(|| value.get("history"))
            .filter(|rows| rows.is_array())
            .cloned()
        {
            let mut object = value;
            if let Some(map) = object.as_object_mut() {
                map.insert("data".to_string(), rows);
            }
            return object;
        }
    }
    value
}

/// Load historical trades for `[since, until]` and aggregate them into flow
/// bars. Only call this from an explicit user action: the route is metered.
///
/// Paging is newest-first. The first request carries `since`; follow-up pages
/// carry `cursor` (the relay rejects `cursor` + `since` together). The loop is
/// bounded by [`TRADE_HISTORY_MAX_PAGES`] × [`TRADE_HISTORY_PAGE_LIMIT`].
pub async fn load_trade_history(
    config: &BridgeConfig,
    since: i64,
    until: i64,
    timeframe_ms: i64,
) -> TradeHistoryOutcome {
    if config.rest_base_url.trim().is_empty() {
        return TradeHistoryOutcome::Error("No API service URL configured".to_string());
    }
    let now = now_ms();
    let until = if until <= 0 { now } else { until.min(now) };
    let requested_since = since;
    // The backend rejects `since` older than its 14-day retention edge.
    let since = since.max(now - TRADE_HISTORY_RETENTION_MS).max(0);
    if since <= 0 || since >= until {
        return TradeHistoryOutcome::Error("invalid trade-history window".to_string());
    }
    let mut partial = since > requested_since;

    let mut cursor: Option<String> = None;
    let mut pages = 0usize;
    let mut rows: Vec<TradePrint> = Vec::new();

    while pages < TRADE_HISTORY_MAX_PAGES {
        let url = config.trades_history_url(
            since,
            until,
            TRADE_HISTORY_PAGE_LIMIT,
            "desc",
            cursor.as_deref(),
        );
        let page = match crate::data::bridge::fetch_trades_history_page(config, &url).await {
            Ok(page) => page,
            Err(error) => {
                if pages == 0 {
                    return outcome_from_error(error);
                }
                // Keep the pages already fetched and report a partial load.
                partial = true;
                break;
            }
        };
        pages = pages.saturating_add(1);
        partial |= page.partial || page.queued;
        let continue_paging = should_request_next(&page, since, cursor.as_deref());
        rows.extend(page.data);
        if !continue_paging {
            break;
        }
        cursor = page.next_cursor;
        if pages >= TRADE_HISTORY_MAX_PAGES {
            // Another page is needed but the budget is exhausted.
            partial = true;
        }
    }

    let trades = dedupe_trades(rows);
    let trade_count = trades.len();
    let bars = aggregate_trades(&trades, timeframe_ms);
    TradeHistoryOutcome::Loaded {
        bars,
        trades: trade_count,
        partial,
    }
}

fn outcome_from_error(error: TradeHistoryError) -> TradeHistoryOutcome {
    match error {
        TradeHistoryError::Auth => TradeHistoryOutcome::AuthRequired,
        TradeHistoryError::Credits(message) => TradeHistoryOutcome::Credits(message),
        TradeHistoryError::NotAvailable => TradeHistoryOutcome::NotAvailable,
        TradeHistoryError::Other(message) => TradeHistoryOutcome::Error(message),
    }
}

/// Whether another page should be requested.
///
/// A page stops paging when it has no cursor, reports no more rows, repeats the
/// previous cursor, is empty, or has already reached the requested `since`.
fn should_request_next(page: &TradeHistoryPage, since: i64, previous_cursor: Option<&str>) -> bool {
    let Some(next) = page
        .next_cursor
        .as_deref()
        .filter(|cursor| !cursor.is_empty())
    else {
        return false;
    };
    let Some(last_ts) = page.data.last().map(|trade| trade.ts) else {
        return false;
    };
    page.has_more && previous_cursor != Some(next) && last_ts > since
}

/// Deduplicate by `(exchange_ts, trade_id)` and sort ascending by event time.
pub fn dedupe_trades(trades: Vec<TradePrint>) -> Vec<TradePrint> {
    let mut seen = HashSet::with_capacity(trades.len());
    let mut unique = Vec::with_capacity(trades.len());
    for trade in trades {
        if trade.ts <= 0 {
            continue;
        }
        if seen.insert(trade.key()) {
            unique.push(trade);
        }
    }
    unique.sort_by(|a, b| {
        a.ts.cmp(&b.ts)
            .then_with(|| a.exchange_ts.cmp(&b.exchange_ts))
            .then_with(|| a.trade_id.cmp(&b.trade_id))
    });
    unique
}

/// Aggregate prints into exact-price [`FlowBar`]s.
///
/// Trades without a side are skipped (the relay stores an empty side as null;
/// they cannot be attributed as buy or sell volume). POC/value area are
/// intentionally left unset; the footprint builder derives them from the
/// price rows.
pub fn aggregate_trades(trades: &[TradePrint], timeframe_ms: i64) -> Vec<FlowBar> {
    use std::collections::BTreeMap;

    let timeframe_ms = timeframe_ms.max(1_000);
    let mut bars: BTreeMap<i64, FlowBar> = BTreeMap::new();
    for trade in trades {
        if trade.ts <= 0
            || !trade.price.is_finite()
            || !trade.amount.is_finite()
            || trade.price <= 0.0
            || trade.amount <= 0.0
        {
            continue;
        }
        let Some(side) = trade.side.as_ref() else {
            continue;
        };
        let buy = matches!(side, TradeSide::Buy);
        if !buy && !matches!(side, TradeSide::Sell) {
            continue;
        }
        let bar_ts = trade.ts.div_euclid(timeframe_ms) * timeframe_ms;
        let bar = bars.entry(bar_ts).or_insert(FlowBar {
            timestamp: bar_ts,
            ..FlowBar::default()
        });
        if buy {
            bar.buy_volume += trade.amount;
        } else {
            bar.sell_volume += trade.amount;
        }
        bar.trades = bar.trades.saturating_add(1);

        let price_row = match bar
            .price_rows
            .iter_mut()
            .find(|row| (row.price_low - trade.price).abs() < 1e-9)
        {
            Some(row) => row,
            None => {
                bar.price_rows.push(PriceFlow {
                    price: trade.price,
                    price_low: trade.price,
                    price_high: trade.price,
                    ..PriceFlow::default()
                });
                bar.price_rows.last_mut().expect("row pushed")
            }
        };
        if buy {
            price_row.buy_volume += trade.amount;
            price_row.buy_count = price_row.buy_count.saturating_add(1);
        } else {
            price_row.sell_volume += trade.amount;
            price_row.sell_count = price_row.sell_count.saturating_add(1);
        }
    }

    let mut out: Vec<FlowBar> = bars.into_values().collect();
    for bar in &mut out {
        bar.price_rows
            .sort_by(|a, b| a.price_low.total_cmp(&b.price_low));
        bar.total_volume = bar.buy_volume + bar.sell_volume;
        bar.row_size = 0.0;
        bar.is_closed = true;
    }
    out
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn epoch_ms(raw: i64) -> i64 {
    if raw != 0 && raw.abs() < EPOCH_MS_CUTOFF {
        raw.saturating_mul(1000)
    } else {
        raw
    }
}

fn epoch_ms_from_value(value: &serde_json::Value) -> Option<i64> {
    let raw = value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
        .or_else(|| value.as_f64().map(|v| v as i64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))?;
    Some(epoch_ms(raw))
}

fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|v| v as f64))
        .or_else(|| value.as_u64().map(|v| v as f64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|v| v.is_finite())
}

fn string_from_value(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(number) = value.as_i64() {
        return Some(number.to_string());
    }
    if let Some(number) = value.as_u64() {
        return Some(number.to_string());
    }
    value.as_f64().map(|number| number.to_string())
}

fn side_from_value(value: &serde_json::Value) -> Option<TradeSide> {
    let text = value.as_str()?.trim().to_ascii_lowercase();
    match text.as_str() {
        "buy" | "b" | "bid" => Some(TradeSide::Buy),
        "sell" | "s" | "ask" => Some(TradeSide::Sell),
        _ => None,
    }
}

fn de_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(epoch_ms_from_value(&value).unwrap_or_default())
}

fn de_u32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<u32>().ok()))
        .unwrap_or_default())
}

fn de_opt_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(string_from_value(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print(id: &str, side: Option<TradeSide>, price: f64, amount: f64, ts: i64) -> TradePrint {
        TradePrint {
            trade_id: Some(id.to_string()),
            exchange_ts: ts,
            ts,
            price,
            amount,
            cost: None,
            side,
            signed_amount: None,
        }
    }

    #[test]
    fn decodes_aliases_numeric_strings_and_null_side() {
        let page = parse_trade_history(serde_json::json!({
            "exchange": "binance",
            "symbol": "BTC/USDT",
            "partial": true,
            "has_more": true,
            "next_cursor": "abc",
            "data": [
                {
                    "id": 42,
                    "ts": "1700000000",
                    "price": "100.5",
                    "amount": "2",
                    "side": null
                },
                {
                    "trade_id": "43",
                    "exchange_ts": 1_700_000_001_123i64,
                    "price": 101,
                    "amount": 1,
                    "side": "SELL",
                    "cost": "0"
                },
                {
                    "trade_id": "44",
                    "timestamp": 1_700_000_002,
                    "price": "102",
                    "qty": "3",
                    "side": "Buy"
                }
            ]
        }));
        assert_eq!(page.data.len(), 3);
        assert!(page.has_more);
        assert_eq!(page.next_cursor.as_deref(), Some("abc"));

        let first = &page.data[0];
        assert_eq!(first.trade_id.as_deref(), Some("42"));
        assert_eq!(first.ts, 1_700_000_000_000);
        assert_eq!(first.exchange_ts, first.ts);
        assert_eq!(first.price, 100.5);
        assert_eq!(first.amount, 2.0);
        assert!(first.side.is_none());

        let second = &page.data[1];
        // `ts` is absent, so `exchange_ts` fills both fields.
        assert_eq!(second.ts, 1_700_000_001_123);
        assert_eq!(second.exchange_ts, 1_700_000_001_123);
        assert_eq!(second.side, Some(TradeSide::Sell));
        assert_eq!(second.cost, None);

        let third = &page.data[2];
        assert_eq!(third.ts, 1_700_000_002_000);
        assert_eq!(third.amount, 3.0);
        assert_eq!(third.side, Some(TradeSide::Buy));
    }

    #[test]
    fn aggregation_buckets_splits_sides_and_skips_null_side() {
        let trades = vec![
            print("1", Some(TradeSide::Buy), 100.0, 2.0, 60_500),
            print("2", Some(TradeSide::Sell), 100.0, 1.0, 60_999),
            print("3", None, 100.0, 9.0, 60_999),
            print("4", Some(TradeSide::Buy), 101.0, 3.0, 120_001),
            print("5", Some(TradeSide::Sell), 101.0, 0.5, 120_001),
        ];
        let bars = aggregate_trades(&trades, 60_000);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].timestamp, 60_000);
        assert_eq!(bars[0].buy_volume, 2.0);
        assert_eq!(bars[0].sell_volume, 1.0);
        assert_eq!(bars[0].trades, 2);
        assert_eq!(bars[0].total_volume, 3.0);
        assert_eq!(bars[0].price_rows.len(), 1);
        assert!(bars[0].is_closed);
        assert_eq!(bars[0].row_size, 0.0);
        assert!(bars[0].poc.is_none());
        assert!(bars[0].vah.is_none());
        assert!(bars[0].val.is_none());

        assert_eq!(bars[1].timestamp, 120_000);
        assert_eq!(bars[1].buy_volume, 3.0);
        assert_eq!(bars[1].sell_volume, 0.5);
        assert_eq!(bars[1].trades, 2);
        assert_eq!(bars[1].price_rows.len(), 1);
        assert_eq!(bars[1].price_rows[0].buy_count, 1);
        assert_eq!(bars[1].price_rows[0].sell_count, 1);
    }

    #[test]
    fn aggregation_merges_exact_prices_and_sorts_rows() {
        let trades = vec![
            print("1", Some(TradeSide::Buy), 102.0, 1.0, 10),
            print("2", Some(TradeSide::Sell), 100.0, 2.0, 11),
            print("3", Some(TradeSide::Buy), 102.0, 3.0, 12),
            print("4", Some(TradeSide::Buy), 101.0, 4.0, 13),
        ];
        let bars = aggregate_trades(&trades, 60_000);
        assert_eq!(bars.len(), 1);
        let bar = &bars[0];
        assert_eq!(bar.price_rows.len(), 3);
        assert_eq!(bar.price_rows[0].price, 100.0);
        assert_eq!(bar.price_rows[1].price, 101.0);
        assert_eq!(bar.price_rows[2].price, 102.0);
        assert_eq!(bar.price_rows[2].buy_volume, 4.0);
        assert_eq!(bar.price_rows[2].buy_count, 2);
    }

    #[test]
    fn dedupe_uses_exchange_ts_and_trade_id_then_sorts_ascending() {
        let trades = vec![
            print("b", Some(TradeSide::Buy), 100.0, 1.0, 200),
            print("a", Some(TradeSide::Buy), 100.0, 1.0, 100),
            print("b", Some(TradeSide::Buy), 100.0, 1.0, 200),
            print("c", Some(TradeSide::Sell), 100.0, 1.0, 100),
        ];
        let unique = dedupe_trades(trades);
        assert_eq!(unique.len(), 3);
        assert_eq!(unique[0].ts, 100);
        assert_eq!(unique[0].trade_id.as_deref(), Some("a"));
        assert_eq!(unique[1].trade_id.as_deref(), Some("c"));
        assert_eq!(unique[2].ts, 200);
    }

    #[test]
    fn paging_stops_on_cursor_exhaustion_repeat_or_since() {
        let mut page = TradeHistoryPage {
            has_more: true,
            next_cursor: Some("next".into()),
            ..TradeHistoryPage::default()
        };
        page.data
            .push(print("1", Some(TradeSide::Buy), 100.0, 1.0, 500));
        assert!(should_request_next(&page, 100, None));
        assert!(!should_request_next(&page, 100, Some("next")));
        assert!(!should_request_next(&page, 500, None));

        page.has_more = false;
        assert!(!should_request_next(&page, 100, None));
        page.has_more = true;
        page.next_cursor = None;
        assert!(!should_request_next(&page, 100, None));

        page.next_cursor = Some("next".into());
        page.data.clear();
        assert!(!should_request_next(&page, 100, None));
    }

    #[test]
    fn page_metadata_decodes_numeric_cursor_and_string_since() {
        let page = parse_trade_history(serde_json::json!({
            "since": "1700000000000",
            "until": 1_700_000_600_000i64,
            "limit": "20000",
            "has_more": true,
            "next_cursor": 12345,
            "data": []
        }));
        assert_eq!(page.since, 1_700_000_000_000);
        assert_eq!(page.limit, 20_000);
        assert_eq!(page.next_cursor.as_deref(), Some("12345"));
    }

    #[test]
    fn bare_array_and_nested_data_are_accepted() {
        let bare = parse_trade_history(serde_json::json!([
            {"trade_id": "1", "ts": 1_700_000_000_000i64, "price": 1.0, "amount": 2.0, "side": "buy"}
        ]));
        assert_eq!(bare.data.len(), 1);

        let nested = parse_trade_history(serde_json::json!({
            "partial": true,
            "data": {"data": [
                {"trade_id": "2", "ts": 1_700_000_000_000i64, "price": 1.0, "amount": 2.0, "side": "sell"}
            ]}
        }));
        assert!(nested.partial);
        assert_eq!(nested.data.len(), 1);
    }
}
