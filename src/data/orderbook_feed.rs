/// 60-FPS–safe order book feed.
///
/// # Architecture
///
/// ```text
/// broadcast::Receiver<Arc<OrderBook>>
///       │  (Tokio background task)
///       ▼
/// Arc<Mutex<Option<Arc<OrderBook>>>>   (latest snapshot only)
///       │  (GPUI render thread, ≤ once per 16 ms)
///       ▼
/// OrderBookPanel::render()
/// ```
///
/// Only the most recent order book snapshot is retained.  The Tokio drain
/// task atomically replaces it on every incoming message.  The GPUI render
/// thread calls `OrderBookFeed::snapshot()` for a brief mutex lock.
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::data::client::tokio_runtime;
use crate::data::orderbook::OrderBook;

// ---------------------------------------------------------------------------
// OrderBookFeed
// ---------------------------------------------------------------------------

/// Shared, thread-safe holder of the latest order book snapshot.
///
/// Clone-cheap: the inner `Arc<Mutex<…>>` is reference-counted.
#[derive(Clone)]
pub struct OrderBookFeed {
    inner: Arc<Mutex<Option<Arc<OrderBook>>>>,
    /// Keep the background drain task alive.
    _task: Arc<JoinHandle<()>>,
}

impl OrderBookFeed {
    /// Create a new feed that drains `receiver` and stores the latest snapshot.
    pub fn new(receiver: broadcast::Receiver<Arc<OrderBook>>) -> Self {
        let inner: Arc<Mutex<Option<Arc<OrderBook>>>> = Arc::new(Mutex::new(None));

        let drain_inner = Arc::clone(&inner);

        let task = tokio_runtime().spawn(async move {
            drain_loop(receiver, drain_inner).await;
        });

        Self {
            inner,
            _task: Arc::new(task),
        }
    }

    /// Return the latest order book snapshot, or `None` if none has arrived yet.
    ///
    /// Acquires the mutex briefly; safe to call from the GPUI render thread.
    pub fn snapshot(&self) -> Option<Arc<OrderBook>> {
        self.inner
            .lock()
            .expect("OrderBookFeed mutex poisoned")
            .clone()
    }

    /// True when no snapshot has been received yet.
    pub fn is_empty(&self) -> bool {
        self.snapshot().is_none()
    }
}

// ---------------------------------------------------------------------------
// Drain loop (Tokio background task)
// ---------------------------------------------------------------------------

async fn drain_loop(
    mut rx: broadcast::Receiver<Arc<OrderBook>>,
    slot: Arc<Mutex<Option<Arc<OrderBook>>>>,
) {
    loop {
        match rx.recv().await {
            Ok(book) => {
                let mut guard = slot.lock().expect("OrderBookFeed mutex poisoned");
                *guard = Some(book);
                debug!("OrderBookFeed: snapshot updated");
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("OrderBookFeed: lagged by {n} snapshots — some data was lost");
                // Continue — do not exit on lag.
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Sender dropped; broadcast is over.
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::orderbook::{OrderBook, PriceLevel};

    fn make_book(best_bid: f64, best_ask: f64) -> Arc<OrderBook> {
        Arc::new(OrderBook::new(
            "binance",
            "BTC/USDT",
            vec![PriceLevel::new(best_bid, 1.0)],
            vec![PriceLevel::new(best_ask, 1.0)],
            1_700_000_000_000,
        ))
    }

    /// Feed starts empty.
    #[tokio::test]
    async fn starts_empty() {
        let (_tx, rx) = broadcast::channel::<Arc<OrderBook>>(8);
        let feed = OrderBookFeed::new(rx);
        assert!(feed.is_empty());
    }

    /// Snapshot returns the most recent book.
    #[tokio::test]
    async fn snapshot_returns_latest() {
        let (tx, rx) = broadcast::channel(8);
        let feed = OrderBookFeed::new(rx);

        tx.send(make_book(67_000.0, 67_010.0)).unwrap();
        tx.send(make_book(68_000.0, 68_010.0)).unwrap();

        // Poll instead of sleeping a fixed 20ms: the drain task is scheduled by
        // the runtime, so a fixed sleep is a race under a loaded test binary
        // (it failed intermittently when the whole lib suite ran in parallel).
        let snap = wait_for_book(&feed, |b| {
            b.best_bid()
                .map(|p| (p - 68_000.0).abs() < f64::EPSILON)
                .unwrap_or(false)
        })
        .await
        .expect("feed never received the latest book");
        assert!((snap.best_bid().unwrap() - 68_000.0).abs() < f64::EPSILON);
    }

    /// Lagged receivers recover without panicking.
    #[tokio::test]
    async fn lagged_receiver_does_not_panic() {
        let (tx, rx) = broadcast::channel(2); // tiny buffer
        let feed = OrderBookFeed::new(rx);

        // Overflow the ring buffer.
        for i in 0..10u64 {
            let _ = tx.send(make_book(i as f64, i as f64 + 10.0));
        }

        // The receiver must still recover the newest book it could read after a
        // Lagged error. Waiting on the condition (not a fixed 30ms) keeps this
        // deterministic regardless of runtime scheduling.
        let snap = wait_for_book(&feed, |_| true).await;
        assert!(snap.is_some(), "feed did not recover after lagging");
    }

    /// Poll `feed` until a book satisfies `pred`, or time out.
    async fn wait_for_book(
        feed: &OrderBookFeed,
        pred: impl Fn(&OrderBook) -> bool,
    ) -> Option<Arc<OrderBook>> {
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        loop {
            if let Some(book) = feed.snapshot() {
                if pred(&book) {
                    return Some(book);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        }
    }
}
