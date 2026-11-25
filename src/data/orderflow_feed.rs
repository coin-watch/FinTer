/// Real-time order-flow aggregation for one market/timeframe.
///
/// The bridge delivers trades on a broadcast channel. This feed drains them on
/// a Tokio task, buckets them by bar and aggressor side, and exposes an
/// immutable snapshot to the GPUI render thread. The render thread never does
/// per-trade work.
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::data::client::tokio_runtime;
use crate::data::tick::{TradeSide, UnifiedTick};

const MAX_SEEN_TRADE_IDS: usize = 20_000;
const MAX_BARS: usize = 5_000;

/// Aggregated volume at one price row within a bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceFlow {
    pub price: f64,
    pub price_low: f64,
    pub price_high: f64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub buy_count: u32,
    pub sell_count: u32,
}

impl PriceFlow {
    pub fn delta(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }

    pub fn volume(&self) -> f64 {
        self.buy_volume + self.sell_volume
    }
}

/// One aggregated bar. Volume is expressed in base units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowBar {
    pub timestamp: i64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub trades: u32,
    /// Per-price rows in ascending price order.
    pub price_rows: Vec<PriceFlow>,
    /// Backend POC, VAH and VAL for this bar when known.
    pub poc: Option<f64>,
    pub vah: Option<f64>,
    pub val: Option<f64>,
    pub total_volume: f64,
    pub row_size: f64,
    pub is_closed: bool,
}

impl FlowBar {
    pub fn delta(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }

    pub fn volume(&self) -> f64 {
        self.buy_volume + self.sell_volume
    }
}

/// Point-in-time order-flow state shared with the chart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrderFlowSnapshot {
    /// Bars in ascending timestamp order.
    pub bars: Vec<FlowBar>,
    /// Cumulative delta aligned with `bars`, as `(bar timestamp, CVD)`.
    pub cvd: Vec<(i64, f64)>,
    /// Cumulative delta over every retained bar.
    pub total_delta: f64,
    /// True when the bridge lagged and the tape may have holes.
    pub partial: bool,
    /// Backend-derived session profile rows, when subscribed.
    pub profile_rows: Vec<DerivedFootprintRow>,
    pub profile_poc: Option<f64>,
    pub profile_vah: Option<f64>,
    pub profile_val: Option<f64>,
    /// Backend session-profile window. `0, 0` means "whole session".
    pub profile_window_start: i64,
    pub profile_window_end: i64,
}

#[derive(Default)]
struct State {
    bars: BTreeMap<i64, FlowBar>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    partial: bool,
}

impl State {
    fn ingest(&mut self, trade: &UnifiedTick, timeframe_ms: i64) {
        if trade.timestamp_ms <= 0 || !trade.price.is_finite() || !trade.amount.is_finite() {
            return;
        }
        if trade.amount <= 0.0 || trade.price <= 0.0 {
            return;
        }

        if let Some(id) = trade.trade_id.as_ref()
            && !id.is_empty()
        {
            if !self.seen.insert(id.clone()) {
                return;
            }
            self.seen_order.push_back(id.clone());
            if self.seen_order.len() > MAX_SEEN_TRADE_IDS
                && let Some(oldest) = self.seen_order.pop_front()
            {
                self.seen.remove(&oldest);
            }
        }

        let amount = match trade.side {
            TradeSide::Buy => amount_with_positive(trade.amount),
            TradeSide::Sell => -amount_with_positive(trade.amount),
            TradeSide::Unknown => return,
        };
        let timeframe_ms = timeframe_ms.max(1_000);
        let bar_ts = trade.timestamp_ms.div_euclid(timeframe_ms) * timeframe_ms;
        let bar = self.bars.entry(bar_ts).or_insert(FlowBar {
            timestamp: bar_ts,
            ..FlowBar::default()
        });
        if amount >= 0.0 {
            bar.buy_volume += amount;
        } else {
            bar.sell_volume += -amount;
        }
        bar.total_volume = bar.buy_volume + bar.sell_volume;
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
        if amount >= 0.0 {
            price_row.buy_volume += amount;
            price_row.buy_count = price_row.buy_count.saturating_add(1);
        } else {
            price_row.sell_volume += -amount;
            price_row.sell_count = price_row.sell_count.saturating_add(1);
        }

        while self.bars.len() > MAX_BARS {
            let Some(oldest) = self.bars.keys().next().copied() else {
                break;
            };
            self.bars.remove(&oldest);
        }
    }

