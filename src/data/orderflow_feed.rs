//! Historical + live footprint aggregation for one market/timeframe.
//!
//! Two sources merge into one bar map:
//!
//! ```text
//! GET /api/v1/orderflow/footprint  (bounded history for the viewport)
//! gateway `footprint` feed         (live in-progress bar snapshots)
//! ```
//!
//! History is fetched on demand by the chart panel when the visible window
//! changes. Live messages overwrite the in-progress bar; once a bar is closed
//! (by history or by a live `is_closed` snapshot) later messages for that
//! `bar_start` are ignored. Unknown-aggressor volume is retained separately so
//! delta stays `buy - sell` while totals include every print.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::data::bridge::BridgeConfig;
use crate::data::client::tokio_runtime;
use crate::data::tick::UnifiedTick;

const MAX_BARS: usize = 5_000;
const HISTORY_CACHE_TTL: Duration = Duration::from_secs(60);
const HISTORY_DEBOUNCE: Duration = Duration::from_millis(250);
const UNAUTHORIZED_COOLDOWN: Duration = Duration::from_secs(60);
const CREDITS_COOLDOWN: Duration = Duration::from_secs(300);

/// Delays before retrying a window the server is still backfilling. The first
/// retry waits 3 s, which is already longer than a render frame and safely
/// inside CoinWatch's 15 s charge-dedupe window, so retrying never shortens it.
const HISTORY_RETRY_DELAYS: [u64; 10] = [3, 5, 8, 12, 15, 20, 20, 20, 20, 20];

/// Delay before the next history retry while CryptoHFTData catches up.
/// `attempt` is zero-based; `None` once the retry budget is exhausted.
pub fn next_history_retry(attempt: u32) -> Option<Duration> {
    HISTORY_RETRY_DELAYS
        .get(attempt as usize)
        .copied()
        .map(Duration::from_secs)
}

/// Aggregated volume at one price row within a bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PriceFlow {
    pub price: f64,
    pub price_low: f64,
    pub price_high: f64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    /// Aggressor-unknown volume. Excluded from delta, included in totals.
    pub unknown_volume: f64,
    pub buy_count: u32,
    pub sell_count: u32,
}

impl PriceFlow {
    pub fn delta(&self) -> f64 {
        self.buy_volume - self.sell_volume
    }

    pub fn volume(&self) -> f64 {
        self.buy_volume + self.sell_volume + self.unknown_volume
    }
}

/// One aggregated bar. Volume is expressed in base units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowBar {
    pub timestamp: i64,
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub unknown_volume: f64,
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
        self.buy_volume + self.sell_volume + self.unknown_volume
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct FootprintRow {
    #[serde(default)]
    pub price_low: f64,
    #[serde(default)]
    pub price_high: f64,
    #[serde(default)]
    pub buy: f64,
    #[serde(default)]
    pub sell: f64,
    #[serde(default)]
    pub unknown: f64,
    #[serde(default)]
    pub buy_count: u32,
    #[serde(default)]
    pub sell_count: u32,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct FootprintBar {
    #[serde(default)]
    pub bar_start: i64,
    #[serde(default)]
    pub bar_end: i64,
    #[serde(default)]
    pub rows: Vec<FootprintRow>,
    #[serde(default)]
    pub poc: Option<f64>,
    #[serde(default)]
    pub vah: Option<f64>,
    #[serde(default)]
    pub val: Option<f64>,
    #[serde(default)]
    pub buy_volume: f64,
    #[serde(default)]
    pub sell_volume: f64,
    #[serde(default)]
    pub unknown_volume: f64,
    #[serde(default)]
    pub delta: f64,
    #[serde(default)]
    pub total_volume: f64,
    #[serde(default)]
    pub trades: u32,
    #[serde(default)]
    pub is_closed: bool,
}

/// `GET {base}/api/v1/orderflow/footprint` response body.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct FootprintHistory {
    #[serde(default)]
    pub instrument: String,
    #[serde(default)]
    pub exchange: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub timeframe: String,
    #[serde(default)]
    pub row_size: f64,
    #[serde(default)]
    pub since: i64,
    #[serde(default)]
    pub until: i64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub queued: bool,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub data: Vec<FootprintBar>,
}

/// Live `FootprintMessage` payload relayed on the gateway `/ws`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct FootprintMessage {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub timeframe: Option<String>,
    #[serde(default)]
    pub bar_start: i64,
    #[serde(default)]
    pub bar_end: i64,
    #[serde(default)]
    pub rows: Vec<FootprintRow>,
    #[serde(default)]
    pub poc: Option<f64>,
    #[serde(default)]
    pub vah: Option<f64>,
    #[serde(default)]
    pub val: Option<f64>,
    #[serde(default)]
    pub delta: f64,
    #[serde(default)]
    pub total_volume: f64,
    #[serde(default)]
    pub row_size: f64,
    #[serde(default)]
    pub is_closed: bool,
}

