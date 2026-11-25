/// Historical order-book (L2) and top-of-book (L1) samples.
///
/// CoinWatch exposes the same envelope for both routes:
///
/// ```text
/// GET {rest}/api/v1/orderbook/history?exchange&symbol&since&until&limit&depth&sample_ms&type&module&source
/// GET {rest}/api/v1/ticker/history?...
/// ```
///
/// The backend samples are change-only and use epoch milliseconds, but the
/// decoder also accepts second timestamps and numeric strings so older
/// gateway/legacy payloads keep working.
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Seconds below this magnitude are treated as epoch seconds and multiplied
/// by 1000. Current epoch milliseconds are ~1.7e12, so the cutoff is safely
/// between the two ranges.
const EPOCH_MS_CUTOFF: i64 = 10_000_000_000;

/// One `[price, quantity]` level from a historical book snapshot.
///
/// Wire form is a two-element array: `[p, q]`. Numeric strings are accepted
/// because the upstream source sometimes serialises decimals as strings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BookLevel {
    pub price: f64,
    pub quantity: f64,
}

impl BookLevel {
    pub const fn new(price: f64, quantity: f64) -> Self {
        Self { price, quantity }
    }

    /// Convert into the in-memory order-book level type.
    pub fn to_price_level(self) -> crate::data::orderbook::PriceLevel {
        crate::data::orderbook::PriceLevel::new(self.price, self.quantity)
    }
}

impl Serialize for BookLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.price, self.quantity].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BookLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(pair) = value.as_array() {
            if pair.len() >= 2 {
                let price =
                    number(&pair[0]).ok_or_else(|| D::Error::custom("missing level price"))?;
                let quantity =
                    number(&pair[1]).ok_or_else(|| D::Error::custom("missing level quantity"))?;
                return Ok(Self { price, quantity });
            }
        }
        // Tolerate `{ "price": ..., "quantity": ... }` objects from older
        // local shims.
        if let Some(object) = value.as_object() {
            let price = object
                .get("price")
                .and_then(number)
                .ok_or_else(|| D::Error::custom("missing level price"))?;
            let quantity = object
                .get("quantity")
                .or_else(|| object.get("qty"))
                .and_then(number)
                .ok_or_else(|| D::Error::custom("missing level quantity"))?;
            return Ok(Self { price, quantity });
        }
        Err(D::Error::custom(
            "order-book level must be [price, quantity]",
        ))
    }
}

/// One change-only historical book sample.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct OrderbookSample {
    /// Sample time in epoch milliseconds (seconds are normalised on decode).
    ///
    /// Accepts the canonical `ts` or the realtime-ondemand legacy facade's
    /// `timestamp`; `ts` wins if both are present.
    pub ts: i64,
    /// Exchange timestamp in epoch milliseconds, when provided.
    pub exchange_ts: Option<i64>,
    /// Venue sequence/nonce, when provided.
    pub seq: Option<u64>,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    /// Requested source depth (`20` or `50`).
    pub limit: Option<u32>,
    /// A carried sample at `since` seeds the left edge of the window.
    pub carried: bool,
}

/// Wire form for [`OrderbookSample`] that tolerates both timestamp spellings.
#[derive(Deserialize)]
struct OrderbookSampleWire {
    #[serde(default)]
    ts: Option<serde_json::Value>,
    #[serde(default)]
    timestamp: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "de_opt_epoch_ms")]
    exchange_ts: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_u64")]
    seq: Option<u64>,
    #[serde(default)]
    bids: Vec<BookLevel>,
    #[serde(default)]
    asks: Vec<BookLevel>,
    #[serde(default, deserialize_with = "de_opt_u32")]
    limit: Option<u32>,
    #[serde(default)]
    carried: bool,
}

impl<'de> Deserialize<'de> for OrderbookSample {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = OrderbookSampleWire::deserialize(deserializer)?;
        // Canonical `ts` takes precedence over the legacy `timestamp` alias.
        let ts = wire
            .ts
            .as_ref()
            .and_then(epoch_ms_from_value)
            .or_else(|| wire.timestamp.as_ref().and_then(epoch_ms_from_value))
            .unwrap_or(0);
        Ok(Self {
            ts,
            exchange_ts: wire.exchange_ts,
            seq: wire.seq,
            bids: wire.bids,
            asks: wire.asks,
            limit: wire.limit,
            carried: wire.carried,
        })
    }
}

