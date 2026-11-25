/// Bounded order-book history for the liquidity heatmap and L1 step lines.
///
/// The bridge emits full book snapshots. This feed keeps a time-ordered ring
/// of book snapshots; it never runs on the render thread. Historical L2
/// samples fetched from CoinWatch are merged behind the live floor so a
/// backfill can never rewrite the live right edge.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::data::book_history::OrderbookSample;
use crate::data::client::tokio_runtime;
use crate::data::orderbook::OrderBook;

/// 4096 samples holds 15 minutes of 1s history plus a healthy live ring at
/// 10 Hz (the live feed appends ~600 samples/minute).
const MAX_SAMPLES: usize = 4096;

/// Retain at most 15 minutes of backfilled history behind the live floor.
const HISTORY_RETENTION_MS: i64 = 15 * 60 * 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiquidityLevel {
    pub price: f64,
    pub quantity: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiquiditySample {
    pub timestamp_ms: i64,
    pub levels: Vec<LiquidityLevel>,
    /// Best bid at this instant (top L2 level or live book). Used by the L1
    /// step-line overlay.
    pub best_bid: Option<f64>,
    /// Best ask at this instant.
    pub best_ask: Option<f64>,
    /// `true` when a history sample only seeds the left edge of its window.
    pub carried: bool,
}

impl LiquiditySample {
    /// Build a heatmap sample from one historical L2 snapshot.
    ///
    /// Best bid/ask are derived from the same L2 levels, so the chart never
    /// spends a second ticker-history request on L1 data.
    pub fn from_history_sample(sample: &OrderbookSample) -> Option<Self> {
        if sample.ts <= 0 {
            return None;
        }
        let mut levels = Vec::with_capacity(sample.bids.len() + sample.asks.len());
        levels.extend(sample.bids.iter().filter_map(|level| {
            (level.price.is_finite() && level.quantity.is_finite() && level.quantity > 0.0)
                .then_some(LiquidityLevel {
                    price: level.price,
                    quantity: level.quantity,
                })
        }));
        levels.extend(sample.asks.iter().filter_map(|level| {
            (level.price.is_finite() && level.quantity.is_finite() && level.quantity > 0.0)
                .then_some(LiquidityLevel {
                    price: level.price,
                    quantity: level.quantity,
                })
        }));
        if levels.is_empty() {
            return None;
        }
        Some(Self {
            timestamp_ms: sample.ts,
            levels,
            best_bid: sample.best_bid(),
            best_ask: sample.best_ask(),
            carried: sample.carried,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiquiditySnapshot {
    /// Oldest sample first.
    pub samples: Vec<LiquiditySample>,
    /// The live broadcast lagged, so the live ring has a gap.
    pub partial: bool,
    /// A history request reported incomplete window coverage.
    pub history_partial: bool,
}

/// Result reported to the chart after a historical L2 request.
#[derive(Clone, Debug, PartialEq)]
pub enum LiquidityHistoryOutcome {
    /// Samples were merged into the feed.
    Loaded { partial: bool, samples: usize },
    /// The backend is still backfilling; poll again after a short delay.
    Queued,
    /// The requested window is entirely inside the 1h server lag.
    NotAvailable,
    /// Missing/expired credentials. The live feed is unaffected.
    AuthRequired,
    /// Credit exhaustion; the message is user-facing.
    Credits(String),
    /// Network/parse/other failure.
    Error(String),
}

#[derive(Default)]
struct State {
    samples: VecDeque<LiquiditySample>,
    partial: bool,
    history_partial: bool,
    /// Newest timestamp ever merged from history. Retention is measured from
    /// this right edge, not from the latest page (pages extend backwards).
    history_end_ms: Option<i64>,
    /// First live sample timestamp. History may only land strictly behind it,
    /// so a backfill can never rewrite the live right edge. Kept separate from
    /// `samples.front()` because the front becomes history after a merge.
    live_floor_ms: Option<i64>,
}

impl State {
    fn ingest(&mut self, book: &OrderBook) {
        let mut levels = Vec::with_capacity(book.bids.len() + book.asks.len());
        levels.extend(book.bids.iter().map(|level| LiquidityLevel {
            price: level.price,
            quantity: level.quantity,
        }));
        levels.extend(book.asks.iter().map(|level| LiquidityLevel {
            price: level.price,
            quantity: level.quantity,
        }));
        levels.retain(|level| level.price.is_finite() && level.quantity.is_finite());
        if levels.is_empty() {
            return;
        }
        let timestamp_ms = if book.timestamp_ms > 0 {
            book.timestamp_ms
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0)
        };
        // The live broadcast is 10 Hz while history is 1 s. Bucket live
        // snapshots to the second so the ring holds 15 minutes of each and the
        // heatmap/L1 path processes the same resolution as history.
        let bucket_ms = timestamp_ms / 1_000 * 1_000;
        if let Some(latest) = self.samples.back_mut() {
            if latest.timestamp_ms == bucket_ms {
                // Latest snapshot in the bucket wins.
                latest.levels = levels;
                latest.best_bid = book.best_bid();
                latest.best_ask = book.best_ask();
                latest.carried = false;
                return;
            }
            if latest.timestamp_ms > bucket_ms {
                // Ignore a stale snapshot so the ring stays sorted.
                return;
            }
        }
        if self.live_floor_ms.is_none() {
            self.live_floor_ms = Some(bucket_ms);
        }
        self.samples.push_back(LiquiditySample {
            timestamp_ms: bucket_ms,
            levels,
            best_bid: book.best_bid(),
            best_ask: book.best_ask(),
            carried: false,
        });
        self.trim_to_cap();
    }

    /// Merge historical samples behind the live floor.
    ///
    /// Returns `true` when the visible ring changed (new/replaced sample, a
    /// trim, or a new history-partial flag). Callers use this to bump the feed
    /// version only on real changes.
    fn merge_history(&mut self, incoming: &[LiquiditySample], partial: bool) -> bool {
        let mut changed = false;
        if partial != self.history_partial {
            self.history_partial = partial;
            changed = true;
        }

        // The live floor is the first live sample: history may only land
        // strictly behind it, so a late page can never rewrite live data.
        let live_floor = self.live_floor_ms;
        let mut accepted: Vec<LiquiditySample> = incoming
            .iter()
            .filter(|sample| sample.timestamp_ms > 0 && !sample.levels.is_empty())
            .filter(|sample| live_floor.is_none_or(|floor| sample.timestamp_ms < floor))
            .cloned()
            .collect();
        if accepted.is_empty() {
            return changed;
        }
        accepted.sort_by_key(|sample| sample.timestamp_ms);

        if let Some(newest) = accepted.last().map(|sample| sample.timestamp_ms) {
            self.history_end_ms = Some(self.history_end_ms.map_or(newest, |end| end.max(newest)));
        }
        let history_floor = self
            .history_end_ms
            .map(|end| end.saturating_sub(HISTORY_RETENTION_MS))
            .unwrap_or(i64::MIN);

        for sample in accepted {
            if sample.timestamp_ms < history_floor {
                continue;
            }
            match self
                .samples
                .binary_search_by_key(&sample.timestamp_ms, |existing| existing.timestamp_ms)
            {
                Ok(index) => {
                    if self.samples[index] != sample {
                        self.samples[index] = sample;
                        changed = true;
                    }
                }
                Err(index) => {
                    self.samples.insert(index, sample);
                    changed = true;
                }
            }
        }

        // Drop history older than the retention window, then enforce the
        // absolute count cap. Live samples are newer and are trimmed last.
        while let Some(front) = self.samples.front() {
            if front.timestamp_ms < history_floor {
                self.samples.pop_front();
                changed = true;
            } else {
                break;
            }
        }
        if self.trim_to_cap() {
            changed = true;
        }
        changed
    }

    fn trim_to_cap(&mut self) -> bool {
        let mut changed = false;
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
            changed = true;
        }
        changed
    }

    fn snapshot(&self) -> LiquiditySnapshot {
        LiquiditySnapshot {
            samples: self.samples.iter().cloned().collect(),
            partial: self.partial,
            history_partial: self.history_partial,
        }
    }
}

#[derive(Clone)]
pub struct LiquidityFeed {
    inner: Arc<Mutex<State>>,
    version: Arc<AtomicU64>,
    cancel: CancellationToken,
    _task: Arc<JoinHandle<()>>,
}

impl LiquidityFeed {
    pub fn new(receiver: broadcast::Receiver<Arc<OrderBook>>) -> Self {
        let inner = Arc::new(Mutex::new(State::default()));
        let version = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();

        let task_inner = Arc::clone(&inner);
        let task_version = Arc::clone(&version);
        let task_cancel = cancel.clone();
        let task = tokio_runtime().spawn(async move {
            drain_loop(receiver, task_inner, task_version, task_cancel).await;
        });

        Self {
            inner,
            version,
            cancel,
            _task: Arc::new(task),
        }
    }

    pub fn snapshot_with_version(&self) -> (u64, LiquiditySnapshot) {
        let value = self
            .inner
            .lock()
            .map(|state| state.snapshot())
            .unwrap_or_default();
        (self.version.load(Ordering::Relaxed), value)
    }

    pub fn version_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.version)
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.samples.is_empty())
            .unwrap_or(true)
    }

    /// Oldest retained sample timestamp (live or history).
    pub fn oldest_ts(&self) -> Option<i64> {
        self.inner
            .lock()
            .ok()
            .and_then(|state| state.samples.front().map(|sample| sample.timestamp_ms))
    }

    /// True when history has been merged behind the live samples.
    pub fn history_partial(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.history_partial)
            .unwrap_or(false)
    }

    /// Merge historical L2 samples behind the live floor.
    ///
    /// The version advances only when data actually changes, so renderer
    /// watchers do not repaint for duplicate queued polls.
    pub fn merge_history(&self, incoming: &[LiquiditySample], partial: bool) {
        if incoming.is_empty() && !partial {
            return;
        }
        let changed = self
            .inner
            .lock()
            .map(|mut state| state.merge_history(incoming, partial))
            .unwrap_or(false);
        if changed {
            self.version.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for LiquidityFeed {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn drain_loop(
    mut receiver: broadcast::Receiver<Arc<OrderBook>>,
    inner: Arc<Mutex<State>>,
    version: Arc<AtomicU64>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            message = receiver.recv() => {
                match message {
                    Ok(book) => {
                        if let Ok(mut state) = inner.lock() {
                            state.ingest(&book);
                        }
                        version.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Ok(mut state) = inner.lock() {
                            state.partial = true;
                        }
                        version.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::book_history::BookLevel;
    use crate::data::orderbook::PriceLevel;

    fn book(ts: i64, bid: f64, ask: f64) -> OrderBook {
        OrderBook::new(
            "binance",
            "BTC/USDT",
            vec![PriceLevel::new(bid, 1.0)],
            vec![PriceLevel::new(ask, 2.0)],
            ts,
        )
    }

    fn book_with_quantities(ts: i64, bid: f64, bid_qty: f64, ask: f64, ask_qty: f64) -> OrderBook {
        OrderBook::new(
            "binance",
            "BTC/USDT",
            vec![PriceLevel::new(bid, bid_qty)],
            vec![PriceLevel::new(ask, ask_qty)],
            ts,
        )
    }

    fn sample(ts: i64, bid: f64, ask: f64) -> LiquiditySample {
        LiquiditySample {
            timestamp_ms: ts,
            levels: vec![
                LiquidityLevel {
                    price: bid,
                    quantity: 1.0,
                },
                LiquidityLevel {
                    price: ask,
                    quantity: 2.0,
                },
            ],
            best_bid: Some(bid),
            best_ask: Some(ask),
            carried: false,
        }
    }

    #[test]
    fn samples_are_ordered_and_bounded() {
        let mut state = State::default();
        // One snapshot per second, so every sample gets its own bucket.
        for ts in 1..=(MAX_SAMPLES as i64 + 5) {
            state.ingest(&book(ts * 1_000, 100.0, 101.0));
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.samples.len(), MAX_SAMPLES);
        assert_eq!(snapshot.samples.first().unwrap().timestamp_ms, 6_000);
        assert_eq!(
            snapshot.samples.last().unwrap().timestamp_ms,
            (MAX_SAMPLES as i64 + 5) * 1_000
        );
        assert_eq!(snapshot.samples.last().unwrap().levels.len(), 2);
        assert_eq!(snapshot.samples.last().unwrap().best_bid, Some(100.0));
        assert_eq!(snapshot.samples.last().unwrap().best_ask, Some(101.0));
    }

    #[test]
    fn live_snapshots_are_bucketed_to_one_second() {
        let mut state = State::default();
        state.ingest(&book_with_quantities(
            1_700_000_000_100,
            100.0,
            1.0,
            101.0,
            2.0,
        ));
        state.ingest(&book_with_quantities(
            1_700_000_000_900,
            100.5,
            3.0,
            101.5,
            4.0,
        ));

        let snapshot = state.snapshot();
        assert_eq!(snapshot.samples.len(), 1);
        let sample = &snapshot.samples[0];
        assert_eq!(sample.timestamp_ms, 1_700_000_000_000);
        assert_eq!(sample.best_bid, Some(100.5));
        assert_eq!(sample.best_ask, Some(101.5));
        assert_eq!(sample.levels[0].price, 100.5);
        assert_eq!(sample.levels[0].quantity, 3.0);
        assert_eq!(sample.levels[1].quantity, 4.0);

        // A snapshot in the next second appends a new bucket.
        state.ingest(&book_with_quantities(
            1_700_000_001_050,
            100.75,
            5.0,
            101.75,
            6.0,
        ));
        let snapshot = state.snapshot();
        assert_eq!(snapshot.samples.len(), 2);
        assert_eq!(snapshot.samples[1].timestamp_ms, 1_700_000_001_000);
        assert_eq!(snapshot.samples[1].best_bid, Some(100.75));
        assert_eq!(state.live_floor_ms, Some(1_700_000_000_000));
    }

    #[test]
    fn merge_is_sorted_deduped_and_never_rewrites_live() {
        let mut state = State::default();
        state.ingest(&book(10_000, 100.0, 101.0));
        let version_before = state.snapshot();

        // Unordered, overlapping page. The duplicate of the live floor must be
        // rejected because history is strictly older than the live floor.
        state.merge_history(
            &[
                sample(7_000, 97.0, 98.0),
                sample(9_000, 99.0, 100.0),
                sample(8_000, 98.0, 99.0),
                sample(10_000, 1.0, 2.0),
            ],
            false,
        );
        let snapshot = state.snapshot();
        let timestamps: Vec<i64> = snapshot
            .samples
            .iter()
            .map(|sample| sample.timestamp_ms)
            .collect();
        assert_eq!(timestamps, vec![7_000, 8_000, 9_000, 10_000]);
        // The live sample was not replaced by the historical duplicate.
        assert_eq!(snapshot.samples.last().unwrap().best_bid, Some(100.0));
        assert_eq!(
            version_before.samples.last().unwrap().best_bid,
            snapshot.samples.last().unwrap().best_bid
        );
    }

    #[test]
    fn merge_bumps_version_only_on_change() {
        // Exercise `State::merge_history` directly to assert the change flag;
        // `LiquidityFeed::merge_history` maps it to the atomic version.
        let mut state = State::default();
        state.ingest(&book(10_000, 100.0, 101.0));
        assert!(state.merge_history(&[sample(9_000, 99.0, 100.0)], false));
        assert!(!state.merge_history(&[sample(9_000, 99.0, 100.0)], false));

        // Replacing a value is a change; identical replacement is not.
        assert!(!state.merge_history(&[sample(9_000, 99.0, 100.0)], false));
        let mut replacement = sample(9_000, 99.0, 100.0);
        replacement.levels[0].quantity = 5.0;
        assert!(state.merge_history(&[replacement], false));
    }

    #[test]
    fn history_partial_is_separate_from_broadcast_lag() {
        let mut state = State::default();
        state.ingest(&book(10_000, 100.0, 101.0));
        assert!(state.merge_history(&[sample(9_000, 99.0, 100.0)], true));
        let snapshot = state.snapshot();
        assert!(snapshot.history_partial);
        assert!(!snapshot.partial);

        // The broadcast-lag flag is only set by the drain loop.
        state.partial = true;
        let snapshot = state.snapshot();
        assert!(snapshot.history_partial);
        assert!(snapshot.partial);
    }

    #[test]
    fn history_is_trimmed_by_age_and_count_cap() {
        let mut state = State::default();
        state.ingest(&book(10_000_000, 100.0, 101.0));
        // 18 minutes of 1s history ending 15 minutes before the live floor.
        let page: Vec<LiquiditySample> = (0..=18 * 60)
            .map(|i| sample(8_020_000 + i * 1_000, 99.0, 100.0))
            .collect();
        state.merge_history(&page, false);

        let history_end = 8_020_000 + 18 * 60 * 1_000;
        let history_floor = history_end - HISTORY_RETENTION_MS;
        let oldest = state.samples.front().unwrap().timestamp_ms;
        assert_eq!(oldest, history_floor);
        assert_eq!(
            state.samples.len(),
            (HISTORY_RETENTION_MS / 1_000) as usize + 2
        );
        assert!(state.samples.len() < MAX_SAMPLES);
    }

    #[test]
    fn history_sample_conversion_derives_l1() {
        let historical = OrderbookSample {
            ts: 1_700_000_000_000,
            bids: vec![BookLevel::new(100.0, 1.0), BookLevel::new(99.5, 2.0)],
            asks: vec![BookLevel::new(100.5, 1.5), BookLevel::new(101.0, 3.0)],
            carried: true,
            ..OrderbookSample::default()
        };
        let converted = LiquiditySample::from_history_sample(&historical).expect("sample");
        assert_eq!(converted.timestamp_ms, 1_700_000_000_000);
        assert_eq!(converted.levels.len(), 4);
        assert_eq!(converted.best_bid, Some(100.0));
        assert_eq!(converted.best_ask, Some(100.5));
        assert!(converted.carried);
    }
}