impl FootprintMessage {
    /// Accept a single object, an array of messages, or `{ data: ... }`.
    pub fn parse_items(value: &serde_json::Value) -> Vec<Self> {
        if let Some(array) = value.as_array() {
            return array
                .iter()
                .filter_map(|item| serde_json::from_value::<Self>(item.clone()).ok())
                .collect();
        }
        let inner = value
            .get("data")
            .or_else(|| value.get("payload"))
            .unwrap_or(value);
        if let Some(array) = inner.as_array() {
            return array
                .iter()
                .filter_map(|item| serde_json::from_value::<Self>(item.clone()).ok())
                .collect();
        }
        serde_json::from_value::<Self>(inner.clone())
            .ok()
            .into_iter()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Why the footprint may not be showing history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FootprintStatus {
    #[default]
    Idle,
    Loading,
    Ready,
    /// No API origin configured: Guest Mode has no footprint backend.
    Guest,
    /// The session token is missing or rejected (401).
    Unauthorized,
    /// CoinWatch refused the metered request (402).
    Credits,
    /// `1w` / `1M` cannot be aligned by fixed epoch arithmetic.
    UnsupportedTimeframe,
    /// Set when a history request fails with `until` in the future.
    Error(String),
    /// The server was still backfilling when the retry budget ran out.
    BackfillTimeout,
}

impl FootprintStatus {
    /// Short note shown in the chart overlay, if any.
    pub fn note(&self) -> Option<&str> {
        match self {
            Self::Guest => Some("Footprint history needs an API session. Sign in to load it."),
            Self::Unauthorized => Some("Footprint history needs sign-in. Retry after signing in."),
            Self::Credits => Some("Footprint history is paused: not enough credits."),
            Self::UnsupportedTimeframe => {
                Some("Footprint does not support weekly or monthly timeframes.")
            }
            Self::Error(message) => Some(message.as_str()),
            Self::BackfillTimeout => Some(
                "Footprint history is still backfilling from CryptoHFTData. Pan the chart to retry.",
            ),
            _ => None,
        }
    }
}

/// Point-in-time merged footprint state shared with the chart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FootprintSnapshot {
    /// Bars in ascending timestamp order.
    pub bars: Vec<FlowBar>,
    pub status: FootprintStatus,
    /// True when the history window touched the retention/lag edge.
    pub partial: bool,
    /// True while the server is backfilling this window from CryptoHFTData.
    pub queued: bool,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    bars: BTreeMap<i64, FlowBar>,
    /// Bars closed by history or a live `is_closed` snapshot. Immutable.
    closed: HashSet<i64>,
    /// Bars that have received a live snapshot. History must not mix rows in.
    live: HashSet<i64>,
    partial: bool,
    queued: bool,
    status: FootprintStatus,
    request_key: Option<String>,
    generation: u64,
    abort: Option<tokio::task::AbortHandle>,
    retry_after: Option<Instant>,
    cache: HashMap<String, (Instant, FootprintHistory)>,
}

impl State {
    fn ingest_live(&mut self, message: &FootprintMessage) {
        if message.bar_start <= 0 {
            return;
        }
        if message.is_closed && self.closed.contains(&message.bar_start) {
            return;
        }
        if message.is_closed {
            self.bars.insert(message.bar_start, flow_from_live(message));
            self.closed.insert(message.bar_start);
            self.live.remove(&message.bar_start);
        } else {
            // Never reopen a bar that history or a prior live snapshot closed.
            if self.closed.contains(&message.bar_start) {
                return;
            }
            self.bars.insert(message.bar_start, flow_from_live(message));
            self.live.insert(message.bar_start);
        }
        self.evict();
    }

    fn apply_history(&mut self, history: &FootprintHistory) {
        for bar in &history.data {
            if bar.bar_start <= 0 {
                continue;
            }
            let flow = flow_from_history(bar);
            let timestamp = flow.timestamp;
            if flow.is_closed {
                // Closed history replaces whatever we had for that bar.
                self.bars.insert(timestamp, flow);
                self.closed.insert(timestamp);
                self.live.remove(&timestamp);
            } else if !self.live.contains(&timestamp) && !self.closed.contains(&timestamp) {
                // An open history bar is only a fallback; live snapshots win.
                self.bars.insert(timestamp, flow);
            }
        }
        // One active viewport request is applied at a time (stale responses are
        // discarded by generation), so the newest window's flags win. A later
        // non-queued response therefore clears both `queued` and `partial` from
        // the server's assignment once coverage completes.
        self.partial = history.partial;
        self.queued = history.queued;
        self.status = FootprintStatus::Ready;
        self.evict();
    }