impl OrderbookSample {
    /// Best bid in this sample (highest bid price).
    pub fn best_bid(&self) -> Option<f64> {
        best_bid_from_levels(&self.bids)
    }

    /// Best ask in this sample (lowest ask price).
    pub fn best_ask(&self) -> Option<f64> {
        best_ask_from_levels(&self.asks)
    }

    pub fn l1_point(&self) -> L1Point {
        L1Point {
            timestamp_ms: self.ts,
            best_bid: self.best_bid(),
            best_ask: self.best_ask(),
        }
    }
}

/// Derived top-of-book point used for L1 step lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct L1Point {
    pub timestamp_ms: i64,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
}

/// CoinWatch historical envelope shared by the L1 and L2 routes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrderbookHistory {
    /// Echo of the instrument, when the backend includes it.
    #[serde(default)]
    pub instrument: Option<serde_json::Value>,
    #[serde(default)]
    pub exchange: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default, deserialize_with = "de_u32")]
    pub depth: u32,
    #[serde(default, deserialize_with = "de_i64")]
    pub sample_ms: i64,
    #[serde(default)]
    pub source: String,
    /// `true` means an asynchronous backfill is still running; poll again.
    #[serde(default)]
    pub queued: bool,
    /// `true` means the exact requested window is not fully covered.
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub data: Vec<OrderbookSample>,
}

impl OrderbookHistory {
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Top-of-book series derived from the L2 samples. No separate ticker
    /// history request is needed by the chart.
    pub fn l1_series(&self) -> Vec<L1Point> {
        self.data.iter().map(OrderbookSample::l1_point).collect()
    }
}

/// Decode a canonical or legacy (`{success, data}`) history payload.
///
/// Unknown metadata fields are ignored. A bare data array, or a payload with
/// the array nested under `data.data`, is also accepted.
pub fn parse_orderbook_history(value: serde_json::Value) -> OrderbookHistory {
    serde_json::from_value(normalize_envelope(value)).unwrap_or_default()
}

fn normalize_envelope(value: serde_json::Value) -> serde_json::Value {
    if value.is_array() {
        return serde_json::json!({ "data": value });
    }
    if !value.is_object() {
        return serde_json::Value::Object(Default::default());
    }
    // Canonical and legacy `{success,data}` envelopes already carry the array
    // at `data`. Only unwrap a nested object.
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
    // Some legacy local shims used `history`.
    if value.get("data").is_none() {
        if let Some(rows) = value
            .get("history")
            .or_else(|| value.get("result"))
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

fn best_bid_from_levels(levels: &[BookLevel]) -> Option<f64> {
    levels
        .iter()
        .map(|level| level.price)
        .filter(|price| price.is_finite() && *price > 0.0)
        .reduce(f64::max)
}

fn best_ask_from_levels(levels: &[BookLevel]) -> Option<f64> {
    levels
        .iter()
        .map(|level| level.price)
        .filter(|price| price.is_finite() && *price > 0.0)
        .reduce(f64::min)
}

fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|v| v as f64))
        .or_else(|| value.as_u64().map(|v| v as f64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        .filter(|v| v.is_finite())
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

fn de_opt_epoch_ms<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(epoch_ms_from_value(&value))
}

fn de_opt_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|v| u64::try_from(v).ok()))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<u64>().ok())))
}

fn de_opt_u32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<u32>().ok())))
}

fn de_u32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    Ok(de_opt_u32(deserializer)?.unwrap_or_default())
}

