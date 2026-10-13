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
    /// Keep the background drain task alive.
    _task: Arc<JoinHandle<()>>,
}

struct CandleFeedInner {
    /// Sorted oldest → newest.
    candles: Vec<OhlcvCandle>,
    /// Millisecond timestamp of the last update.
    last_update_ms: i64,
    /// Monotonically increasing data revision.
    version: u64,
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
        }));

        let drain_inner = Arc::clone(&inner);
        let tf = timeframe.unwrap_or_else(|| "1m".to_string());

        let task = tokio_runtime().spawn(async move {
            drain_loop(rx, tick_rx, tf, drain_inner).await;
        });

        Self {
            inner,
            _task: Arc::new(task),
        }
    }

    /// Return a point-in-time snapshot of the candles (oldest first).
    ///
    /// Acquires the mutex briefly; safe to call from the GPUI render thread.
    
    pub fn prepend_history(&self, older_candles: &[OhlcvCandle]) {
        let mut inner = self.inner.lock().unwrap();
        if merge_candles(&mut inner.candles, older_candles) {
            inner.version = inner.version.saturating_add(1);
        }
    }

    pub fn snapshot(&self) -> Vec<OhlcvCandle> {
        self.inner
            .lock()
            .expect("CandleFeed mutex poisoned")
            .candles
            .clone()
    }

    /// Return a point-in-time snapshot of candles along with the feed version.
    pub fn snapshot_with_version(&self) -> (Vec<OhlcvCandle>, u64) {
        let inner = self.inner.lock().expect("CandleFeed mutex poisoned");
        (inner.candles.clone(), inner.version)
    }

    /// Number of candles currently in the buffer.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("CandleFeed mutex poisoned")
            .candles
            .len()
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

    /// Always returns `None` — errors are now logged at the bridge level.
    /// Kept for call-site compat with the old polling-based implementation.
    pub fn last_error(&self) -> Option<String> {
        None
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

fn align_timestamp(ts: i64, interval_ms: i64) -> i64 {
    if interval_ms <= 0 {
        return ts;
    }
    (ts / interval_ms) * interval_ms
}

async fn drain_loop(
    mut rx: broadcast::Receiver<Arc<Vec<OhlcvCandle>>>,
    mut tick_rx: Option<broadcast::Receiver<Arc<crate::data::tick::UnifiedTick>>>,
    timeframe: String,
    buffer: Arc<Mutex<CandleFeedInner>>,
) {
    info!("CandleFeed: drain loop started, waiting for OHLCV and ticker messages");
    let interval_ms = get_interval_ms(&timeframe);

    loop {
        tokio::select! {
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
                        if merge_candles(&mut inner.candles, &batch) {
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
                        let mut inner = buffer.lock().expect("CandleFeed mutex poisoned");
                        
                        // Use tick's timestamp instead of local wall clock!
                        let tick_ms = tick.timestamp_ms;
                        let current_bucket_ts = align_timestamp(tick_ms, interval_ms);

                        let mut changed = false;
                        if let Some(last) = inner.candles.last_mut() {
                            if last.timestamp == current_bucket_ts {
                                let high = last.high.max(price);
                                let low = last.low.min(price);
                                changed = last.close != price || last.high != high || last.low != low;
                                if changed {
                                    last.close = price;
                                    last.high = high;
                                    last.low = low;
                                }
                            } else if current_bucket_ts > last.timestamp {
                                let new_candle = OhlcvCandle {
                                    timestamp: current_bucket_ts,
                                    open: last.close,
                                    high: last.close.max(price),
                                    low: last.close.min(price),
                                    close: price,
                                    volume: 0.0,
                                };
                                inner.candles.push(new_candle);
                                if inner.candles.len() > MAX_CANDLES {
                                    inner.candles.remove(0);
                                }
                                changed = true;
                            } else {
                                // Older buckets update the forming last candle so ticker price stays visible.
                                let high = last.high.max(price);
                                let low = last.low.min(price);
                                changed = last.close != price || last.high != high || last.low != low;
                                if changed {
                                    last.close = price;
                                    last.high = high;
                                    last.low = low;
                                }
                            }
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
/// After merging, evict the oldest candles if the buffer exceeds `MAX_CANDLES`.
fn merge_candles(buf: &mut Vec<OhlcvCandle>, incoming: &[OhlcvCandle]) -> bool {
    let before = buf.clone();
    for candle in incoming {
        match buf.binary_search_by_key(&candle.timestamp, |c| c.timestamp) {
            Ok(idx) => {
                // Preserve a no-op update so it cannot invalidate caches.
                if buf[idx] != *candle {
                    buf[idx] = candle.clone();
                }
            }
            Err(idx) => buf.insert(idx, candle.clone()),
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
        merge_candles(&mut buf, &[candle(2000, 2.0)]);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf[0].timestamp, 1000);
        assert_eq!(buf[1].timestamp, 2000);
        assert_eq!(buf[2].timestamp, 3000);
    }

    #[test]
    fn merge_upserts_existing() {
        let mut buf = vec![candle(1000, 1.0), candle(2000, 2.0)];
        // Update the forming candle at ts=2000 with a new close price.
        merge_candles(&mut buf, &[candle(2000, 2.5)]);
        assert_eq!(buf.len(), 2);
        assert!((buf[1].close - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn merge_evicts_oldest_when_over_limit() {
        let mut buf: Vec<OhlcvCandle> = (0..MAX_CANDLES as i64)
            .map(|i| candle(i * 1000, i as f64))
            .collect();
        // Add one more — should evict the oldest.
        merge_candles(&mut buf, &[candle(MAX_CANDLES as i64 * 1000, 999.0)]);
        assert_eq!(buf.len(), MAX_CANDLES);
        // Oldest (ts=0) should be gone.
        assert_eq!(buf[0].timestamp, 1000);
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
}