    /// Apply a response and cache it only when it is a complete window.
    /// Queued (server backfill in progress) and partial (retention edge)
    /// responses are transient and must not suppress the retry.
    fn accept_history(&mut self, key: &str, history: &FootprintHistory) {
        self.apply_history(history);
        if history.queued || history.partial {
            return;
        }
        let now = Instant::now();
        self.cache
            .insert(key.to_string(), (now + HISTORY_CACHE_TTL, history.clone()));
        // Keep the cache bounded; viewport keys churn while panning.
        if self.cache.len() > 64 {
            self.cache.retain(|_, (expires, _)| *expires > now);
        }
    }

    fn evict(&mut self) {
        while self.bars.len() > MAX_BARS {
            let Some(oldest) = self.bars.keys().next().copied() else {
                break;
            };
            self.bars.remove(&oldest);
            self.closed.remove(&oldest);
            self.live.remove(&oldest);
        }
    }

    fn snapshot(&self) -> FootprintSnapshot {
        FootprintSnapshot {
            bars: self.bars.values().cloned().collect(),
            status: self.status.clone(),
            partial: self.partial,
            queued: self.queued,
        }
    }
}

fn flow_from_history(bar: &FootprintBar) -> FlowBar {
    let mut price_rows: Vec<PriceFlow> = bar
        .rows
        .iter()
        .filter(|row| row.price_low.is_finite() && row.price_high.is_finite())
        .map(|row| PriceFlow {
            price: (row.price_low + row.price_high) / 2.0,
            price_low: row.price_low,
            price_high: row.price_high,
            buy_volume: row.buy.max(0.0),
            sell_volume: row.sell.max(0.0),
            unknown_volume: row.unknown.max(0.0),
            buy_count: row.buy_count,
            sell_count: row.sell_count,
        })
        .collect();
    price_rows.sort_by(|a, b| a.price_low.total_cmp(&b.price_low));

    let row_buy: f64 = price_rows.iter().map(|row| row.buy_volume).sum();
    let row_sell: f64 = price_rows.iter().map(|row| row.sell_volume).sum();
    let row_unknown: f64 = price_rows.iter().map(|row| row.unknown_volume).sum();
    let row_trades: u32 = price_rows
        .iter()
        .map(|row| row.buy_count.saturating_add(row.sell_count))
        .sum();

    FlowBar {
        timestamp: bar.bar_start,
        buy_volume: if bar.buy_volume > 0.0 {
            bar.buy_volume
        } else {
            row_buy
        },
        sell_volume: if bar.sell_volume > 0.0 {
            bar.sell_volume
        } else {
            row_sell
        },
        unknown_volume: if bar.unknown_volume > 0.0 {
            bar.unknown_volume
        } else {
            row_unknown
        },
        trades: if bar.trades > 0 {
            bar.trades
        } else {
            row_trades
        },
        price_rows,
        poc: bar.poc,
        vah: bar.vah,
        val: bar.val,
        total_volume: if bar.total_volume > 0.0 {
            bar.total_volume
        } else {
            bar.buy_volume + bar.sell_volume + bar.unknown_volume
        },
        row_size: 0.0,
        is_closed: bar.is_closed,
    }
}

fn flow_from_live(message: &FootprintMessage) -> FlowBar {
    let mut price_rows: Vec<PriceFlow> = message
        .rows
        .iter()
        .filter(|row| row.price_low.is_finite() && row.price_high.is_finite())
        .map(|row| PriceFlow {
            price: (row.price_low + row.price_high) / 2.0,
            price_low: row.price_low,
            price_high: row.price_high,
            buy_volume: row.buy.max(0.0),
            sell_volume: row.sell.max(0.0),
            unknown_volume: row.unknown.max(0.0),
            buy_count: row.buy_count,
            sell_count: row.sell_count,
        })
        .collect();
    price_rows.sort_by(|a, b| a.price_low.total_cmp(&b.price_low));

    let buy_volume: f64 = price_rows.iter().map(|row| row.buy_volume).sum();
    let sell_volume: f64 = price_rows.iter().map(|row| row.sell_volume).sum();
    let unknown_volume: f64 = price_rows.iter().map(|row| row.unknown_volume).sum();
    let trades = price_rows
        .iter()
        .map(|row| row.buy_count.saturating_add(row.sell_count))
        .sum();

    FlowBar {
        timestamp: message.bar_start,
        buy_volume,
        sell_volume,
        unknown_volume,
        trades,
        price_rows,
        poc: message.poc,
        vah: message.vah,
        val: message.val,
        total_volume: if message.total_volume > 0.0 {
            message.total_volume
        } else {
            buy_volume + sell_volume + unknown_volume
        },
        row_size: message.row_size,
        is_closed: message.is_closed,
    }
}

// ---------------------------------------------------------------------------
// Feed
// ---------------------------------------------------------------------------

/// Shared footprint feed driven by historical REST and the live gateway feed.
#[derive(Clone)]
pub struct FootprintFeed {
    inner: Arc<Mutex<State>>,
    version: Arc<AtomicU64>,
    config: BridgeConfig,
    timeframe: String,
    cancel: CancellationToken,
    task: Arc<JoinHandle<()>>,
}

impl FootprintFeed {
    pub fn new(receiver: broadcast::Receiver<Arc<FootprintMessage>>, config: BridgeConfig) -> Self {
        let inner = Arc::new(Mutex::new(State::default()));
        let version = Arc::new(AtomicU64::new(0));
        let cancel = CancellationToken::new();
        let timeframe = config.timeframe.clone();

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
                            Ok(message) => {
                                if let Ok(mut state) = task_inner.lock() {
                                    state.ingest_live(&message);
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
            config,
            timeframe,
            cancel,
            task: Arc::new(task),
        }
    }

    /// Cheap snapshot for the render thread.
    pub fn snapshot(&self) -> FootprintSnapshot {
        self.inner
            .lock()
            .map(|state| state.snapshot())
            .unwrap_or_default()
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn version_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.version)
    }

