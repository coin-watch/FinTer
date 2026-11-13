/// Candle feed — provides OHLCV candlestick data for the chart panel.
///
/// # Architecture
///
/// ```text
/// Bridge (WebSocket → broadcast channel)
///       │
///       │  ohlcv_history  (one-shot REST batch, full history)
///       │  ohlcv          (real-time WebSocket candle updates)
///       ▼
/// CandleFeed::drain_loop  (Tokio background task)
///       │  merge_candles() — upsert by timestamp, maintain sorted order
///       ▼
/// Arc<Mutex<Vec<OhlcvCandle>>>   (sorted oldest → newest, capped at MAX_CANDLES)
///       │
///       ▼
/// ChartPanel::render()  (calls snapshot() on GPUI frame)
/// ```
///
/// The bridge emits two OHLCV message types on the same channel:
/// - `ohlcv_history` — full history batch fetched via REST at startup.
///   Rust treats this identically to a streaming update.
/// - `ohlcv` — real-time candle updates via WebSocket (one or a few candles
///   at a time, typically just the in-progress forming candle).
///
/// Both are merged into the same sorted buffer using upsert-by-timestamp
/// semantics, so the latest price for an in-progress candle is always shown.
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::data::client::tokio_runtime;
use crate::data::models::OhlcvCandle;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of candles to retain in the buffer (oldest are evicted).
pub const MAX_CANDLES: usize = 10_000;

/// Maximum number of missing periods the clock-driven rollover will backfill
/// in one step. Larger gaps skip straight to the current bucket.
const MAX_BACKFILL_PERIODS: i64 = 500;

// ---------------------------------------------------------------------------
// CandleFeedConfig — kept for API compat / ChartPanel construction
// ---------------------------------------------------------------------------

/// Configuration metadata for a `CandleFeed` (used for display only).
#[derive(Debug, Clone)]
pub struct CandleFeedConfig {
    /// Exchange ID (e.g. "binance").
    pub exchange: String,
    /// Trading symbol (e.g. "BTC/USDT").
    pub symbol: String,
    /// Timeframe (e.g. "1m", "5m", "1h", "1d").
    pub timeframe: String,
    /// Desired history limit (informational — actual limit is set on Bridge).
    pub limit: u32,
    /// Legacy field — unused, kept for call-site compat.
    pub poll_interval_ms: u64,
}

impl CandleFeedConfig {
    pub fn new(exchange: &str, symbol: &str, timeframe: &str) -> Self {
        Self {
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            limit: MAX_CANDLES as u32,
            poll_interval_ms: 0,
        }
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    /// No-op — polling interval is no longer used (WebSocket-driven).
    pub fn with_poll_interval(mut self, _ms: u64) -> Self {
        self.poll_interval_ms = 0;
        self
    }
}

// ---------------------------------------------------------------------------
// CandleFeed
// ---------------------------------------------------------------------------

/// Shared, thread-safe buffer of recent OHLCV candles.
///
/// Clone-cheap: the inner `Arc<Mutex<…>>` is reference-counted.
#[derive(Clone)]
pub struct CandleFeed {
    inner: Arc<Mutex<CandleFeedInner>>,
    /// Timeframe this feed was created for (e.g. "1m").
    timeframe: Option<String>,
    /// Keep the background drain task alive.
    _task: Arc<JoinHandle<()>>,
}

pub(crate) struct CandleFeedInner {
    /// Sorted oldest → newest.
    candles: Vec<OhlcvCandle>,
    /// Millisecond timestamp of the last update.
    last_update_ms: i64,
    /// Monotonically increasing data revision.
    version: u64,
    /// Timestamp of candle currently owned/driven by the real-time ticker stream.
    live_ticker_candle_ts: Option<i64>,
    /// Most recent live ticker price observed from tick_rx.
    latest_ticker_price: Option<f64>,
    /// Latest credit exhaustion error, if any.
    credit_error: Option<crate::data::credits::CreditExhaustedInfo>,
    /// Latest network/API fetch error, if any.
    fetch_error: Option<String>,
}

impl CandleFeed {
    /// Create a new `CandleFeed` that consumes OHLCV messages from `rx`.
    ///
    /// `rx` should come from `Bridge::subscribe_ohlcv()`.
    pub fn new(
        rx: broadcast::Receiver<Arc<Vec<OhlcvCandle>>>,
        tick_rx: Option<broadcast::Receiver<Arc<crate::data::tick::UnifiedTick>>>,
        timeframe: Option<String>,
    ) -> Self {
        let inner = Arc::new(Mutex::new(CandleFeedInner {
            candles: Vec::with_capacity(MAX_CANDLES),
            last_update_ms: 0,
            version: 0,
            live_ticker_candle_ts: None,
            latest_ticker_price: None,
            credit_error: None,
            fetch_error: None,
        }));

        let drain_inner = Arc::clone(&inner);
        let tf = timeframe.clone().unwrap_or_else(|| "1m".to_string());

        let task = tokio_runtime().spawn(async move {
            drain_loop(rx, tick_rx, tf, drain_inner).await;
        });

        Self {
            inner,
            timeframe,
            _task: Arc::new(task),
        }
    }