fn de_i64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|v| i64::try_from(v).ok()))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_envelope_and_tuple_levels() {
        let value = serde_json::json!({
            "instrument": "BTC/USDT",
            "exchange": "binance",
            "symbol": "BTC/USDT",
            "depth": 20,
            "sample_ms": 1000,
            "source": "cryptohftdata",
            "queued": false,
            "partial": true,
            "data": [{
                "ts": 1700000000,
                "exchange_ts": 1700000000123i64,
                "seq": "42",
                "bids": [[100.0, "1.5"], [99.0, 2.0]],
                "asks": [[101.0, 3.0]],
                "limit": "20",
                "carried": true
            }]
        });
        let history = parse_orderbook_history(value);
        assert_eq!(history.exchange, "binance");
        assert_eq!(history.depth, 20);
        assert_eq!(history.sample_ms, 1000);
        assert!(history.partial);
        assert_eq!(history.data.len(), 1);
        let sample = &history.data[0];
        assert_eq!(sample.ts, 1_700_000_000_000); // seconds -> ms
        assert_eq!(sample.exchange_ts, Some(1_700_000_000_123));
        assert_eq!(sample.seq, Some(42));
        assert_eq!(sample.limit, Some(20));
        assert!(sample.carried);
        assert_eq!(sample.bids[0], BookLevel::new(100.0, 1.5));
        assert_eq!(sample.best_bid(), Some(100.0));
        assert_eq!(sample.best_ask(), Some(101.0));
    }

    #[test]
    fn legacy_success_data_envelope_is_accepted() {
        let value = serde_json::json!({
            "success": true,
            "data": [{
                "ts": 1_700_000_000_000i64,
                "bids": [[100.0, 1.0]],
                "asks": [[101.0, 1.0]]
            }]
        });
        let history = parse_orderbook_history(value);
        assert_eq!(history.data.len(), 1);
        assert_eq!(history.data[0].ts, 1_700_000_000_000);
        assert!(!history.data[0].carried);
        assert_eq!(history.data[0].l1_point().best_bid, Some(100.0));
    }

    #[test]
    fn legacy_timestamp_field_survives_and_canonical_ts_wins() {
        let legacy = serde_json::json!({
            "success": true,
            "queued": false,
            "data": [{
                "timestamp": 1_795_565_400,
                "bids": [[100.0, 1.0]],
                "asks": [[101.0, 2.0]]
            }]
        });
        let history = parse_orderbook_history(legacy);
        assert_eq!(history.data.len(), 1);
        assert_eq!(history.data[0].ts, 1_795_565_400_000);
        let l1 = history.l1_series();
        assert_eq!(l1.len(), 1);
        assert_eq!(l1[0].timestamp_ms, 1_795_565_400_000);
        assert_eq!(l1[0].best_bid, Some(100.0));
        assert_eq!(l1[0].best_ask, Some(101.0));

        // Canonical `ts` takes precedence over the legacy alias.
        let both = serde_json::json!({
            "data": [{
                "ts": 1_795_565_400,
                "timestamp": 1_795_565_999,
                "bids": [[10.0, 1.0]],
                "asks": []
            }]
        });
        let history = parse_orderbook_history(both);
        assert_eq!(history.data.len(), 1);
        assert_eq!(history.data[0].ts, 1_795_565_400_000);
    }

    #[test]
    fn bare_array_and_nested_data_are_accepted() {
        let rows = serde_json::json!([
            {"ts": 1_700_000_000_000i64, "bids": [[10.0, 1.0]], "asks": [[11.0, 2.0]]}
        ]);
        assert_eq!(parse_orderbook_history(rows).data.len(), 1);

        let nested = serde_json::json!({
            "queued": true,
            "data": {"data": [{"ts": 1_700_000_000_000i64, "bids": [], "asks": []}]}
        });
        let history = parse_orderbook_history(nested);
        assert!(history.queued);
        assert_eq!(history.data.len(), 1);
    }

    #[test]
    fn l1_series_derives_top_of_book() {
        let history = parse_orderbook_history(serde_json::json!({
            "data": [
                {"ts": 1000, "bids": [[5.0, 1.0], [4.0, 2.0]], "asks": [[7.0, 1.0], [8.0, 2.0]]},
                {"ts": 2000, "bids": [], "asks": [[9.0, 1.0]]}
            ]
        }));
        let l1 = history.l1_series();
        assert_eq!(l1.len(), 2);
        assert_eq!(l1[0].timestamp_ms, 1_000_000);
        assert_eq!(l1[0].best_bid, Some(5.0));
        assert_eq!(l1[0].best_ask, Some(7.0));
        assert_eq!(l1[1].best_bid, None);
        assert_eq!(l1[1].best_ask, Some(9.0));
    }

    #[test]
    fn second_and_millisecond_timestamps_normalise() {
        let history = parse_orderbook_history(serde_json::json!({
            "data": [{"ts": 1_700_000_000}, {"ts": 1_700_000_000_000i64}]
        }));
        assert_eq!(history.data[0].ts, 1_700_000_000_000);
        assert_eq!(history.data[1].ts, 1_700_000_000_000);
    }
}