    pub fn timeframe(&self) -> &str {
        &self.timeframe
    }

    /// Feed the live trade stream into the same bars. Only used by tests and
    /// legacy callers that aggregate locally; the live footprint channel is
    /// authoritative in production.
    pub fn ingest_trade(&self, trade: &UnifiedTick, timeframe_ms: i64) {
        if trade.side == crate::data::tick::TradeSide::Unknown {
            return;
        }
        let bar_start =
            trade.timestamp_ms.div_euclid(timeframe_ms.max(1_000)) * timeframe_ms.max(1_000);
        if let Ok(mut state) = self.inner.lock() {
            if state.closed.contains(&bar_start) {
                return;
            }
            let bar = state.bars.entry(bar_start).or_insert_with(|| FlowBar {
                timestamp: bar_start,
                ..FlowBar::default()
            });
            let amount = if trade.amount.is_finite() && trade.amount > 0.0 {
                trade.amount
            } else {
                0.0
            };
            if trade.side == crate::data::tick::TradeSide::Buy {
                bar.buy_volume += amount;
            } else {
                bar.sell_volume += amount;
            }
            bar.total_volume = bar.buy_volume + bar.sell_volume + bar.unknown_volume;
            bar.trades = bar.trades.saturating_add(1);
            state.live.insert(bar_start);
        }
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Request history for the visible window. Debounced and de-duplicated by
    /// `(exchange, symbol, timeframe, since, until, row_size)`.
    pub fn request_visible(&self, since: i64, until: i64, limit: usize, row_size: f64) {
        if self.timeframe_is_unsupported() {
            if let Ok(mut state) = self.inner.lock() {
                if state.status != FootprintStatus::UnsupportedTimeframe {
                    state.status = FootprintStatus::UnsupportedTimeframe;
                    drop(state);
                    self.version.fetch_add(1, Ordering::Relaxed);
                }
            }
            return;
        }
        if since <= 0 || until <= since || !row_size.is_finite() || row_size <= 0.0 {
            return;
        }

        let limit = limit.clamp(1, 2_000);
        let row_key = (row_size * 100_000_000.0).round() as i64;
        let key = format!(
            "{}:{}:{}:{since}:{until}:{row_key}:{limit}",
            self.config.exchange.to_lowercase(),
            self.config.symbol,
            self.timeframe
        );

        let now = Instant::now();
        let (generation, config) = {
            let Ok(mut state) = self.inner.lock() else {
                return;
            };
            if state.request_key.as_deref() == Some(key.as_str()) {
                return;
            }
            if state.retry_after.is_some_and(|at| at > now) {
                return;
            }
            // A different viewport supersedes any in-flight fetch, including a
            // queued-window retry loop still sleeping between attempts.
            if let Some(abort) = state.abort.take() {
                abort.abort();
            }
            if let Some(entry) = state.cache.get(&key) {
                if entry.0 > now {
                    let history = entry.1.clone();
                    state.request_key = Some(key.clone());
                    state.apply_history(&history);
                    drop(state);
                    self.version.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
            state.request_key = Some(key.clone());
            state.generation = state.generation.wrapping_add(1);
            if state.status != FootprintStatus::Ready {
                state.status = FootprintStatus::Loading;
            }
            (state.generation, self.config.clone())
        };
        self.version.fetch_add(1, Ordering::Relaxed);

        let inner = Arc::clone(&self.inner);
        let version = Arc::clone(&self.version);
        let timeframe = self.timeframe.clone();
        let request_key = key.clone();
        let handle = tokio_runtime().spawn(async move {
            tokio::time::sleep(HISTORY_DEBOUNCE).await;
            if !generation_matches(&inner, generation) {
                return;
            }

            let mut attempt: u32 = 0;
            loop {
                let outcome =
                    fetch_footprint_history(&config, &timeframe, since, until, row_size, limit)
                        .await;

                let retry = {
                    let Ok(mut state) = inner.lock() else {
                        return;
                    };
                    if state.generation != generation
                        || state.request_key.as_deref() != Some(&request_key)
                    {
                        return;
                    }
                    let retry = apply_history_outcome(&mut state, &request_key, attempt, outcome);
                    drop(state);
                    version.fetch_add(1, Ordering::Relaxed);
                    retry
                };

                let Some(delay) = retry else {
                    return;
                };
                attempt = attempt.saturating_add(1);
                // Sleep without holding the state lock so live ingest and
                // render snapshots keep flowing during a server backfill.
                tokio::time::sleep(delay).await;
                if !generation_matches(&inner, generation) {
                    return;
                }
            }
        });
        if let Ok(mut state) = self.inner.lock() {
            state.abort = Some(handle.abort_handle());
        }
    }

    fn timeframe_is_unsupported(&self) -> bool {
        matches!(self.timeframe.as_str(), "1w" | "1M")
    }
}

impl Drop for FootprintFeed {
    fn drop(&mut self) {
        // Only cancel when the last clone goes away.
        if Arc::strong_count(&self.task) == 1 {
            self.cancel.cancel();
        }
    }
}

fn generation_matches(inner: &Arc<Mutex<State>>, generation: u64) -> bool {
    inner
        .lock()
        .map(|state| state.generation == generation)
        .unwrap_or(false)
}

/// Apply one history fetch result. Returns the delay before retrying a queued
/// (server-side backfill) window, or `None` when this request is finished.
///
/// A queued response still applies its bars so the live/lag portion is
/// visible. Errors never loop, so 401/402 cooldowns cannot turn into a retry
/// storm.
fn apply_history_outcome(
    state: &mut State,
    request_key: &str,
    attempt: u32,
    outcome: Result<FootprintHistory, FootprintFetchError>,
) -> Option<Duration> {
    match outcome {
        Ok(history) => {
            let queued = history.queued;
            state.accept_history(request_key, &history);
            state.retry_after = None;
            if !queued {
                return None;
            }
            match next_history_retry(attempt) {
                Some(delay) => Some(delay),
                None => {
                    // The backfill outlived the retry budget. Stop polling and
                    // leave a note; panning to a new window starts fresh.
                    state.queued = false;
                    state.status = FootprintStatus::BackfillTimeout;
                    None
                }
            }
        }
        Err(error) => {
            state.status = error.status;
            state.retry_after = error.cooldown.map(|cooldown| Instant::now() + cooldown);
            debug!("footprint: history request failed: {}", error.message);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// Error returned by the metered footprint history endpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct FootprintFetchError {
    pub status: FootprintStatus,
    pub message: String,
    pub cooldown: Option<Duration>,
}

impl FootprintFetchError {
    fn new(status: FootprintStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            cooldown: None,
        }
    }

    fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = Some(cooldown);
        self
    }
}

/// Build the CoinWatch history URL. Symbol keeps `:` verbatim (swaps) while
/// transport-unsafe characters are still percent-encoded.
pub fn footprint_history_url(
    config: &BridgeConfig,
    timeframe: &str,
    since: i64,
    until: i64,
    row_size: f64,
    limit: usize,
) -> String {
    format!(
        "{}/api/v1/orderflow/footprint?exchange={}&symbol={}&type={}&timeframe={}&since={since}&until={until}&row_size={}&limit={}&module={}&source=native",
        config.rest_base_url.trim_end_matches('/'),
        urlencoding::encode(&config.exchange.to_lowercase()),
        encode_symbol(&config.symbol),
        market_type_for_symbol(&config.symbol),
        urlencoding::encode(timeframe),
        row_size,
        limit,
        urlencoding::encode(&format!("Chart · {} ({})", config.symbol, timeframe)),
    )
}

/// `type` mirrors the unified-symbol market class. Perp/swap symbols carry a
/// `:` settlement suffix (`BTC/USDT:USDT`); everything else is spot.
fn market_type_for_symbol(symbol: &str) -> &'static str {
    if symbol.contains(':') { "swap" } else { "spot" }
}

fn encode_symbol(symbol: &str) -> String {
    urlencoding::encode(symbol)
        .replace("%3A", ":")
        .replace("%2F", "/")
}

/// Fetch historical footprint bars for a bounded viewport window.
pub async fn fetch_footprint_history(
    config: &BridgeConfig,
    timeframe: &str,
    since: i64,
    until: i64,
    row_size: f64,
    limit: usize,
) -> Result<FootprintHistory, FootprintFetchError> {
    if config.rest_base_url.trim().is_empty() {
        return Err(FootprintFetchError::new(
            FootprintStatus::Guest,
            "No API service URL configured",
        ));
    }
    if matches!(timeframe, "1w" | "1M") {
        return Err(FootprintFetchError::new(
            FootprintStatus::UnsupportedTimeframe,
            "Footprint does not support weekly or monthly timeframes",
        ));
    }

    let token = crate::data::session_store::load_session()
        .ok()
        .flatten()
        .map(|session| session.access_token)
        .or_else(|| config.access_token.clone());
    let Some(token) = token else {
        return Err(FootprintFetchError::new(
            FootprintStatus::Unauthorized,
            "Sign in to load footprint history",
        )
        .with_cooldown(UNAUTHORIZED_COOLDOWN));
    };

    let url = footprint_history_url(config, timeframe, since, until, row_size, limit);
    debug!("footprint: GET {}", url.split('?').next().unwrap_or(&url));

    let response = reqwest::Client::new()
        .get(&url)
        .header("Origin", &config.rest_base_url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| {
            FootprintFetchError::new(
                FootprintStatus::Error(format!("Footprint history request failed: {error}")),
                error.to_string(),
            )
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FootprintFetchError::new(
            FootprintStatus::Unauthorized,
            "Footprint history needs sign-in",
        )
        .with_cooldown(UNAUTHORIZED_COOLDOWN));
    }
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("Not enough credits for footprint history");
        let balance = body
            .get("balance")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        let required = body
            .get("required")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        crate::data::credits::CreditStore::record(
            "Footprint",
            message,
            balance,
            required,
            Some(&config.rest_base_url),
        );
        return Err(FootprintFetchError::new(FootprintStatus::Credits, message)
            .with_cooldown(CREDITS_COOLDOWN));
    }
    if !status.is_success() {
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        let message = body
            .get("message")
            .or_else(|| body.get("error"))
            .and_then(|value| value.as_str())
            .unwrap_or("Footprint history request failed");
        return Err(FootprintFetchError::new(
            FootprintStatus::Error(format!("Footprint history: {message}")),
            message,
        ));
    }

    response.json::<FootprintHistory>().await.map_err(|error| {
        FootprintFetchError::new(
            FootprintStatus::Error(format!("Footprint history response parse error: {error}")),
            error.to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_bar(bar_start: i64, closed: bool) -> FootprintBar {
        FootprintBar {
            bar_start,
            bar_end: bar_start + 60_000,
            rows: vec![FootprintRow {
                price_low: 100.0,
                price_high: 101.0,
                buy: 3.0,
                sell: 1.0,
                unknown: 2.0,
                buy_count: 3,
                sell_count: 1,
            }],
            poc: Some(100.5),
            vah: Some(101.0),
            val: Some(100.0),
            buy_volume: 3.0,
            sell_volume: 1.0,
            unknown_volume: 2.0,
            delta: 2.0,
            total_volume: 6.0,
            trades: 4,
            is_closed: closed,
        }
    }

    fn live_message(bar_start: i64, buy: f64, sell: f64, closed: bool) -> FootprintMessage {
        FootprintMessage {
            kind: Some("footprint".into()),
            timeframe: Some("1m".into()),
            bar_start,
            bar_end: bar_start + 60_000,
            rows: vec![FootprintRow {
                price_low: 100.0,
                price_high: 101.0,
                buy,
                sell,
                ..FootprintRow::default()
            }],
            poc: Some(100.5),
            vah: Some(101.0),
            val: Some(100.0),
            delta: buy - sell,
            total_volume: buy + sell,
            row_size: 1.0,
            is_closed: closed,
        }
    }

    fn empty(config: BridgeConfig) -> FootprintFeed {
        let (_tx, rx) = broadcast::channel(16);
        FootprintFeed::new(rx, config)
    }

    fn queued_history(partial: bool) -> FootprintHistory {
        FootprintHistory {
            queued: true,
            partial,
            data: vec![history_bar(60_000, false)],
            ..FootprintHistory::default()
        }
    }

    fn complete_history(partial: bool) -> FootprintHistory {
        FootprintHistory {
            queued: false,
            partial,
            data: vec![history_bar(60_000, true)],
            ..FootprintHistory::default()
        }
    }

    fn config() -> BridgeConfig {
        BridgeConfig {
            rest_base_url: String::new(),
            exchange: "binance".into(),
            symbol: "BTC/USDT".into(),
            timeframe: "1m".into(),
            access_token: None,
            ..BridgeConfig::default_dev("binance", "BTC/USDT")
        }
    }

    #[test]
    fn history_replaces_closed_bars_and_protects_them_from_live() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.apply_history(&FootprintHistory {
                data: vec![history_bar(60_000, true), history_bar(120_000, false)],
                partial: true,
                ..FootprintHistory::default()
            });

            // Live snapshot replaces the in-progress bar.
            state.ingest_live(&live_message(120_000, 9.0, 1.0, false));
            // A later snapshot for a history-closed bar must be ignored.
            state.ingest_live(&live_message(60_000, 99.0, 99.0, true));
        }

        let snapshot = feed.snapshot();
        assert!(snapshot.partial);
        assert_eq!(snapshot.status, FootprintStatus::Ready);
        let closed = snapshot
            .bars
            .iter()
            .find(|bar| bar.timestamp == 60_000)
            .unwrap();
        assert_eq!(closed.buy_volume, 3.0);
        assert!(closed.is_closed);
        let open = snapshot
            .bars
            .iter()
            .find(|bar| bar.timestamp == 120_000)
            .unwrap();
        assert_eq!(open.buy_volume, 9.0);
        assert_eq!(open.sell_volume, 1.0);
        assert!(!open.is_closed);
    }

    #[test]
    fn live_closed_snapshot_is_immutable() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.ingest_live(&live_message(60_000, 5.0, 1.0, true));
            state.ingest_live(&live_message(60_000, 50.0, 10.0, false));
        }

        let snapshot = feed.snapshot();
        let bar = &snapshot.bars[0];
        assert_eq!(bar.buy_volume, 5.0);
        assert!(bar.is_closed);
    }

