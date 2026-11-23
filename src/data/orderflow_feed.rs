/// Real-time order-flow aggregation for one market/timeframe.
///
/// The bridge delivers trades on a broadcast channel. This feed drains them on
/// a Tokio task, buckets them by bar and aggressor side, and exposes an
/// immutable snapshot to the GPUI render thread. The render thread never does
/// per-trade work.
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::data::client::tokio_runtime;
use crate::data::tick::{TradeSide, UnifiedTick};

const MAX_SEEN_TRADE_IDS: usize = 20_000;
const MAX_BARS: usize = 5_000;

/// Aggregated volume at one price within a bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceFlow {
    pub price: f64,
    pub buy_volume: f64,
    pub sell_volume: f64,
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
    /// Per-price volume, keyed by price in millionths.
    pub price_rows: BTreeMap<i64, PriceFlow>,
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

        if let Some(id) = trade.trade_id.as_ref() {
            if !id.is_empty() {
                if !self.seen.insert(id.clone()) {
                    return;
                }
                self.seen_order.push_back(id.clone());
                if self.seen_order.len() > MAX_SEEN_TRADE_IDS {
                    if let Some(oldest) = self.seen_order.pop_front() {
                        self.seen.remove(&oldest);
                    }
                }
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
        bar.trades = bar.trades.saturating_add(1);

        let price_key = (trade.price * 1_000_000.0).round() as i64;
        let price_row = bar.price_rows.entry(price_key).or_insert(PriceFlow {
            price: trade.price,
            ..PriceFlow::default()
        });
        if amount >= 0.0 {
            price_row.buy_volume += amount;
        } else {
            price_row.sell_volume += -amount;
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
        let price_row = snapshot.bars[0]
            .price_rows
            .values()
            .next()
            .expect("price row");
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
}