    /// Advance the live candle to the current wall-clock bucket if a period
    /// boundary has been crossed. Returns `true` when candles changed.
    fn rollover_to_now(&self, inner: &mut CandleFeedInner) -> bool {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let tf = self.timeframe.as_deref().unwrap_or("1m").to_string();
        let interval_ms = get_interval_ms(&tf);
        if check_and_form_live_candle(inner, interval_ms, &tf, now_ms) {
            inner.version = inner.version.saturating_add(1);
            inner.last_update_ms = now_ms;
            true
        } else {
            false
        }
    }

    /// Return a point-in-time snapshot of the candles (oldest first).
    ///
    /// Acquires the mutex briefly; safe to call from the GPUI render thread.
    pub fn snapshot(&self) -> Vec<OhlcvCandle> {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        self.rollover_to_now(&mut inner);
        inner.candles.clone()
    }

    /// Merge incoming historical candles into the buffer (upsert by timestamp).
    pub fn merge_history(&self, incoming: &[OhlcvCandle]) {
        if incoming.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        let ticker_ts = inner.live_ticker_candle_ts;
        let latest_price = inner.latest_ticker_price;
        if merge_candles(&mut inner.candles, incoming, ticker_ts, latest_price) {
            inner.version = inner.version.saturating_add(1);
            inner.last_update_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
        }
    }

    pub fn prepend_history(&self, older_candles: &[OhlcvCandle]) {
        let mut inner = self.inner.lock().unwrap();
        let ticker_ts = inner.live_ticker_candle_ts;
        let latest_price = inner.latest_ticker_price;
        if merge_candles(&mut inner.candles, older_candles, ticker_ts, latest_price) {
            inner.version = inner.version.saturating_add(1);
        }
    }

    /// Return a point-in-time snapshot of candles along with the feed version.
    pub fn snapshot_with_version(&self) -> (Vec<OhlcvCandle>, u64) {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        self.rollover_to_now(&mut inner);
        (inner.candles.clone(), inner.version)
    }

    /// Number of candles currently in the buffer.
    pub fn len(&self) -> usize {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        self.rollover_to_now(&mut inner);
        inner.candles.len()
    }

    /// Candles currently in the chart viewport, in the same order and with the
    /// same range as the chart's own `ChartState::visible_range` slice.
    ///
    /// Used to map a mouse position to the candle occupying that slot.
    pub fn visible_slice(&self, state: &gpui_component::chart::ChartState) -> Vec<OhlcvCandle> {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        self.rollover_to_now(&mut inner);
        let range = state.visible_range(inner.candles.len());
        inner.candles[range].to_vec()
    }

    /// True when no candles have been received yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the last update timestamp in milliseconds.
    pub fn last_update_ms(&self) -> i64 {
        self.inner
            .lock()
            .expect("CandleFeed mutex poisoned")
            .last_update_ms
    }

    /// Monotonic candle data revision, incremented only when contents change.
    pub fn version(&self) -> u64 {
        self.inner
            .lock()
            .expect("CandleFeed mutex poisoned")
            .version
    }

    /// Record a credit exhaustion error for this feed.
    pub fn set_credit_error(&self, error: crate::data::credits::CreditExhaustedInfo) {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        inner.credit_error = Some(error);
        inner.version = inner.version.saturating_add(1);
    }

    /// Clear any recorded credit exhaustion error.
    pub fn clear_credit_error(&self) {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        if inner.credit_error.is_some() {
            inner.credit_error = None;
            inner.version = inner.version.saturating_add(1);
        }
    }

    /// Retrieve the current credit exhaustion error, if any.
    pub fn credit_error(&self) -> Option<crate::data::credits::CreditExhaustedInfo> {
        self.inner
            .lock()
            .expect("CandleFeed mutex poisoned")
            .credit_error
            .clone()
    }

    /// Set a fetch/network error message.
    pub fn set_error(&self, error: String) {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        inner.fetch_error = Some(error);
        inner.version = inner.version.saturating_add(1);
    }

    /// Clear any recorded fetch error.
    pub fn clear_error(&self) {
        let mut inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        if inner.fetch_error.is_some() {
            inner.fetch_error = None;
            inner.version = inner.version.saturating_add(1);
        }
    }