    fn snapshot(&self) -> OrderFlowSnapshot {
        let bars: Vec<FlowBar> = self.bars.values().cloned().collect();
        let mut cvd = Vec::with_capacity(bars.len());
        let mut running = 0.0;
        for bar in &bars {
            running += bar.delta();
            cvd.push((bar.timestamp, running));
        }
        OrderFlowSnapshot {
            bars,
            cvd,
            total_delta: running,
            partial: self.partial,
            profile_rows: Vec::new(),
            profile_poc: None,
            profile_vah: None,
            profile_val: None,
            profile_window_start: 0,
            profile_window_end: 0,
        }
    }
}

fn amount_with_positive(amount: f64) -> f64 {
    if amount.is_finite() && amount > 0.0 {
        amount
    } else {
        0.0
    }
}

/// Merge historical flow bars behind a live snapshot without double counting.
///
/// Only history bars strictly older than the oldest live bar are inserted, so
/// the live window (including the seam bar) remains authoritative. Bars are
/// deduplicated by timestamp and sorted ascending, and CVD/`total_delta` are
/// recomputed across the merged series.
///
/// If the live snapshot has no bars there is no anchor for the seam; the live
/// path stays authoritative and this returns `0` without touching history.
/// Returns the number of history bars inserted.
pub fn merge_history_bars(live: &mut OrderFlowSnapshot, history: Vec<FlowBar>) -> usize {
    let Some(oldest_live) = live.bars.first().map(|bar| bar.timestamp) else {
        return 0;
    };
    let mut history: Vec<FlowBar> = history
        .into_iter()
        .filter(|bar| bar.timestamp > 0 && bar.timestamp < oldest_live)
        .collect();
    if history.is_empty() {
        return 0;
    }
    history.sort_by_key(|bar| bar.timestamp);
    history.dedup_by_key(|bar| bar.timestamp);
    let inserted = history.len();

    let mut merged = Vec::with_capacity(inserted + live.bars.len());
    merged.append(&mut history);
    merged.extend(live.bars.iter().cloned());
    merged.sort_by_key(|bar| bar.timestamp);
    // History already excludes the oldest live timestamp; this is a defensive
    // dedupe for malformed pages that repeat a bar.
    merged.dedup_by_key(|bar| bar.timestamp);

    let mut cvd = Vec::with_capacity(merged.len());
    let mut running = 0.0;
    for bar in &merged {
        running += bar.delta();
        cvd.push((bar.timestamp, running));
    }
    live.bars = merged;
    live.cvd = cvd;
    live.total_delta = running;
    inserted
}

/// Shared order-flow feed driven by the bridge's trade stream.
pub struct OrderFlowFeed {
    inner: Arc<Mutex<State>>,
    version: Arc<AtomicU64>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl OrderFlowFeed {
    pub fn new(receiver: broadcast::Receiver<Arc<UnifiedTick>>, timeframe_ms: i64) -> Self {
        let inner = Arc::new(Mutex::new(State::default()));
        let version = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();

        let task_inner = Arc::clone(&inner);
        let task_version = Arc::clone(&version);
        let task_cancel = cancel.clone();
        let task = tokio_runtime().spawn(async move {
            drain_loop(
                receiver,
                task_inner,
                task_version,
                task_cancel,
                timeframe_ms,
            )
            .await;
        });

        Self {
            inner,
            version,
            cancel,
            _task: task,
        }
    }

    /// Cheap snapshot for the render thread.
    pub fn snapshot_with_version(&self) -> (u64, OrderFlowSnapshot) {
        let value = self
            .inner
            .lock()
            .map(|state| state.snapshot())
            .unwrap_or_default();
        (self.version.load(Ordering::Relaxed), value)
    }

    pub fn snapshot(&self) -> OrderFlowSnapshot {
        self.snapshot_with_version().1
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn version_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.version)
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.bars.is_empty())
            .unwrap_or(true)
    }
}