    #[test]
    fn history_does_not_mix_rows_into_a_live_open_bar() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.ingest_live(&live_message(120_000, 9.0, 1.0, false));
            state.apply_history(&FootprintHistory {
                data: vec![history_bar(120_000, false)],
                ..FootprintHistory::default()
            });
        }

        let snapshot = feed.snapshot();
        let bar = &snapshot.bars[0];
        assert_eq!(bar.buy_volume, 9.0);
        assert_eq!(bar.price_rows.len(), 1);
        assert_eq!(bar.price_rows[0].buy_volume, 9.0);
    }

    #[test]
    fn unknown_volume_round_trips_and_delta_excludes_it() {
        let history: FootprintHistory = serde_json::from_str(
            r#"{
                "exchange":"binance","symbol":"BTC/USDT","timeframe":"1m","row_size":1.0,
                "since":0,"until":60000,"source":"clickhouse","queued":false,"partial":false,
                "data":[{
                    "bar_start":0,"bar_end":60000,
                    "rows":[{"price_low":100,"price_high":101,"buy":2,"sell":1,"unknown":5,"buy_count":2,"sell_count":1}],
                    "poc":100.5,"vah":101,"val":100,
                    "buy_volume":2,"sell_volume":1,"unknown_volume":5,
                    "delta":1,"total_volume":8,"trades":3,"is_closed":true
                }]
            }"#,
        )
        .unwrap();
        let flow = flow_from_history(&history.data[0]);
        assert_eq!(flow.delta(), 1.0);
        assert_eq!(flow.volume(), 8.0);
        assert_eq!(flow.price_rows[0].unknown_volume, 5.0);
    }

    #[test]
    fn live_payload_accepts_single_and_array_shapes() {
        let single =
            serde_json::json!({"type":"footprint","timeframe":"1m","bar_start":1,"rows":[]});
        assert_eq!(FootprintMessage::parse_items(&single).len(), 1);
        let array = serde_json::json!([
            {"type":"footprint","bar_start":1,"rows":[]},
            {"type":"footprint","bar_start":2,"rows":[]}
        ]);
        assert_eq!(FootprintMessage::parse_items(&array).len(), 2);
    }

    #[test]
    fn history_url_preserves_the_swap_colon_and_encodes_the_range() {
        let mut cfg = config();
        cfg.rest_base_url = "https://staging.coinwat.ch/".into();
        cfg.symbol = "BTC/USDT:USDT".into();
        let url = footprint_history_url(&cfg, "1m", 1_000, 2_000, 0.5, 20);
        assert!(
            url.starts_with("https://staging.coinwat.ch/api/v1/orderflow/footprint?"),
            "{url}"
        );
        assert!(url.contains("symbol=BTC/USDT:USDT"), "{url}");
        assert!(url.contains("type=swap"), "{url}");
        assert!(url.contains("timeframe=1m"), "{url}");
        assert!(url.contains("since=1000"), "{url}");
        assert!(url.contains("until=2000"), "{url}");
        assert!(url.contains("row_size=0.5"), "{url}");
        assert!(url.contains("limit=20"), "{url}");
        assert!(url.contains("source=native"), "{url}");
    }

    #[test]
    fn history_url_marks_spot_symbols_as_spot() {
        let mut cfg = config();
        cfg.rest_base_url = "https://staging.coinwat.ch".into();
        cfg.symbol = "BTC/USDT".into();
        let url = footprint_history_url(&cfg, "1m", 1_000, 2_000, 0.5, 20);
        assert!(url.contains("symbol=BTC/USDT"), "{url}");
        assert!(url.contains("type=spot"), "{url}");
        assert!(!url.contains("type=swap"), "{url}");
    }

    #[test]
    fn history_retry_schedule_backs_off_then_stops() {
        let delays: Vec<u64> = (0..HISTORY_RETRY_DELAYS.len() as u32)
            .filter_map(|attempt| next_history_retry(attempt).map(|delay| delay.as_secs()))
            .collect();
        assert_eq!(delays, vec![3, 5, 8, 12, 15, 20, 20, 20, 20, 20]);
        assert_eq!(next_history_retry(0), Some(Duration::from_secs(3)));
        assert_eq!(next_history_retry(9), Some(Duration::from_secs(20)));
        assert_eq!(next_history_retry(10), None);
    }

    #[test]
    fn queued_history_is_applied_but_not_cached() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.accept_history("window", &queued_history(true));
            assert!(
                state.cache.is_empty(),
                "queued responses must not be cached"
            );
        }

        let snapshot = feed.snapshot();
        assert!(snapshot.queued);
        assert!(snapshot.partial);
        assert_eq!(snapshot.status, FootprintStatus::Ready);
        assert_eq!(snapshot.bars.len(), 1, "the live/lag portion still renders");
    }

    #[test]
    fn partial_history_is_not_cached() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.accept_history("window", &complete_history(true));
            assert!(
                state.cache.is_empty(),
                "a partial window must be retried on the next viewport"
            );
        }
        let snapshot = feed.snapshot();
        assert!(snapshot.partial);
        assert!(!snapshot.queued);
    }

    #[test]
    fn complete_history_is_cached() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.accept_history("window", &complete_history(false));
            let (expires, cached) = state.cache.get("window").expect("complete window cached");
            assert!(*expires > Instant::now());
            assert!(!cached.queued);
            assert!(!cached.partial);
        }
        assert!(!feed.snapshot().partial);
        assert!(!feed.snapshot().queued);
    }

    #[test]
    fn queued_fetch_schedules_retries_and_credits_errors_stop() {
        let mut state = State {
            generation: 1,
            request_key: Some("window".into()),
            ..State::default()
        };

        let retry = apply_history_outcome(&mut state, "window", 0, Ok(queued_history(false)));
        assert_eq!(retry, Some(Duration::from_secs(3)));
        assert!(state.queued);

        // 401/402 style errors set the cooldown and end the retry loop.
        let error = FootprintFetchError::new(FootprintStatus::Credits, "no credits")
            .with_cooldown(CREDITS_COOLDOWN);
        let retry = apply_history_outcome(&mut state, "window", 1, Err(error));
        assert_eq!(retry, None);
        assert_eq!(state.status, FootprintStatus::Credits);
        assert!(state.retry_after.is_some());
    }

    #[test]
    fn exhausted_queued_retries_leave_a_note() {
        let mut state = State {
            generation: 1,
            request_key: Some("window".into()),
            ..State::default()
        };

        let retry = apply_history_outcome(&mut state, "window", 9, Ok(queued_history(true)));
        assert_eq!(retry, Some(Duration::from_secs(20)));

        let retry = apply_history_outcome(&mut state, "window", 10, Ok(queued_history(true)));
        assert_eq!(retry, None);
        assert_eq!(state.status, FootprintStatus::BackfillTimeout);
        assert!(
            !state.queued,
            "polling stopped, so the queued note is cleared"
        );
        assert!(state.status.note().is_some());
    }

    #[test]
    fn a_complete_window_clears_previous_queued_and_partial_flags() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.accept_history("window", &queued_history(true));
        }
        let snapshot = feed.snapshot();
        assert!(snapshot.queued);
        assert!(snapshot.partial);

        {
            let mut state = feed.inner.lock().unwrap();
            state.accept_history("window", &complete_history(false));
        }
        let snapshot = feed.snapshot();
        assert!(
            !snapshot.queued,
            "the final response clears the queued note"
        );
        assert!(
            !snapshot.partial,
            "the final response clears the partial note"
        );
    }

    #[test]
    fn a_complete_window_clears_a_previous_partial_flag() {
        let feed = empty(config());
        {
            let mut state = feed.inner.lock().unwrap();
            state.apply_history(&FootprintHistory {
                data: vec![history_bar(60_000, true)],
                partial: true,
                ..FootprintHistory::default()
            });
        }
        assert!(feed.snapshot().partial);

        {
            let mut state = feed.inner.lock().unwrap();
            state.apply_history(&FootprintHistory {
                data: vec![history_bar(120_000, true)],
                partial: false,
                ..FootprintHistory::default()
            });
        }
        assert!(!feed.snapshot().partial);
    }
}