    /// Returns the last credit or fetch error message if any, or `None`.
    pub fn last_error(&self) -> Option<String> {
        let inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        inner.credit_error.as_ref().map(|e| e.message.clone())
            .or_else(|| inner.fetch_error.clone())
    }
}

// ---------------------------------------------------------------------------
// Drain loop (Tokio background task)
// ---------------------------------------------------------------------------

fn get_interval_ms(timeframe: &str) -> i64 {
    match timeframe {
        "1s" => 1_000,
        "1m" => 60_000,
        "3m" => 180_000,
        "5m" => 300_000,
        "15m" => 900_000,
        "30m" => 1_800_000,
        "1h" => 3_600_000,
        "2h" => 7_200_000,
        "4h" => 14_400_000,
        "6h" => 21_600_000,
        "8h" => 28_800_000,
        "12h" => 43_200_000,
        "1d" => 86_400_000,
        "1w" => 604_800_000,
        "1M" => 2_592_000_000,
        _ => 60_000,
    }
}

fn align_timestamp(ts: i64, interval_ms: i64, timeframe: &str) -> i64 {
    if interval_ms <= 0 {
        return ts;
    }
    match timeframe {
        "1w" => {
            // Unix epoch 1970-01-01 was a Thursday.
            // Crypto weekly candles (Binance, OKX, Bybit, etc.) start on Monday 00:00:00 UTC.
            // Monday Jan 5 1970 00:00:00 UTC is 4 days = 345_600_000 ms after epoch.
            const MONDAY_OFFSET: i64 = 4 * 86_400_000;
            ((ts - MONDAY_OFFSET).div_euclid(interval_ms)) * interval_ms + MONDAY_OFFSET
        }
        "1M" => {
            // Calendar month: align to 1st of the month at 00:00:00 UTC
            if let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp(ts / 1000, 0) {
                use chrono::Datelike;
                if let Some(date) = chrono::NaiveDate::from_ymd_opt(dt.year(), dt.month(), 1) {
                    if let Some(dt_start) = date.and_hms_opt(0, 0, 0) {
                        return dt_start.and_utc().timestamp_millis();
                    }
                }
            }
            (ts / interval_ms) * interval_ms
        }
        _ => (ts / interval_ms) * interval_ms,
    }
}

/// Form and append any live candles whose period boundary has been crossed
/// between the last buffered candle and `now_ms`.
///
/// This is the clock-driven counterpart to the ticker/kline driven rollover:
/// when the market is quiet, no WebSocket messages arrive, so the chart would
/// otherwise stay stuck on the previous candle while the countdown reaches 0.
///
/// Returns `true` when at least one candle was appended.
pub(crate) fn check_and_form_live_candle(
    inner: &mut CandleFeedInner,
    interval_ms: i64,
    timeframe: &str,
    now_ms: i64,
) -> bool {
    if interval_ms <= 0 || inner.candles.is_empty() {
        return false;
    }

    let last = match inner.candles.last() {
        Some(c) => c.clone(),
        None => return false,
    };

    let next_candle_ts = if timeframe == "1M" {
        align_timestamp(last.timestamp + 32 * 86_400_000, interval_ms, timeframe)
    } else {
        last.timestamp + interval_ms
    };

    if now_ms < next_candle_ts {
        return false;
    }

    // Guard against wildly stale data (e.g. synthetic/epoch timestamps or a
    // feed that has been idle for a very long time): only bridge gaps of up to
    // a bounded number of periods, otherwise skip forward to the current
    // bucket instead of appending thousands of empty candles.
    let current_bucket = align_timestamp(now_ms, interval_ms, timeframe);
    let mut next_ts = next_candle_ts;
    if interval_ms > 0 {
        let periods_behind = (current_bucket.saturating_sub(next_candle_ts)) / interval_ms;
        if periods_behind > MAX_BACKFILL_PERIODS {
            next_ts = current_bucket;
        }
    }
    let mut changed = false;
    let mut prev_close = last.close;
    let price = inner.latest_ticker_price.unwrap_or(prev_close);

    while next_ts <= current_bucket {
        let is_current = next_ts == current_bucket;
        let c_price = if is_current { price } else { prev_close };
        let new_candle = OhlcvCandle {
            timestamp: next_ts,
            open: prev_close,
            high: prev_close.max(c_price),
            low: prev_close.min(c_price),
            close: c_price,
            volume: 0.0,
        };
        inner.candles.push(new_candle);
        prev_close = c_price;
        changed = true;

        if timeframe == "1M" {
            next_ts = align_timestamp(next_ts + 32 * 86_400_000, interval_ms, timeframe);
        } else {
            next_ts += interval_ms;
        }

        if inner.candles.len() > MAX_CANDLES + 50 {
            break;
        }
    }

    if changed {
        if inner.candles.len() > MAX_CANDLES {
            let excess = inner.candles.len() - MAX_CANDLES;
            inner.candles.drain(0..excess);
        }
        inner.live_ticker_candle_ts = Some(current_bucket);
    }

    changed
}

async fn drain_loop(
    mut rx: broadcast::Receiver<Arc<Vec<OhlcvCandle>>>,
    mut tick_rx: Option<broadcast::Receiver<Arc<crate::data::tick::UnifiedTick>>>,
    timeframe: String,
    buffer: Arc<Mutex<CandleFeedInner>>,
) {
    info!("CandleFeed: drain loop started, waiting for OHLCV and ticker messages");
    let interval_ms = get_interval_ms(&timeframe);

    // Periodic wall-clock tick so the live candle rolls over even when the
    // market is quiet and neither klines nor ticker ticks arrive.
    let mut clock_interval = tokio::time::interval(std::time::Duration::from_millis(250));
    clock_interval.tick().await;

    loop {
        tokio::select! {
            _ = clock_interval.tick() => {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let mut inner = buffer.lock().expect("CandleFeed mutex poisoned");
                if check_and_form_live_candle(&mut inner, interval_ms, &timeframe, now_ms) {
                    inner.version = inner.version.saturating_add(1);
                    inner.last_update_ms = now_ms;
                }
            }
            batch_res = rx.recv() => {
                match batch_res {
                    Ok(batch) => {
                        if batch.is_empty() {
                            continue;
                        }

                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);

                        let mut inner = buffer.lock().expect("CandleFeed mutex poisoned");
                        let ticker_ts = inner.live_ticker_candle_ts;
                        let latest_price = inner.latest_ticker_price;
                        if merge_candles(&mut inner.candles, &batch, ticker_ts, latest_price) {
                            inner.version = inner.version.saturating_add(1);
                            inner.last_update_ms = now_ms;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("CandleFeed: lagged by {n} messages — some candle updates dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("CandleFeed: channel closed, drain loop exiting");
                        break;
                    }
                }
            }
            tick_res = async {
                match &mut tick_rx {
                    Some(r) => r.recv().await,
                    None => futures_util::future::pending().await,
                }
            } => {
                if let Ok(tick) = tick_res {
                    let price = tick.price;
                    if price > 0.0 && price.is_finite() {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        let tick_ms = tick.timestamp_ms.max(now_ms);
                        let mut inner = buffer.lock().expect("CandleFeed mutex poisoned");
                        inner.latest_ticker_price = Some(price);

                        let mut changed =
                            check_and_form_live_candle(&mut inner, interval_ms, &timeframe, tick_ms);

                        if inner.candles.is_empty() {
                            let current_bucket_ts = align_timestamp(tick_ms, interval_ms, &timeframe);
                            let new_candle = OhlcvCandle {
                                timestamp: current_bucket_ts,
                                open: price,
                                high: price,
                                low: price,
                                close: price,
                                volume: 0.0,
                            };
                            inner.candles.push(new_candle);
                            inner.live_ticker_candle_ts = Some(current_bucket_ts);
                            changed = true;
                        } else if let Some(last) = inner.candles.last_mut() {
                            let high = last.high.max(price);
                            let low = last.low.min(price);
                            if last.close != price || last.high != high || last.low != low {
                                last.close = price;
                                last.high = high;
                                last.low = low;
                                changed = true;
                            }
                            inner.live_ticker_candle_ts = Some(last.timestamp);
                        }

                        if changed {
                            inner.version = inner.version.saturating_add(1);
                            inner.last_update_ms = tick_ms;
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Merge helper — upsert-by-timestamp
// ---------------------------------------------------------------------------

/// Merge `incoming` candles into `buf`, which is kept sorted by timestamp.
///
/// For each incoming candle:
/// - If a candle with the same timestamp exists, overwrite it (live update).
/// - Otherwise insert in sorted order.
///
/// For the latest forming candle, preserves real-time ticker close price and
/// incorporates incoming volume and high/low ranges.
///
/// After merging, evict the oldest candles if the buffer exceeds `MAX_CANDLES`.
fn merge_candles(
    buf: &mut Vec<OhlcvCandle>,
    incoming: &[OhlcvCandle],
    live_ticker_candle_ts: Option<i64>,
    latest_ticker_price: Option<f64>,
) -> bool {
    let before = buf.clone();
    for candle in incoming {
        let is_incoming_latest = buf.last().map(|last| candle.timestamp >= last.timestamp).unwrap_or(true);

        if let Some(live_ts) = live_ticker_candle_ts {
            if candle.timestamp == live_ts {
                if let Ok(idx) = buf.binary_search_by_key(&candle.timestamp, |c| c.timestamp) {
                    let live_c = &mut buf[idx];
                    live_c.high = live_c.high.max(candle.high);
                    live_c.low = live_c.low.min(candle.low);
                    if let Some(tp) = latest_ticker_price {
                        live_c.close = tp;
                        live_c.high = live_c.high.max(tp);
                        live_c.low = live_c.low.min(tp);
                    }
                    if candle.volume.is_finite() && candle.volume >= 0.0 {
                        live_c.volume = candle.volume;
                    }
                    continue;
                }
            }
        }

        match buf.binary_search_by_key(&candle.timestamp, |c| c.timestamp) {
            Ok(idx) => {
                let is_last_idx = idx + 1 == buf.len();
                if is_last_idx && latest_ticker_price.is_some() {
                    let tp = latest_ticker_price.unwrap();
                    let live_c = &mut buf[idx];
                    live_c.high = live_c.high.max(candle.high).max(tp);
                    live_c.low = live_c.low.min(candle.low).min(tp);
                    live_c.close = tp;
                    if candle.volume.is_finite() && candle.volume >= 0.0 {
                        live_c.volume = candle.volume;
                    }
                } else if buf[idx] != *candle {
                    buf[idx] = candle.clone();
                }
            }
            Err(idx) => {
                let mut c = candle.clone();
                if is_incoming_latest && idx == buf.len() {
                    if let Some(tp) = latest_ticker_price {
                        c.high = c.high.max(tp);
                        c.low = c.low.min(tp);
                        c.close = tp;
                    }
                }
                buf.insert(idx, c);
            }
        }
    }

    // Evict oldest candles if over the limit.
    if buf.len() > MAX_CANDLES {
        let excess = buf.len() - MAX_CANDLES;
        buf.drain(0..excess);
    }
    *buf != before
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn candle(ts: i64, close: f64) -> OhlcvCandle {
        OhlcvCandle {
            timestamp: ts,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn config_builder_works() {
        let config = CandleFeedConfig::new("binance", "BTC/USDT", "1m")
            .with_limit(100)
            .with_poll_interval(10_000);

        assert_eq!(config.exchange, "binance");
        assert_eq!(config.symbol, "BTC/USDT");
        assert_eq!(config.timeframe, "1m");
        assert_eq!(config.limit, 100);
        // poll_interval_ms is always 0 (no-op)
        assert_eq!(config.poll_interval_ms, 0);
    }

    #[test]
    fn ohlcv_candle_struct() {
        let c = candle(1700000000000, 105.0);
        assert_eq!(c.timestamp, 1700000000000);
        assert!(c.close >= c.open - f64::EPSILON);
    }

    #[test]
    fn merge_inserts_sorted() {
        let mut buf = vec![candle(1000, 1.0), candle(3000, 3.0)];
        merge_candles(&mut buf, &[candle(2000, 2.0)], None, None);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0].timestamp, 1000);
        assert_eq!(buf[1].timestamp, 2000);
        assert_eq!(buf[2].timestamp, 3000);
    }

    #[test]
    fn merge_upserts_existing() {
        let mut buf = vec![candle(1000, 1.0), candle(2000, 2.0)];
        // Update the forming candle at ts=2000 with a new close price.
        merge_candles(&mut buf, &[candle(2000, 2.5)], None, None);
        assert_eq!(buf.len(), 2);
        assert!((buf[1].close - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn merge_evicts_oldest_when_over_limit() {
        let mut buf: Vec<OhlcvCandle> = (0..MAX_CANDLES as i64)
            .map(|i| candle(i * 1000, i as f64))
            .collect();
        // Add one more — should evict the oldest.
        merge_candles(&mut buf, &[candle(MAX_CANDLES as i64 * 1000, 999.0)], None, None);
        assert_eq!(buf.len(), MAX_CANDLES);
        // Oldest (ts=0) should be gone.
        assert_eq!(buf[0].timestamp, 1000);
    }

    #[test]
    fn merge_protects_live_ticker_candle() {
        let mut buf = vec![candle(1000, 1.0), candle(2000, 2.8)];
        buf[1].high = 3.0;
        buf[1].low = 2.0;
        buf[1].volume = 10.0;

        // Lagging kline snapshot arrives for ts=2000 with an older price
        let lagging_kline = OhlcvCandle {
            timestamp: 2000,
            open: 2.0,
            high: 2.5,
            low: 1.9,
            close: 2.2, // older close
            volume: 15.0, // newer/larger volume
        };

        // If ts=2000 is marked as live_ticker_candle_ts or latest_ticker_price is Some(2.8),
        // live price is not overwritten, but high/low bounds expand and volume updates.
        merge_candles(&mut buf, &[lagging_kline], Some(2000), Some(2.8));
        assert_eq!(buf.len(), 2);
        assert!((buf[1].close - 2.8).abs() < f64::EPSILON); // preserved!
        assert!((buf[1].high - 3.0).abs() < f64::EPSILON);
        assert!((buf[1].low - 1.9).abs() < f64::EPSILON); // expanded low
        assert!((buf[1].volume - 15.0).abs() < f64::EPSILON); // updated volume
    }

    #[test]
    fn candle_feed_version_increments_only_when_data_changes() {
        let (_tx, rx) = broadcast::channel(16);
        let feed = CandleFeed::new(rx, None, Some("1m".to_string()));
        assert_eq!(feed.version(), 0);

        // Prepend new candle -> version increments
        feed.prepend_history(&[candle(1000, 10.0)]);
        assert_eq!(feed.version(), 1);

        // Prepend identical candle -> version does not increment
        feed.prepend_history(&[candle(1000, 10.0)]);
        assert_eq!(feed.version(), 1);

        // Prepend modified candle -> version increments
        feed.prepend_history(&[candle(1000, 12.0)]);
        assert_eq!(feed.version(), 2);

        // Prepend another candle -> version increments
        feed.prepend_history(&[candle(500, 8.0)]);
        assert_eq!(feed.version(), 3);
    }

    #[test]
    fn test_align_timestamp_weekly_monday() {
        // Test Monday-alignment: 1970-01-05 00:00:00 UTC was Monday (ts = 345_600_000)
        let interval_ms = get_interval_ms("1w");
        assert_eq!(interval_ms, 604_800_000);

        // Monday 2026-10-12 00:00:00 UTC = 1791763200000
        let mon = 1791763200000;
        assert_eq!(align_timestamp(mon, interval_ms, "1w"), mon);

        // Wednesday 2026-10-14 08:24:12 UTC = 1791935853249
        let wed = 1791935853249;
        assert_eq!(align_timestamp(wed, interval_ms, "1w"), mon);

        // Sunday 2026-10-18 23:59:59 UTC = mon + 604_800_000 - 1000
        let sun = mon + 604_800_000 - 1000;
        assert_eq!(align_timestamp(sun, interval_ms, "1w"), mon);

        // Next Monday 2026-10-19 00:00:00 UTC = mon + 604_800_000
        let next_mon = mon + 604_800_000;
        assert_eq!(align_timestamp(next_mon, interval_ms, "1w"), next_mon);
    }

    #[test]
    fn test_align_timestamp_monthly() {
        let interval_ms = get_interval_ms("1M");
        // 2026-10-14 08:24:12 UTC
        let mid_oct = 1791935853249;
        let oct_1 = align_timestamp(mid_oct, interval_ms, "1M");
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(oct_1 / 1000, 0).unwrap();
        use chrono::Datelike;
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 10);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_merge_updates_live_candle_volume_from_kline() {
        let mut buf = vec![candle(1000, 1.0), candle(2000, 2.8)];
        buf[1].high = 3.0;
        buf[1].low = 2.0;
        buf[1].volume = 0.0; // new live candle formed with 0 volume

        // Incoming kline update arrives with actual candle volume
        let kline = OhlcvCandle {
            timestamp: 2000,
            open: 2.0,
            high: 2.9,
            low: 2.1,
            close: 2.5,
            volume: 12.5,
        };

        merge_candles(&mut buf, &[kline], Some(2000), Some(2.8));
        assert_eq!(buf.len(), 2);
        // Preserves live ticker close and high
        assert!((buf[1].close - 2.8).abs() < f64::EPSILON);
        assert!((buf[1].high - 3.0).abs() < f64::EPSILON);
        // Backfills true volume from kline
        assert!((buf[1].volume - 12.5).abs() < f64::EPSILON);
    }

    /// Current wall-clock time in ms. Tests that go through `CandleFeed`'s
    /// public API must anchor their timestamps near "now", otherwise the
    /// clock-driven rollover would (correctly) fill the gap to the present.
    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[tokio::test]
    async fn test_drain_loop_live_candle_volume_not_polluted_by_ticker() {
        let (rx_tx, rx) = broadcast::channel(16);
        let (tick_tx, tick_rx) = broadcast::channel(16);

        let feed = CandleFeed::new(rx, Some(tick_rx), Some("1m".to_string()));

        // Anchor on the current minute bucket: the previous candle is `t0`,
        // the forming one is `t1`.
        let t0 = align_timestamp(now_ms(), 60_000, "1m") - 60_000;
        let t1 = t0 + 60_000;

        // Historical candle at ts = t0
        rx_tx
            .send(Arc::new(vec![OhlcvCandle {
                timestamp: t0,
                open: 100.0,
                high: 105.0,
                low: 95.0,
                close: 102.0,
                volume: 8.5,
            }]))
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Ticker tick arrives at the start of the next minute (ts = t1)
        let tick = Arc::new(crate::data::tick::UnifiedTick::new(
            "binance",
            "BTC/USDT",
            104.0,
            99999.0, // test that tick.amount is NOT used as candle volume!
            crate::data::tick::TradeSide::Unknown,
            t1 + 5_000,
        ));
        tick_tx.send(tick).unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let snap = feed.snapshot();
        assert_eq!(snap.len(), 2);
        let live_candle = snap.last().unwrap();
        assert_eq!(live_candle.timestamp, t1);
        assert_eq!(live_candle.close, 104.0);
        // Must be 0.0, NOT 99999.0!
        assert_eq!(live_candle.volume, 0.0);

        // Now exchange kline arrives for t1 with true volume 3.4
        rx_tx
            .send(Arc::new(vec![OhlcvCandle {
                timestamp: t1,
                open: 102.0,
                high: 104.5,
                low: 101.5,
                close: 103.5,
                volume: 3.4,
            }]))
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let snap2 = feed.snapshot();
        let live_candle2 = snap2.last().unwrap();
        assert_eq!(live_candle2.timestamp, t1);
        // Live close remains from fast ticker
        assert_eq!(live_candle2.close, 104.0);
        // High is max(104.0, 104.5) = 104.5
        assert_eq!(live_candle2.high, 104.5);
        // True volume from kline is backfilled
        assert!((live_candle2.volume - 3.4).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_drain_loop_ticker_price_aligns_and_creates_new_period() {
        let (rx_tx, rx) = broadcast::channel(16);
        let (tick_tx, tick_rx) = broadcast::channel(16);

        let feed = CandleFeed::new(rx, Some(tick_rx), Some("1m".to_string()));

        // Anchor on the current minute bucket, two candles back.
        let t0 = align_timestamp(now_ms(), 60_000, "1m") - 120_000;
        let t1 = t0 + 60_000;

        // Initial historical candle at ts = t0
        rx_tx
            .send(Arc::new(vec![OhlcvCandle {
                timestamp: t0,
                open: 100.0,
                high: 105.0,
                low: 95.0,
                close: 102.0,
                volume: 8.5,
            }]))
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Ticker tick arrives within current candle duration (ts = t0 + 20s)
        let tick1 = Arc::new(crate::data::tick::UnifiedTick::new(
            "binance",
            "BTC/USDT",
            105.0,
            0.0,
            crate::data::tick::TradeSide::Unknown,
            t0 + 20_000,
        ));
        tick_tx.send(tick1).unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let snap1 = feed.snapshot();
        // t0 is two buckets behind the wall clock, so the clock rollover has
        // already backfilled the current bucket alongside it. The ticker price
        // is applied to the *forming* candle, so t0 keeps its historical close.
        assert_eq!(snap1.len(), 3);
        let c0 = snap1.iter().find(|c| c.timestamp == t0).unwrap();
        assert_eq!(c0.close, 102.0);
        let live = snap1.last().unwrap();
        assert_eq!(live.timestamp, align_timestamp(now_ms(), 60_000, "1m"));
        assert_eq!(live.close, 105.0);

        // Next ticker tick crosses into the next 1m period (ts = t1 + 5s)
        let tick2 = Arc::new(crate::data::tick::UnifiedTick::new(
            "binance",
            "BTC/USDT",
            106.5,
            0.0,
            crate::data::tick::TradeSide::Unknown,
            t1 + 5_000,
        ));
        tick_tx.send(tick2).unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let snap2 = feed.snapshot();
        // The ticker price always applies to the *forming* (current) candle.
        let live2 = snap2.last().unwrap();
        assert_eq!(live2.timestamp, align_timestamp(now_ms(), 60_000, "1m"));
        assert_eq!(live2.close, 106.5);

        // Delayed exchange kline arrives for the current bucket with a lagging
        // close (104.0); the live ticker close must win.
        let cur = live2.timestamp;
        rx_tx
            .send(Arc::new(vec![OhlcvCandle {
                timestamp: cur,
                open: 105.0,
                high: 107.0,
                low: 103.0,
                close: 104.0,
                volume: 15.0,
            }]))
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // `snapshot()` may have rolled over to the current bucket by now; the
        // current candle must still carry the live ticker close, not the lagging kline.
        let snap3 = feed.snapshot();
        let c1 = snap3
            .iter()
            .find(|c| c.timestamp == cur)
            .expect("current candle missing");
        assert_eq!(c1.close, 106.5);
        assert_eq!(c1.high, 107.0);
        assert_eq!(c1.volume, 15.0);
    }

    // -----------------------------------------------------------------------
    // Clock-driven live candle formation
    // -----------------------------------------------------------------------

    fn inner_with(candles: Vec<OhlcvCandle>, price: Option<f64>) -> CandleFeedInner {
        CandleFeedInner {
            candles,
            last_update_ms: 0,
            version: 0,
            live_ticker_candle_ts: None,
            latest_ticker_price: price,
            credit_error: None,
            fetch_error: None,
        }
    }

    #[test]
    fn check_and_form_does_nothing_before_boundary() {
        let mut inner = inner_with(vec![candle(60_000, 100.0)], Some(101.0));
        let changed = check_and_form_live_candle(&mut inner, 60_000, "1m", 119_999);
        assert!(!changed);
        assert_eq!(inner.candles.len(), 1);
        assert_eq!(inner.live_ticker_candle_ts, None);
    }

    #[test]
    fn check_and_form_creates_candle_at_boundary() {
        let mut inner = inner_with(vec![candle(60_000, 100.0)], Some(101.5));
        let changed = check_and_form_live_candle(&mut inner, 60_000, "1m", 120_000);
        assert!(changed);
        assert_eq!(inner.candles.len(), 2);
        let live = inner.candles.last().unwrap();
        assert_eq!(live.timestamp, 120_000);
        assert_eq!(live.open, 100.0);
        assert_eq!(live.close, 101.5);
        assert_eq!(live.high, 101.5);
        assert_eq!(live.low, 100.0);
        assert_eq!(live.volume, 0.0);
        assert_eq!(inner.live_ticker_candle_ts, Some(120_000));
    }

    #[test]
    fn check_and_form_is_idempotent_within_same_bucket() {
        let mut inner = inner_with(vec![candle(60_000, 100.0)], Some(101.0));
        assert!(check_and_form_live_candle(&mut inner, 60_000, "1m", 150_000));
        assert!(!check_and_form_live_candle(&mut inner, 60_000, "1m", 150_500));
        assert_eq!(inner.candles.len(), 2);
    }

    #[test]
    fn check_and_form_backfills_skipped_periods() {
        let mut inner = inner_with(vec![candle(60_000, 100.0)], None);
        // Jump 3 periods ahead: 120k, 180k, 240k should be created.
        let changed = check_and_form_live_candle(&mut inner, 60_000, "1m", 250_000);
        assert!(changed);
        let ts: Vec<i64> = inner.candles.iter().map(|c| c.timestamp).collect();
        assert_eq!(ts, vec![60_000, 120_000, 180_000, 240_000]);
        // Without a ticker price, flat candles carry the previous close.
        for c in &inner.candles[1..] {
            assert_eq!(c.open, 100.0);
            assert_eq!(c.close, 100.0);
        }
    }

    #[test]
    fn check_and_form_ignores_empty_buffer_and_bad_interval() {
        let mut empty = inner_with(Vec::new(), Some(1.0));
        assert!(!check_and_form_live_candle(&mut empty, 60_000, "1m", i64::MAX));

        let mut inner = inner_with(vec![candle(60_000, 100.0)], Some(1.0));
        assert!(!check_and_form_live_candle(&mut inner, 0, "1m", i64::MAX));
        assert_eq!(inner.candles.len(), 1);
    }

    #[test]
    fn check_and_form_evicts_to_max_candles() {
        let mut candles: Vec<OhlcvCandle> = (0..MAX_CANDLES as i64)
            .map(|i| candle(i * 60_000, 100.0))
            .collect();
        let last_ts = candles.last().unwrap().timestamp;
        let mut inner = inner_with(std::mem::take(&mut candles), Some(100.0));
        let changed = check_and_form_live_candle(&mut inner, 60_000, "1m", last_ts + 60_000);
        assert!(changed);
        assert_eq!(inner.candles.len(), MAX_CANDLES);
        assert_eq!(inner.candles.last().unwrap().timestamp, last_ts + 60_000);
    }

    #[test]
    fn check_and_form_monthly_advances_calendar_month() {
        let interval_ms = get_interval_ms("1M");
        // 2026-10-01 00:00:00 UTC
        let oct_1 = chrono::DateTime::<chrono::Utc>::from_timestamp(
            chrono::NaiveDate::from_ymd_opt(2026, 10, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp(),
            0,
        )
        .unwrap()
        .timestamp_millis();
        let mut inner = inner_with(vec![candle(oct_1, 50.0)], Some(55.0));

        // Mid-October: still inside the October candle.
        assert!(!check_and_form_live_candle(
            &mut inner,
            interval_ms,
            "1M",
            oct_1 + 10 * 86_400_000
        ));
        assert_eq!(inner.candles.len(), 1);

        // November 3rd: the November candle must be formed.
        let changed =
            check_and_form_live_candle(&mut inner, interval_ms, "1M", oct_1 + 33 * 86_400_000);
        assert!(changed);
        assert_eq!(inner.candles.len(), 2);
        let live = inner.candles.last().unwrap();
        assert_eq!(live.timestamp, align_timestamp(oct_1 + 33 * 86_400_000, interval_ms, "1M"));
        assert_eq!(live.open, 50.0);
        assert_eq!(live.close, 55.0);
    }

    #[tokio::test]
    async fn test_snapshot_forms_live_candle_on_clock_rollover() {
        let (_tx, rx) = broadcast::channel(16);
        let feed = CandleFeed::new(rx, None, Some("1m".to_string()));

        // Seed a candle that is already several periods in the past so the
        // wall clock is guaranteed to be past its next boundary.
        let now = now_ms();
        let stale_ts = align_timestamp(now, 60_000, "1m") - 5 * 60_000;
        feed.merge_history(&[OhlcvCandle {
            timestamp: stale_ts,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.5,
            volume: 1.0,
        }]);
        // `len()` itself performs the clock rollover, so the 5 missing periods
        // are already backfilled by the time we read it.
        assert_eq!(feed.len(), 6);
        let version_before = feed.version();

        // No WebSocket traffic at all — snapshot alone must roll the candle over.
        let snap = feed.snapshot();
        assert!(snap.len() >= 2, "expected live candle to be formed");
        let live = snap.last().unwrap();
        assert_eq!(live.timestamp, align_timestamp(now_ms(), 60_000, "1m"));
        assert_eq!(live.open, 100.5);
        assert_eq!(live.close, 100.5);
        assert_eq!(live.volume, 0.0);
        // The rollover already happened during `len()`, so reading the snapshot
        // again in the same bucket is a no-op and must not bump the version.
        assert_eq!(feed.version(), version_before);

        // Repeated snapshots in the same bucket must not add more candles.
        let len_after = feed.snapshot().len();
        assert_eq!(feed.snapshot().len(), len_after);
    }

    #[test]
    fn merge_history_upserts_and_bumps_version() {
        let (_tx, rx) = broadcast::channel(16);
        let feed = CandleFeed::new(rx, None, Some("1m".to_string()));
        assert_eq!(feed.version(), 0);

        // Anchor near "now" so the clock rollover does not add extra candles.
        let base = align_timestamp(now_ms(), 60_000, "1m");
        let (t0, t1) = (base - 60_000, base);

        feed.merge_history(&[candle(t0, 10.0), candle(t1, 11.0)]);
        assert_eq!(feed.len(), 2);
        assert_eq!(feed.version(), 1);

        // Same values → no version bump.
        feed.merge_history(&[candle(t0, 10.0), candle(t1, 11.0)]);
        assert_eq!(feed.version(), 1);

        // Changed close → version bump.
        feed.merge_history(&[candle(t1, 12.0)]);
        assert_eq!(feed.version(), 2);
        assert_eq!(feed.snapshot().last().unwrap().close, 12.0);
    }
}