impl Drop for OrderFlowFeed {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn drain_loop(
    mut receiver: broadcast::Receiver<Arc<UnifiedTick>>,
    inner: Arc<Mutex<State>>,
    version: Arc<AtomicU64>,
    cancel: CancellationToken,
    timeframe_ms: i64,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            message = receiver.recv() => {
                match message {
                    Ok(trade) => {
                        if let Ok(mut state) = inner.lock() {
                            state.ingest(&trade, timeframe_ms);
                        }
                        version.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let _ = skipped;
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

/// Derived Level 3 events received from the backend. FinTer renders these
/// directly instead of recomputing order flow from raw trades.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DerivedDelta {
    pub timeframe: String,
    pub bar_start: i64,
    pub bar_end: i64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub delta: f64,
    pub cvd: f64,
    pub vwap: f64,
    pub trades: u32,
    pub max_print: f64,
    pub is_closed: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DerivedFootprintRow {
    pub price_low: f64,
    pub price_high: f64,
    pub buy: f64,
    pub sell: f64,
    pub buy_count: u32,
    pub sell_count: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DerivedFootprint {
    pub timeframe: String,
    pub bar_start: i64,
    pub bar_end: i64,
    pub rows: Vec<DerivedFootprintRow>,
    pub poc: Option<f64>,
    pub vah: Option<f64>,
    pub val: Option<f64>,
    pub delta: f64,
    pub total_volume: f64,
    #[serde(default)]
    pub row_size: f64,
    pub is_closed: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DerivedProfile {
    pub timeframe: String,
    pub window_start: i64,
    pub window_end: i64,
    pub rows: Vec<DerivedFootprintRow>,
    pub poc: Option<f64>,
    pub vah: Option<f64>,
    pub val: Option<f64>,
    pub total_volume: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DerivedFlowEvent {
    Delta(DerivedDelta),
    Footprint(DerivedFootprint),
    Profile(DerivedProfile),
}

#[derive(Default)]
struct DerivedState {
    bars: BTreeMap<i64, FlowBar>,
    cvd: BTreeMap<i64, f64>,
    profile: Vec<DerivedFootprintRow>,
    profile_poc: Option<f64>,
    profile_vah: Option<f64>,
    profile_val: Option<f64>,
    profile_window_start: i64,
    profile_window_end: i64,
    partial: bool,
}

impl DerivedState {
    fn ingest(&mut self, event: &DerivedFlowEvent) {
        match event {
            DerivedFlowEvent::Delta(delta) => {
                let bar = self.bars.entry(delta.bar_start).or_insert(FlowBar {
                    timestamp: delta.bar_start,
                    ..FlowBar::default()
                });
                // A footprint event is authoritative for the price ladder. A
                // delta event can arrive later; overwriting the bar volumes from
                // it would then disagree with `price_rows`. Keep the row-derived
                // totals when rows exist and only refresh the live metadata.
                if bar.price_rows.is_empty() {
                    bar.buy_volume = delta.buy_volume;
                    bar.sell_volume = delta.sell_volume;
                    bar.total_volume = delta.buy_volume + delta.sell_volume;
                } else {
                    let buy: f64 = bar.price_rows.iter().map(|row| row.buy_volume).sum();
                    let sell: f64 = bar.price_rows.iter().map(|row| row.sell_volume).sum();
                    if buy + sell > 0.0 {
                        bar.buy_volume = buy;
                        bar.sell_volume = sell;
                        bar.total_volume = buy + sell;
                    }
                }
                bar.trades = delta.trades;
                bar.is_closed = delta.is_closed;
                self.cvd.insert(delta.bar_start, delta.cvd);
            }
            DerivedFlowEvent::Footprint(footprint) => {
                let bar = self.bars.entry(footprint.bar_start).or_insert(FlowBar {
                    timestamp: footprint.bar_start,
                    ..FlowBar::default()
                });
                bar.buy_volume = 0.0;
                bar.sell_volume = 0.0;
                bar.trades = 0;
                bar.price_rows.clear();
                for row in &footprint.rows {
                    let price_row = PriceFlow {
                        price: (row.price_low + row.price_high) / 2.0,
                        price_low: row.price_low,
                        price_high: row.price_high,
                        buy_volume: row.buy,
                        sell_volume: row.sell,
                        buy_count: row.buy_count,
                        sell_count: row.sell_count,
                    };
                    bar.buy_volume += price_row.buy_volume;
                    bar.sell_volume += price_row.sell_volume;
                    bar.trades += price_row.buy_count + price_row.sell_count;
                    bar.price_rows.push(price_row);
                }
                bar.price_rows
                    .sort_by(|a, b| a.price_low.total_cmp(&b.price_low));
                bar.poc = footprint.poc;
                bar.vah = footprint.vah;
                bar.val = footprint.val;
                bar.total_volume = footprint.total_volume;
                bar.row_size = footprint.row_size;
                bar.is_closed = footprint.is_closed;
            }
            DerivedFlowEvent::Profile(profile) => {
                self.profile = profile.rows.clone();
                self.profile_poc = profile.poc;
                self.profile_vah = profile.vah;
                self.profile_val = profile.val;
                self.profile_window_start = profile.window_start;
                self.profile_window_end = profile.window_end;
            }
        }
    }

    fn snapshot(&self) -> OrderFlowSnapshot {
        let bars: Vec<FlowBar> = self.bars.values().cloned().collect();
        let mut cvd = Vec::with_capacity(bars.len());
        let mut running = 0.0;
        for bar in &bars {
            if let Some(value) = self.cvd.get(&bar.timestamp) {
                running = *value;
            } else {
                running += bar.delta();
            }
            cvd.push((bar.timestamp, running));
        }
        OrderFlowSnapshot {
            bars,
            cvd,
            total_delta: running,
            partial: self.partial,
            profile_rows: self.profile.clone(),
            profile_poc: self.profile_poc,
            profile_vah: self.profile_vah,
            profile_val: self.profile_val,
            profile_window_start: self.profile_window_start,
            profile_window_end: self.profile_window_end,
        }
    }
}

/// Feed that consumes derived Level 3 events from the backend.
pub struct DerivedFlowFeed {
    inner: Arc<Mutex<DerivedState>>,
    version: Arc<AtomicU64>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl DerivedFlowFeed {
    pub fn new(receiver: broadcast::Receiver<Arc<DerivedFlowEvent>>) -> Self {
        let inner = Arc::new(Mutex::new(DerivedState::default()));
        let version = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();

        let task_inner = Arc::clone(&inner);
        let task_version = Arc::clone(&version);
        let task_cancel = cancel.clone();
        let task = tokio_runtime().spawn(async move {
            let mut receiver = receiver;
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    message = receiver.recv() => {
                        match message {
                            Ok(event) => {
                                if let Ok(mut state) = task_inner.lock() {
                                    state.ingest(&event);
                                }
                                task_version.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                if let Ok(mut state) = task_inner.lock() {
                                    state.partial = true;
                                }
                                task_version.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });

        Self {
            inner,
            version,
            cancel,
            _task: task,
        }
    }

    pub fn snapshot_with_version(&self) -> (u64, OrderFlowSnapshot) {
        let value = self
            .inner
            .lock()
            .map(|state| state.snapshot())
            .unwrap_or_default();
        (self.version.load(Ordering::Relaxed), value)
    }

    pub fn snapshot(&self) -> OrderFlowSnapshot {
        self.snapshot_with_version().1
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn version_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.version)
    }

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.bars.is_empty())
            .unwrap_or(true)
    }
}

impl Drop for DerivedFlowFeed {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn trade(id: &str, side: TradeSide, price: f64, amount: f64, ts: i64) -> UnifiedTick {
        let mut tick = UnifiedTick::new("binance", "BTC/USDT", price, amount, side, ts);
        tick.trade_id = Some(id.to_string());
        tick
    }

    #[test]
    fn buckets_buy_sell_and_skips_unknown_side() {
        let mut state = State::default();
        state.ingest(&trade("1", TradeSide::Buy, 100.0, 2.0, 60_500), 60_000);
        state.ingest(&trade("2", TradeSide::Sell, 100.0, 1.0, 60_999), 60_000);
        state.ingest(&trade("3", TradeSide::Unknown, 100.0, 5.0, 60_999), 60_000);
        state.ingest(&trade("4", TradeSide::Buy, 100.0, 3.0, 120_001), 60_000);

        let snapshot = state.snapshot();
        assert_eq!(snapshot.bars.len(), 2);
        assert_eq!(snapshot.bars[0].timestamp, 60_000);
        assert!((snapshot.bars[0].buy_volume - 2.0).abs() < f64::EPSILON);
        assert!((snapshot.bars[0].sell_volume - 1.0).abs() < f64::EPSILON);
        assert!((snapshot.bars[0].delta() - 1.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.bars[0].price_rows.len(), 1);
        let price_row = snapshot.bars[0].price_rows.first().expect("price row");
        assert!((price_row.price - 100.0).abs() < f64::EPSILON);
        assert!((price_row.buy_volume - 2.0).abs() < f64::EPSILON);
        assert!((price_row.sell_volume - 1.0).abs() < f64::EPSILON);
        assert!((snapshot.cvd[1].1 - 4.0).abs() < f64::EPSILON);
        assert!((snapshot.total_delta - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn dedupes_trade_ids_and_allows_late_bars() {
        let mut state = State::default();
        state.ingest(&trade("1", TradeSide::Buy, 100.0, 2.0, 60_100), 60_000);
        state.ingest(&trade("1", TradeSide::Buy, 100.0, 2.0, 60_100), 60_000);
        state.ingest(&trade("2", TradeSide::Sell, 100.0, 1.0, 1_000), 60_000);

        let snapshot = state.snapshot();
        assert_eq!(snapshot.bars.len(), 2);
        assert_eq!(snapshot.bars[0].timestamp, 0);
        assert!((snapshot.bars[0].sell_volume - 1.0).abs() < f64::EPSILON);
        let total: f64 = snapshot.bars.iter().map(FlowBar::volume).sum();
        assert!((total - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeframe_controls_bar_boundaries() {
        let mut state = State::default();
        state.ingest(&trade("1", TradeSide::Buy, 100.0, 2.0, 299_999), 300_000);
        state.ingest(&trade("2", TradeSide::Buy, 100.0, 2.0, 300_000), 300_000);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.bars.len(), 2);
        assert_eq!(snapshot.bars[0].timestamp, 0);
        assert_eq!(snapshot.bars[1].timestamp, 300_000);
    }

    #[test]
    fn derived_events_populate_display_snapshot() {
        let mut state = DerivedState::default();
        state.ingest(&DerivedFlowEvent::Delta(DerivedDelta {
            timeframe: "1m".into(),
            bar_start: 60_000,
            bar_end: 120_000,
            buy_volume: 3.0,
            sell_volume: 1.0,
            delta: 2.0,
            cvd: 2.0,
            vwap: 100.0,
            trades: 4,
            max_print: 200.0,
            is_closed: false,
        }));
        state.ingest(&DerivedFlowEvent::Footprint(DerivedFootprint {
            timeframe: "1m".into(),
            bar_start: 60_000,
            bar_end: 120_000,
            rows: vec![
                DerivedFootprintRow {
                    price_low: 100.0,
                    price_high: 100.0,
                    buy: 2.0,
                    sell: 1.0,
                    buy_count: 2,
                    sell_count: 1,
                },
                DerivedFootprintRow {
                    price_low: 101.0,
                    price_high: 101.0,
                    buy: 1.0,
                    sell: 0.0,
                    buy_count: 1,
                    sell_count: 0,
                },
            ],
            poc: Some(100.0),
            vah: Some(101.0),
            val: Some(100.0),
            delta: 2.0,
            total_volume: 4.0,
            row_size: 1.0,
            is_closed: false,
        }));

        let snapshot = state.snapshot();
        assert_eq!(snapshot.bars.len(), 1);
        assert_eq!(snapshot.bars[0].price_rows.len(), 2);
        assert!((snapshot.bars[0].buy_volume - 3.0).abs() < f64::EPSILON);
        assert!((snapshot.bars[0].sell_volume - 1.0).abs() < f64::EPSILON);
        assert!((snapshot.cvd[0].1 - 2.0).abs() < f64::EPSILON);
    }

    fn footprint_event(bar_start: i64) -> DerivedFlowEvent {
        DerivedFlowEvent::Footprint(DerivedFootprint {
            timeframe: "1m".into(),
            bar_start,
            bar_end: bar_start + 60_000,
            rows: vec![
                DerivedFootprintRow {
                    price_low: 100.0,
                    price_high: 100.5,
                    buy: 2.0,
                    sell: 1.0,
                    buy_count: 2,
                    sell_count: 1,
                },
                DerivedFootprintRow {
                    price_low: 100.5,
                    price_high: 101.0,
                    buy: 1.0,
                    sell: 0.5,
                    buy_count: 1,
                    sell_count: 1,
                },
            ],
            poc: Some(100.25),
            vah: Some(101.0),
            val: Some(100.0),
            delta: 1.5,
            total_volume: 4.5,
            row_size: 0.5,
            is_closed: false,
        })
    }

    fn delta_event(bar_start: i64) -> DerivedFlowEvent {
        DerivedFlowEvent::Delta(DerivedDelta {
            timeframe: "1m".into(),
            bar_start,
            bar_end: bar_start + 60_000,
            buy_volume: 100.0,
            sell_volume: 50.0,
            delta: 50.0,
            cvd: 50.0,
            vwap: 100.0,
            trades: 9,
            max_print: 200.0,
            is_closed: true,
        })
    }

    #[test]
    fn delta_after_footprint_keeps_rows_and_row_derived_totals() {
        let mut state = DerivedState::default();
        state.ingest(&footprint_event(60_000));
        state.ingest(&delta_event(60_000));

        let snapshot = state.snapshot();
        let bar = &snapshot.bars[0];
        assert_eq!(bar.price_rows.len(), 2);
        assert!((bar.row_size - 0.5).abs() < f64::EPSILON);
        // The delta event's 100/50 volumes must not overwrite the ladder.
        assert!((bar.buy_volume - 3.0).abs() < f64::EPSILON);
        assert!((bar.sell_volume - 1.5).abs() < f64::EPSILON);
        assert!((bar.total_volume - 4.5).abs() < f64::EPSILON);
        // Live metadata still comes from the delta event.
        assert_eq!(bar.trades, 9);
        assert!(bar.is_closed);
        assert_eq!(bar.poc, Some(100.25));
        assert!((snapshot.cvd[0].1 - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn delta_then_footprint_uses_footprint_totals() {
        let mut state = DerivedState::default();
        state.ingest(&delta_event(60_000));
        state.ingest(&footprint_event(60_000));

        let snapshot = state.snapshot();
        let bar = &snapshot.bars[0];
        assert_eq!(bar.price_rows.len(), 2);
        assert!((bar.buy_volume - 3.0).abs() < f64::EPSILON);
        assert!((bar.sell_volume - 1.5).abs() < f64::EPSILON);
        // The footprint event is authoritative for the bar total.
        assert!((bar.total_volume - 4.5).abs() < f64::EPSILON);
        assert!(!bar.is_closed);
    }

    #[test]
    fn delta_without_rows_uses_delta_volumes() {
        let mut state = DerivedState::default();
        state.ingest(&delta_event(60_000));
        let snapshot = state.snapshot();
        let bar = &snapshot.bars[0];
        assert!(bar.price_rows.is_empty());
        assert!((bar.buy_volume - 100.0).abs() < f64::EPSILON);
        assert!((bar.sell_volume - 50.0).abs() < f64::EPSILON);
        assert!((bar.total_volume - 150.0).abs() < f64::EPSILON);
    }

    fn flow_bar(timestamp: i64, buy: f64, sell: f64) -> FlowBar {
        FlowBar {
            timestamp,
            buy_volume: buy,
            sell_volume: sell,
            total_volume: buy + sell,
            is_closed: true,
            ..FlowBar::default()
        }
    }

    fn live_snapshot() -> OrderFlowSnapshot {
        let bars = vec![flow_bar(200, 2.0, 1.0), flow_bar(300, 1.0, 2.0)];
        let mut cvd = Vec::new();
        let mut running = 0.0;
        for bar in &bars {
            running += bar.delta();
            cvd.push((bar.timestamp, running));
        }
        OrderFlowSnapshot {
            bars,
            cvd,
            total_delta: running,
            ..OrderFlowSnapshot::default()
        }
    }

    #[test]
    fn merge_history_inserts_only_before_live_and_recomputes_cvd() {
        let mut live = live_snapshot();
        let inserted = merge_history_bars(
            &mut live,
            vec![
                flow_bar(100, 0.0, 1.0),
                // At/after the oldest live bar: the live path owns the seam.
                flow_bar(200, 99.0, 0.0),
                flow_bar(250, 99.0, 0.0),
                // Duplicate history timestamp: first one wins after sorting.
                flow_bar(50, 4.0, 0.0),
                flow_bar(50, 9.0, 0.0),
            ],
        );
        // Timestamps 50 and 100 are inserted; the 50 duplicate collapses.
        assert_eq!(inserted, 2);
        let timestamps: Vec<i64> = live.bars.iter().map(|bar| bar.timestamp).collect();
        assert_eq!(timestamps, vec![50, 100, 200, 300]);
        // deltas: +4, -1, +1, -1 => cvd 4, 3, 4, 3
        assert_eq!(
            live.cvd,
            vec![(50, 4.0), (100, 3.0), (200, 4.0), (300, 3.0)]
        );
        assert!((live.total_delta - 3.0).abs() < f64::EPSILON);
        // The live seam bar's volumes are untouched.
        assert!((live.bars[2].buy_volume - 2.0).abs() < f64::EPSILON);
        assert!((live.bars[2].sell_volume - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn merge_history_without_live_anchor_is_a_noop() {
        let mut live = OrderFlowSnapshot::default();
        let inserted = merge_history_bars(&mut live, vec![flow_bar(100, 1.0, 0.0)]);
        assert_eq!(inserted, 0);
        assert!(live.bars.is_empty());
        assert!(live.cvd.is_empty());
        assert_eq!(live.total_delta, 0.0);
    }

    #[test]
    fn merge_history_is_idempotent_for_overlapping_windows() {
        let mut live = live_snapshot();
        let history = vec![flow_bar(50, 1.0, 0.0), flow_bar(100, 0.0, 1.0)];
        assert_eq!(merge_history_bars(&mut live, history.clone()), 2);
        let first_total = live.total_delta;
        // A fresh merge starts from the feed snapshot again; applying the same
        // history to the already-merged snapshot must not add new bars.
        assert_eq!(merge_history_bars(&mut live, history), 0);
        assert_eq!(live.bars.len(), 4);
        assert_eq!(live.total_delta, first_total);
    }
}
