/// 60-FPS–throttled tick feed with a fixed-capacity ring buffer.
///
/// # Architecture
///
/// ```text
/// broadcast::Receiver<Arc<UnifiedTick>>
///       │  (Tokio background task)
///       ▼
/// Arc<Mutex<VecDeque<Arc<UnifiedTick>>>>   (ring buffer, max MAX_TICKS)
///       │  (GPUI render thread, ≤ once per 16 ms)
///       ▼
/// TapePanel::render()
/// ```
///
/// The Tokio task pushes every incoming tick into the ring buffer.  The
/// GPUI render thread calls `TickFeed::snapshot()` to get a cheap
/// point-in-time clone of the deque.  There is no blocking — only the
/// brief mutex lock required to push/pop.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::data::client::tokio_runtime;
use crate::data::tick::UnifiedTick;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of ticks retained in the ring buffer.
/// At 10k ticks/sec this is 50 ms of history — more than enough for the
/// Tape panel to display recent trades.
pub const MAX_TICKS: usize = 500;

// ---------------------------------------------------------------------------
// TickFeed
// ---------------------------------------------------------------------------

/// Shared, thread-safe ring buffer of recent ticks.
///
/// Clone-cheap: the inner `Arc<Mutex<…>>` is reference-counted.
#[derive(Clone)]
pub struct TickFeed {
    inner: Arc<Mutex<VecDeque<Arc<UnifiedTick>>>>,
    /// Keep the background drain task alive.
    _task: Arc<JoinHandle<()>>,
}

impl TickFeed {
    /// Create a new `TickFeed` that drains `receiver` into the ring buffer.
    ///
    /// The drain task runs until the sender side of `receiver` is dropped or
    /// the `TickFeed` itself is dropped.
    pub fn new(receiver: broadcast::Receiver<Arc<UnifiedTick>>) -> Self {
        let inner: Arc<Mutex<VecDeque<Arc<UnifiedTick>>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(MAX_TICKS)));

        let drain_inner = Arc::clone(&inner);

        let task = tokio_runtime().spawn(async move {
            drain_loop(receiver, drain_inner).await;
        });

        Self {
            inner,
            _task: Arc::new(task),
        }
    }

    /// Return a point-in-time snapshot of the ring buffer (newest last).
    ///
    /// Acquires the mutex briefly; safe to call from the GPUI render thread.
    pub fn snapshot(&self) -> Vec<Arc<UnifiedTick>> {
        self.inner
            .lock()
            .expect("TickFeed mutex poisoned")
            .iter()
            .cloned()
            .collect()
    }

    /// Number of ticks currently in the ring buffer.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("TickFeed mutex poisoned").len()
    }

    /// True when no ticks have been received yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Drain loop (Tokio background task)
// ---------------------------------------------------------------------------

async fn drain_loop(
    mut rx: broadcast::Receiver<Arc<UnifiedTick>>,
    buffer: Arc<Mutex<VecDeque<Arc<UnifiedTick>>>>,
) {
    loop {
        match rx.recv().await {
            Ok(tick) => {
                let mut buf = buffer.lock().expect("TickFeed mutex poisoned");
                if buf.len() >= MAX_TICKS {
                    buf.pop_front(); // evict oldest
                }
                buf.push_back(tick);
                debug!("TickFeed: buffer size = {}", buf.len());
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!("TickFeed: lagged by {n} ticks — some data was lost");
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
    use crate::data::tick::{TradeSide, UnifiedTick};

    fn make_tick(price: f64) -> Arc<UnifiedTick> {
        Arc::new(UnifiedTick::new(
            "binance",
            "BTC/USDT",
            price,
            0.1,
            TradeSide::Buy,
            1_700_000_000_000,
        ))
    }

    /// Feed pushes ticks and snapshot returns them in FIFO order.
    #[tokio::test]
    async fn snapshot_fifo_order() {
        let (tx, rx) = broadcast::channel(64);
        let feed = TickFeed::new(rx);

        tx.send(make_tick(100.0)).unwrap();
        tx.send(make_tick(200.0)).unwrap();
        tx.send(make_tick(300.0)).unwrap();

        // Give the drain task a moment to process.
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        let snap = feed.snapshot();
        assert_eq!(snap.len(), 3);
        assert!((snap[0].price - 100.0).abs() < f64::EPSILON);
        assert!((snap[2].price - 300.0).abs() < f64::EPSILON);
    }

    /// Ring buffer evicts oldest when full.
    #[tokio::test]
    async fn ring_buffer_evicts_oldest() {
        let (tx, rx) = broadcast::channel(MAX_TICKS + 64);
        let feed = TickFeed::new(rx);

        // Overflow by 10 ticks.
        let overflow = 10;
        for i in 0..(MAX_TICKS + overflow) {
            tx.send(make_tick(i as f64)).unwrap();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let snap = feed.snapshot();
        assert_eq!(snap.len(), MAX_TICKS);
        // Oldest retained tick should be the (overflow)th one.
        assert!((snap[0].price - overflow as f64).abs() < f64::EPSILON);
    }

    /// Feed starts empty.
    #[tokio::test]
    async fn starts_empty() {
        let (_tx, rx) = broadcast::channel::<Arc<UnifiedTick>>(64);
        let feed = TickFeed::new(rx);
        assert!(feed.is_empty());
    }
}
