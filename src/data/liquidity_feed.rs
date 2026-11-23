/// Bounded order-book history for the liquidity heatmap.
///
/// The bridge emits full book snapshots. This feed keeps a small time-ordered
/// ring of top-of-book levels for the chart; it never runs on the render thread.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::data::client::tokio_runtime;
use crate::data::orderbook::OrderBook;

const MAX_SAMPLES: usize = 600;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiquidityLevel {
    pub price: f64,
    pub quantity: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiquiditySample {
    pub timestamp_ms: i64,
    pub levels: Vec<LiquidityLevel>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiquiditySnapshot {
    /// Oldest sample first.
    pub samples: Vec<LiquiditySample>,
    pub partial: bool,
}

#[derive(Default)]
struct State {
    samples: VecDeque<LiquiditySample>,
    partial: bool,
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
        self.samples.push_back(LiquiditySample {
            timestamp_ms,
            levels,
        });
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
    }

    fn snapshot(&self) -> LiquiditySnapshot {
        LiquiditySnapshot {
            samples: self.samples.iter().cloned().collect(),
            partial: self.partial,
        }
    }
}

pub struct LiquidityFeed {
    inner: Arc<Mutex<State>>,
    version: Arc<AtomicU64>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
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
            _task: task,
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

    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|state| state.samples.is_empty())
            .unwrap_or(true)
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

    #[test]
    fn samples_are_ordered_and_bounded() {
        let mut state = State::default();
        for ts in 1..=(MAX_SAMPLES as i64 + 5) {
            state.ingest(&book(ts, 100.0, 101.0));
        }
        let snapshot = state.snapshot();
        assert_eq!(snapshot.samples.len(), MAX_SAMPLES);
        assert_eq!(snapshot.samples.first().unwrap().timestamp_ms, 6);
        assert_eq!(snapshot.samples.last().unwrap().timestamp_ms, 605);
        assert_eq!(snapshot.samples.last().unwrap().levels.len(), 2);
    }
}
