/// CoinWatch realtime gateway client.
///
/// Connects to CoinWatch's `/ws` facade and negotiates the same v2 protocol the
/// web dashboard uses (`hello` / `sub` / `snapshot` / `delta` / `hb`). Both the
/// native client and the browser share one endpoint; there is no legacy
/// `?legacy=1` path.
///
/// Channels:
/// - `ticker`    → [`UnifiedTick`] stream (real-time price updates)
/// - `trades`    → [`UnifiedTick`] stream (may arrive as batches)
/// - `orderbook` → [`OrderBook`] snapshot stream
/// - `ohlcv`     → [`OhlcvCandle`] stream
///
/// OHLCV history is still fetched once via REST on the first connection so the
/// chart has more context than the gateway's snapshot; live candle updates then
/// merge on top. Ticker, book, and trade snapshots come from the gateway.
///
/// Dropping the handle cancels the supervision loop and closes the connection.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::data::book_history::{parse_orderbook_history, OrderbookHistory};
use crate::data::client::{service_base_url, tokio_runtime};
use crate::data::models::OhlcvCandle;
use crate::data::orderbook::{OrderBook, PriceLevel};
use crate::data::orderflow_feed::{
    DerivedDelta, DerivedFlowEvent, DerivedFootprint, DerivedProfile
};
use crate::data::tick::{TradeSide, UnifiedTick};
use crate::data::trade_history::{TradeHistoryError, TradeHistoryPage, parse_trade_history};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Depth of the trade broadcast ring buffer.
const BROADCAST_CAPACITY: usize = 1024;

/// Depth of the order book broadcast ring buffer.
const OB_BROADCAST_CAPACITY: usize = 64;

/// Depth of the OHLCV broadcast ring buffer.
const OHLCV_BROADCAST_CAPACITY: usize = 64;

/// Delay between WebSocket reconnection attempts (ms).
const RECONNECT_DELAY_MS: u64 = 3_000;
const HISTORY_CACHE_MAX_ENTRIES: usize = 200;

#[derive(Clone)]
struct HistoryCacheEntry {
    candles: Vec<OhlcvCandle>,
    expires_at: Instant,
}

static HISTORY_REQUEST_CACHE: OnceLock<Mutex<HashMap<String, HistoryCacheEntry>>> = OnceLock::new();

fn history_request_cache() -> &'static Mutex<HashMap<String, HistoryCacheEntry>> {
    HISTORY_REQUEST_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Historical L2/L1 responses are larger than candle pages, so keep a smaller
/// bounded cache than the OHLCV one.
const BOOK_HISTORY_CACHE_MAX_ENTRIES: usize = 100;

#[derive(Clone)]
struct BookHistoryCacheEntry {
    history: OrderbookHistory,
    expires_at: Instant,
}

static BOOK_HISTORY_REQUEST_CACHE: OnceLock<Mutex<HashMap<String, BookHistoryCacheEntry>>> =
    OnceLock::new();

fn book_history_request_cache() -> &'static Mutex<HashMap<String, BookHistoryCacheEntry>> {
    BOOK_HISTORY_REQUEST_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Raw-trade history pages are capped at 20k rows, but still larger than book
/// samples; keep a bounded per-credential cache like the L2 route.
const TRADE_HISTORY_CACHE_MAX_ENTRIES: usize = 100;

#[derive(Clone)]
struct TradeHistoryCacheEntry {
    page: TradeHistoryPage,
    expires_at: Instant,
}

static TRADE_HISTORY_REQUEST_CACHE: OnceLock<Mutex<HashMap<String, TradeHistoryCacheEntry>>> =
    OnceLock::new();

fn trade_history_request_cache() -> &'static Mutex<HashMap<String, TradeHistoryCacheEntry>> {
    TRADE_HISTORY_REQUEST_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Failure modes the chart needs to distinguish for the liquidity history note.
#[derive(Clone, Debug, thiserror::Error)]
pub enum BookHistoryError {
    /// Missing or expired credentials; the live feed still works.
    #[error("sign-in required")]
    Auth,
    /// The account ran out of credits.
    #[error("{0}")]
    Credits(String),
    /// The whole requested window is inside the server's 1h lag.
    #[error("data is not available yet")]
    NotAvailable,
    /// Network, configuration, parse, or other failure.
    #[error("{0}")]
    Other(String),
}

#[derive(Clone)]
enum TickerCacheState {
    Ready(Instant, serde_json::Value),
    InFlight(tokio::sync::watch::Receiver<Option<serde_json::Value>>),
}

static TICKER_REQUEST_CACHE: OnceLock<Mutex<HashMap<String, TickerCacheState>>> = OnceLock::new();

pub async fn fetch_ticker_snapshot(config: &BridgeConfig, module: &str) -> Option<serde_json::Value> {
    if config.rest_base_url.trim().is_empty() {
        return None;
    }
    let cache_key = format!(
        "{}:{}:{}",
        config.rest_base_url,
        config.exchange.to_lowercase(),
        crate::data::trade_store::normalize_symbol(&config.symbol)
    );
    let cache = TICKER_REQUEST_CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    let (tx_to_fetch, mut rx) = {
        let mut guard = cache.lock().unwrap();
        match guard.get(&cache_key) {
            Some(TickerCacheState::Ready(instant, val))
                if instant.elapsed() < Duration::from_millis(2000) =>
            {
                debug!("gateway: reusing cached ticker snapshot for {}", cache_key);
                return Some(val.clone());
            }
            Some(TickerCacheState::InFlight(rx)) => {
                debug!("gateway: awaiting in-flight ticker snapshot for {}", cache_key);
                (None, rx.clone())
            }
            _ => {
                let (tx, rx) = tokio::sync::watch::channel(None);
                guard.insert(cache_key.clone(), TickerCacheState::InFlight(rx.clone()));
                (Some(tx), rx)
            }
        }
    };

    if let Some(tx) = tx_to_fetch {
        let url = config.ticker_snapshot_url(module);
        let token = crate::data::session_store::load_session()
            .ok()
            .flatten()
            .map(|s| s.access_token)
            .or_else(|| config.access_token.clone());
        let mut req = reqwest::Client::new().get(&url).header("Origin", &config.rest_base_url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }

        let fetch_res = match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let val = resp.json::<serde_json::Value>().await.ok();
                if let Some(ref v) = val {
                    if v.get("error").and_then(|e| e.as_str()) == Some("insufficient_credits") {
                        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("Not enough credits");
                        let balance = v.get("balance").and_then(|b| b.as_i64()).unwrap_or(0);
                        let required = v.get("required").and_then(|r| r.as_i64()).unwrap_or(0);
                        crate::data::credits::CreditStore::record(
                            module,
                            msg,
                            balance,
                            required,
                            Some(&config.rest_base_url),
                        );
                    }
                }
                val
            }
            Ok(resp) => {
                let status = resp.status();
                let val: serde_json::Value = resp.json().await.unwrap_or_default();
                if status.as_u16() == 402 || val.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits") {
                    let msg = val.get("message").and_then(|v| v.as_str()).unwrap_or("Not enough credits");
                    let balance = val.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
                    let required = val.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
                    crate::data::credits::CreditStore::record(
                        module,
                        msg,
                        balance,
                        required,
                        Some(&config.rest_base_url),
                    );
                    warn!("gateway: ticker bootstrap returned 402 (insufficient credits) for {module}: {msg}");
                } else {
                    warn!("gateway: ticker bootstrap returned HTTP {}", status);
                }
                None
            }
            Err(e) => {
                warn!("gateway: ticker bootstrap failed: {}", e);
                None
            }
        };

        {
            let mut guard = cache.lock().unwrap();
            if let Some(ref val) = fetch_res {
                guard.insert(
                    cache_key,
                    TickerCacheState::Ready(Instant::now(), val.clone()),
                );
            } else {
                guard.remove(&cache_key);
            }
        }
        let _ = tx.send(fetch_res.clone());
        return fetch_res;
    }

    if rx.changed().await.is_ok() {
        rx.borrow().clone()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Runtime configuration for the coinwat.ch gateway client.
#[derive(Clone, Debug)]
pub struct BridgeConfig {
    /// Base URL of the REST gateway (e.g. `"https://coinwat.ch"`).
    /// The WebSocket URL is derived from this by replacing the scheme.
    pub rest_base_url: String,
    /// Exchange ID (e.g. `"binance"`, `"binanceusdm"`).
    pub exchange: String,
    /// Trading pair symbol (e.g. `"BTC/USDT"`).
    pub symbol: String,
    /// Use sandbox/testnet.
    pub sandbox: bool,
    /// Kline timeframe (e.g. `"1m"`, `"5m"`, `"1h"`).
    pub timeframe: String,
    /// Number of historical candles to fetch via REST on startup.
    pub ohlcv_limit: u32,
    /// Optional CoinWatch API bearer token for protected REST endpoints.
    pub access_token: Option<String>,
    /// Only subscribe to channels required by the owning dashboard pane.
    pub ticker: bool,
    pub trades: bool,
    pub orderbook: bool,
    pub kline: bool,
    pub fetch_history: bool,
    /// Trade snapshot size requested at subscribe time.
    pub trade_limit: u32,
    /// Request the trade snapshot from the current bar's open when connecting.
    pub trade_since_current_bar: bool,
    /// Order-book depth requested at subscribe time.
    pub orderbook_depth: u32,
    /// Subscribe to derived delta bars.
    pub delta: bool,
    /// Subscribe to derived footprint bars.
    pub footprint: bool,
    /// Subscribe to the derived session profile.
    pub profile: bool,
}

impl BridgeConfig {
    pub fn default_dev(exchange: &str, symbol: &str) -> Self {
        Self {
            rest_base_url: service_base_url(),
            exchange: exchange.to_string(),
            symbol: symbol.to_string(),
            sandbox: false,
            timeframe: "1m".to_string(),
            ohlcv_limit: 500,
            access_token: None,
            ticker: true,
            trades: true,
            orderbook: true,
            kline: true,
            fetch_history: true,
            trade_limit: 100,
            trade_since_current_bar: false,
            orderbook_depth: 20,
            delta: false,
            footprint: false,
            profile: false,
        }
    }

    pub fn production(exchange: &str, symbol: &str) -> Self {
        Self::default_dev(exchange, symbol)
    }

    /// Gateway market stream endpoint. Override only for local gateway work.
    pub fn ws_url(&self) -> String {
        std::env::var("FINTER_WS_URL").unwrap_or_else(|_| {
            let base = self.rest_base_url.trim_end_matches('/');
            if base.is_empty() {
                String::new()
            } else if let Some(origin) = base.strip_prefix("https://") {
                format!("wss://{origin}/ws")
            } else if let Some(origin) = base.strip_prefix("http://") {
                format!("ws://{origin}/ws")
            } else if base.contains("://") {
                format!("{base}/ws")
            } else {
                format!("wss://{base}/ws")
            }
        })
    }

    /// REST URL for the initial 200-candle history fetch.
    pub fn timeframe_to_ms(timeframe: &str) -> i64 {
        let mut num = 0;
        let mut unit = "";
        if let Some(idx) = timeframe.find(|c: char| c.is_alphabetic()) {
            let (n, u) = timeframe.split_at(idx);
            num = n.parse::<i64>().unwrap_or(1);
            unit = u;
        }
        match unit {
            "s" => num * 1000,
            "m" => num * 60 * 1000,
            "h" => num * 60 * 60 * 1000,
            "d" => num * 24 * 60 * 60 * 1000,
            "w" => num * 7 * 24 * 60 * 60 * 1000,
            "M" => num * 30 * 24 * 60 * 60 * 1000,
            _ => 60 * 1000, // fallback 1m
        }
    }

    pub fn ticker_snapshot_url(&self, module: &str) -> String {
        format!(
            "{}/api/v1/ticker?exchange={}&symbol={}&module={}&source=native",
            self.rest_base_url,
            urlencoding::encode(&self.exchange.to_lowercase()),
            urlencoding::encode(&self.symbol),
            urlencoding::encode(&format!("{} · {}", module, self.symbol)),
        )
    }

    pub fn ohlcv_history_url(&self, until: Option<i64>) -> String {
        let mut url = format!(
            "{}/api/v1/ohlcv?exchange={}&symbol={}&timeframe={}&limit={}&module={}&source=native",
            self.rest_base_url,
            urlencoding::encode(&self.exchange),
            urlencoding::encode(&self.symbol),
            urlencoding::encode(&self.timeframe),
            self.ohlcv_limit,
            urlencoding::encode(&format!("Chart · {} ({})", self.symbol, self.timeframe)),
        );
        if let Some(u) = until {
            // Only send `until` (matching the web version's fallback in
            // `shim.ts`). Forcing a `since` window would pin the request to a
            // fixed span, so a quiet market (weekend, holiday, illiquid pair)
            // could return an empty window and stall backward paging. Letting
            // the backend / CCXT traverse backwards from `until` jumps over
            // those gaps.
            url.push_str(&format!("&until={}", u));
        }
        url
    }

    /// Market type used by the history routes to resolve coverage/state.
    ///
    /// realtime-ondemand keys historical coverage by `type=spot|swap|future`,
    /// so a perpetual symbol must not be queried as spot.
    fn history_market_type(&self) -> &'static str {
        let exchange = self.exchange.to_ascii_lowercase();
        if exchange.ends_with("usdm") || self.symbol.contains(':') {
            "swap"
        } else if exchange.ends_with("coinm") || exchange.ends_with("futures") {
            "future"
        } else {
            "spot"
        }
    }

    /// Historical L2 order-book route (CoinWatch `/api/v1/orderbook/history`).
    pub fn orderbook_history_url(
        &self,
        since: i64,
        until: i64,
        depth: u32,
        sample_ms: i64,
    ) -> String {
        self.book_history_url("orderbook", since, until, depth, sample_ms)
    }

    /// Historical top-of-book route (CoinWatch `/api/v1/ticker/history`).
    ///
    /// Exposed for API completeness. The chart derives L1 from the L2 history
    /// instead of spending a second metered request.
    pub fn ticker_history_url(
        &self,
        since: i64,
        until: i64,
        depth: u32,
        sample_ms: i64,
    ) -> String {
        self.book_history_url("ticker", since, until, depth, sample_ms)
    }

    fn book_history_url(
        &self,
        route: &str,
        since: i64,
        until: i64,
        depth: u32,
        sample_ms: i64,
    ) -> String {
        let sample_ms = sample_ms.max(1);
        let market_type = self.history_market_type();
        // The server accepts 1..=20000 samples; one 15-minute 1s window is 900.
        let limit = ((until.saturating_sub(since)).max(0) / sample_ms).clamp(1, 20_000);
        // The route only supports 20 or 50 levels.
        let depth = if depth > 20 { 50 } else { 20 };
        format!(
            "{}/api/v1/{route}/history?exchange={}&symbol={}&since={since}&until={until}&limit={limit}&depth={depth}&sample_ms={sample_ms}&type={market_type}&module={}&source=native",
            self.rest_base_url,
            urlencoding::encode(&self.exchange),
            urlencoding::encode(&self.symbol),
            urlencoding::encode(&format!("Chart · {}", self.symbol)),
        )
    }

    /// Cursor-paginated raw-trade history (CoinWatch
    /// `/api/v1/trades/history`).
    ///
    /// `cursor` and `since` are mutually exclusive on the backend; when a
    /// cursor is supplied it already encodes the next page's lower bound, so
    /// `since` is omitted.
    pub fn trades_history_url(
        &self,
        since: i64,
        until: i64,
        limit: usize,
        order: &str,
        cursor: Option<&str>,
    ) -> String {
        let limit = limit.clamp(1, 20_000);
        let order = if order.eq_ignore_ascii_case("asc") {
            "asc"
        } else {
            "desc"
        };
        let market_type = self.history_market_type();
        let mut url = format!(
            "{}/api/v1/trades/history?exchange={}&symbol={}&until={until}&limit={limit}&type={market_type}&order={order}",
            self.rest_base_url,
            urlencoding::encode(&self.exchange),
            urlencoding::encode(&self.symbol),
        );
        match cursor.filter(|cursor| !cursor.is_empty()) {
            Some(cursor) => {
                url.push_str("&cursor=");
                url.push_str(&urlencoding::encode(cursor));
            }
            None => {
                url.push_str(&format!("&since={since}"));
            }
        }
        url
    }
}

// ---------------------------------------------------------------------------
// Bridge
// ---------------------------------------------------------------------------

/// Live handle to the WebSocket supervision loop.
///
/// Dropping this cancels the loop and closes the WebSocket connection.
pub struct Bridge {
    tick_sender: broadcast::Sender<Arc<UnifiedTick>>,
    trade_sender: broadcast::Sender<Arc<UnifiedTick>>,
    ob_sender: broadcast::Sender<Arc<OrderBook>>,
    ohlcv_sender: broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    derived_sender: broadcast::Sender<Arc<DerivedFlowEvent>>,
    /// Cached full OHLCV history — replayed to new subscribers immediately.
    last_ohlcv_history: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    last_tick: Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>>,
    last_orderbook: Arc<std::sync::Mutex<Option<Arc<OrderBook>>>>,
    cancel_token: CancellationToken,
    _supervisor: JoinHandle<()>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        info!("gateway: supervision loop cancelled");
    }
}

impl Bridge {
    /// Connect to the gateway WebSocket and start streaming.
    ///
    /// The supervision loop will reconnect automatically on any disconnect.
    pub fn start(config: BridgeConfig) -> anyhow::Result<Self> {
        let (tick_sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (trade_sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (ob_sender, _) = broadcast::channel(OB_BROADCAST_CAPACITY);
        let (ohlcv_sender, _) = broadcast::channel(OHLCV_BROADCAST_CAPACITY);
        let (derived_sender, _) = broadcast::channel(256);
        let cancel_token = CancellationToken::new();
        let last_ohlcv_history: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let last_tick: Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>> =
            Arc::new(std::sync::Mutex::new(None));
        let last_orderbook: Arc<std::sync::Mutex<Option<Arc<OrderBook>>>> =
            Arc::new(std::sync::Mutex::new(None));

        let tick_tx = tick_sender.clone();
        let trade_tx = trade_sender.clone();
        let ob_tx = ob_sender.clone();
        let ohlcv_tx = ohlcv_sender.clone();
        let derived_tx = derived_sender.clone();
        let token = cancel_token.clone();
        let history_cache = Arc::clone(&last_ohlcv_history);
        let tick_cache = Arc::clone(&last_tick);
        let ob_cache = Arc::clone(&last_orderbook);

        let supervisor = tokio_runtime().spawn(async move {
            supervision_loop(
                config,
                tick_tx,
                trade_tx,
                ob_tx,
                ohlcv_tx,
                derived_tx,
                token,
                history_cache,
                tick_cache,
                ob_cache,
            )
            .await;
        });

        Ok(Self {
            tick_sender,
            trade_sender,
            ob_sender,
            ohlcv_sender,
            derived_sender,
            last_ohlcv_history,
            last_tick,
            last_orderbook,
            cancel_token,
            _supervisor: supervisor,
        })
    }

    /// Subscribe to the trade tick stream.
    ///
    /// If a tick (e.g. from ticker snapshot) is already cached, it is replayed
    /// immediately so new subscribers do not miss the initial price.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<UnifiedTick>> {
        let rx = self.tick_sender.subscribe();
        if let Some(tick) = self.last_tick.lock().unwrap().clone() {
            let _ = self.tick_sender.send(tick);
        }
        rx
    }

    /// Return the most recently cached tick, if any.
    pub fn latest_tick(&self) -> Option<Arc<UnifiedTick>> {
        self.last_tick.lock().unwrap().clone()
    }

    /// Subscribe to actual executed market trades stream.
    pub fn subscribe_trades(&self) -> broadcast::Receiver<Arc<UnifiedTick>> {
        self.trade_sender.subscribe()
    }

    /// Subscribe to the order book snapshot stream.
    ///
    /// If an order book snapshot is already cached, it is replayed immediately.
    pub fn subscribe_orderbook(&self) -> broadcast::Receiver<Arc<OrderBook>> {
        let rx = self.ob_sender.subscribe();
        if let Some(book) = self.last_orderbook.lock().unwrap().clone() {
            let _ = self.ob_sender.send(book);
        }
        rx
    }

    /// Return the most recently cached orderbook, if any.
    pub fn latest_orderbook(&self) -> Option<Arc<OrderBook>> {
        self.last_orderbook.lock().unwrap().clone()
    }

    /// Subscribe to the OHLCV candle stream.
    ///
    /// If history is already cached, it is replayed immediately so the chart
    /// renders data without waiting for the next WS push.
    pub fn subscribe_ohlcv(&self) -> broadcast::Receiver<Arc<Vec<OhlcvCandle>>> {
        let rx = self.ohlcv_sender.subscribe();
        if let Some(history) = self.last_ohlcv_history.lock().unwrap().clone() {
            info!(
                "gateway: replaying {} cached candles to new subscriber",
                history.len()
            );
            match self.ohlcv_sender.send(history) {
                Ok(_) => info!("gateway: cache replay successful"),
                Err(e) => warn!("gateway: cache replay failed: {}", e),
            }
        } else {
            info!("gateway: no cached history yet — arrives after first REST fetch");
        }
        rx
    }

    /// Subscribe to derived Level 3 order-flow events.
    pub fn subscribe_derived_flow(&self) -> broadcast::Receiver<Arc<DerivedFlowEvent>> {
        self.derived_sender.subscribe()
    }

    /// Number of active trade tick subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tick_sender.receiver_count()
    }

    /// Check if this bridge's module (or general market-data requests) has run out of credits.
    pub fn credit_error(&self, module: &str) -> Option<crate::data::credits::CreditExhaustedInfo> {
        crate::data::credits::CreditStore::module_error(module)
    }
}

// ---------------------------------------------------------------------------
// Supervision loop
// ---------------------------------------------------------------------------

async fn supervision_loop(
    config: BridgeConfig,
    tick_tx: broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    derived_tx: broadcast::Sender<Arc<DerivedFlowEvent>>,
    cancel_token: CancellationToken,
    history_cache: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    last_tick_cache: Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>>,
    last_ob_cache: Arc<std::sync::Mutex<Option<Arc<OrderBook>>>>,
) {
    let mut history_fetched = false;
    loop {
        if cancel_token.is_cancelled() {
            info!("gateway: supervision loop cancelled");
            return;
        }

        let ws_url = config.ws_url();
        if ws_url.trim().is_empty() {
            warn!("gateway: no API service URL configured — streaming disabled");
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(RECONNECT_DELAY_MS)) => {}
                _ = cancel_token.cancelled() => { return; }
            }
            continue;
        }

        info!(
            "gateway: connecting to {} ({} {})",
            ws_url,
            config.exchange,
            config.symbol
        );

        match connect_and_stream(
            &config,
            &tick_tx,
            &trade_tx,
            &ob_tx,
            &ohlcv_tx,
            &derived_tx,
            &cancel_token,
            &history_cache,
            &last_tick_cache,
            &last_ob_cache,
            !history_fetched,
        )
        .await
        {
            Ok(_) => info!("gateway: WebSocket closed cleanly"),
            Err(e) => error!("gateway: WebSocket error: {e}"),
        }
        history_fetched = true;

        if cancel_token.is_cancelled() {
            return;
        }

        warn!("gateway: reconnecting in {}ms …", RECONNECT_DELAY_MS);
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(RECONNECT_DELAY_MS)) => {}
            _ = cancel_token.cancelled() => { return; }
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket connection + message loop
// ---------------------------------------------------------------------------

/// Outbound protocol frames for the realtime-ondemand gateway.
#[derive(serde::Serialize)]
struct HelloFrame {
    op: &'static str,
    protocol: u8,
    caps: [&'static str; 4],
}

#[derive(serde::Serialize)]
struct SubscribeFrame<'a> {
    op: &'static str,
    id: &'a str,
    requirement: Requirement<'a>,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Requirement<'a> {
    Ticker {
        exchange: &'a str,
        symbol: &'a str,
    },
    Trades {
        exchange: &'a str,
        symbol: &'a str,
        limit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        since: Option<i64>,
    },
    Orderbook {
        exchange: &'a str,
        symbol: &'a str,
        depth: u32,
    },
    Ohlcv {
        exchange: &'a str,
        symbol: &'a str,
        timeframe: &'a str,
        lookback: u32,
    },
    Delta {
        exchange: &'a str,
        symbol: &'a str,
        timeframe: &'a str,
    },
    Footprint {
        exchange: &'a str,
        symbol: &'a str,
        timeframe: &'a str,
        rows: u32,
    },
    Profile {
        exchange: &'a str,
        symbol: &'a str,
        timeframe: &'a str,
        mode: &'a str,
    },
}

#[derive(serde::Serialize)]
struct HeartbeatFrame {
    op: &'static str,
    ts: i64,
}

#[derive(serde::Serialize)]
struct ResyncFrame<'a> {
    op: &'static str,
    id: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamChannel {
    Ticker,
    Trades,
    Orderbook,
    Ohlcv,
    Delta,
    Footprint,
    Profile,
}

impl StreamChannel {
    fn id(self) -> &'static str {
        match self {
            Self::Ticker => "ticker",
            Self::Trades => "trades",
            Self::Orderbook => "orderbook",
            Self::Ohlcv => "kline",
            Self::Delta => "delta",
            Self::Footprint => "footprint",
            Self::Profile => "profile",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "ticker" => Some(Self::Ticker),
            "trades" | "tape" => Some(Self::Trades),
            "orderbook" | "book" => Some(Self::Orderbook),
            "kline" | "ohlcv" | "chart" => Some(Self::Ohlcv),
            "delta" => Some(Self::Delta),
            "footprint" => Some(Self::Footprint),
            "profile" => Some(Self::Profile),
            _ => None,
        }
    }
}

fn channel_for_feed(feed: &str) -> Option<StreamChannel> {
    if feed.starts_with("ticker:") {
        Some(StreamChannel::Ticker)
    } else if feed.starts_with("trades:") {
        Some(StreamChannel::Trades)
    } else if feed.starts_with("book:") {
        Some(StreamChannel::Orderbook)
    } else if feed.starts_with("ohlcv:") || feed.starts_with("candles:") {
        Some(StreamChannel::Ohlcv)
    } else if feed.starts_with("delta:") {
        Some(StreamChannel::Delta)
    } else if feed.starts_with("footprint:") {
        Some(StreamChannel::Footprint)
    } else if feed.starts_with("profile:") {
        Some(StreamChannel::Profile)
    } else {
        None
    }
}

/// A parsed v2 gateway frame. The client only needs the fields it acts on.
#[derive(Debug, Default)]
struct GatewayFrame {
    op: Option<String>,
    id: Option<String>,
    feed: Option<String>,
    seq: Option<u64>,
    first_seq: Option<u64>,
    last_seq: Option<u64>,
    data: Option<serde_json::Value>,
    message: Option<String>,
    code: Option<String>,
    stale: Option<bool>,
}

impl GatewayFrame {
    fn parse(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let object = value.as_object()?;
        Some(Self {
            op: object
                .get("op")
                .and_then(|v| v.as_str())
                .map(ToString::to_string),
            id: object
                .get("id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string),
            feed: object
                .get("feed")
                .and_then(|v| v.as_str())
                .map(ToString::to_string),
            seq: object.get("seq").and_then(|v| v.as_u64()),
            first_seq: object.get("first_seq").and_then(|v| v.as_u64()),
            last_seq: object.get("last_seq").and_then(|v| v.as_u64()),
            data: object.get("data").cloned(),
            message: object
                .get("message")
                .and_then(|v| v.as_str())
                .map(ToString::to_string),
            code: object
                .get("code")
                .and_then(|v| v.as_str())
                .map(ToString::to_string),
            stale: object.get("stale").and_then(|v| v.as_bool()),
        })
    }
}

/// Top-level envelope for the internal dispatch path.
#[derive(Debug)]
struct GatewayMessage {
    /// Set on subscription ack: `"info"` / `"data"`.
    msg_type: Option<String>,
    /// Set on data pushes: `"trades"` / `"orderbook"` / `"kline"` / `"ticker"`.
    channel: Option<String>,
    exchange: Option<String>,
    symbol: Option<String>,
    timeframe: Option<String>,
    /// Payload — shape depends on `channel`.
    data: Option<serde_json::Value>,
    /// Server-side error string or info message.
    message: Option<String>,
}

impl<'de> serde::Deserialize<'de> for GatewayMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut msg = GatewayMessage {
            msg_type: None,
            channel: None,
            exchange: None,
            symbol: None,
            timeframe: None,
            data: None,
            message: None,
        };

        if let serde_json::Value::Object(map) = value {
            msg.msg_type = map.get("type").and_then(|v| v.as_str()).map(ToString::to_string);
            msg.channel = map.get("channel")
                .or_else(|| map.get("topic"))
                .or_else(|| map.get("stream"))
                .and_then(|v| v.as_str())
                .map(|s| if s == "ohlcv" { "kline".to_string() } else { s.to_string() });
            msg.exchange = map.get("exchange").and_then(|v| v.as_str()).map(ToString::to_string);
            msg.symbol = map.get("symbol").and_then(|v| v.as_str()).map(ToString::to_string);
            msg.timeframe = map.get("timeframe").and_then(|v| v.as_str()).map(ToString::to_string);
            msg.message = map.get("message").and_then(|v| v.as_str()).map(ToString::to_string);

            // Accept "data" or "payload". If both exist, take whichever is non-null (preferring "data").
            msg.data = match (map.get("data"), map.get("payload")) {
                (Some(d), _) if !d.is_null() => Some(d.clone()),
                (_, Some(p)) if !p.is_null() => Some(p.clone()),
                (Some(d), _) => Some(d.clone()),
                _ => None,
            };
        }

        Ok(msg)
    }
}

/// A single trade entry in a `trades` data push.
#[derive(serde::Deserialize, Debug, Clone)]
pub struct GatewayTrade {
    #[serde(default, alias = "id", alias = "trade_id", alias = "a")]
    pub id: Option<String>,
    #[serde(
        default,
        alias = "exchange_ts",
        alias = "t",
        alias = "T",
        alias = "time"
    )]
    pub timestamp: i64,
    #[serde(default, alias = "s", alias = "S")]
    pub side: Option<String>,
    #[serde(default, alias = "p", alias = "px")]
    pub price: f64,
    #[serde(default, alias = "q", alias = "qty", alias = "size", alias = "sz")]
    pub amount: f64,
    #[serde(default, alias = "cost", alias = "notional")]
    pub cost: Option<f64>,
}

/// Order book payload in an `orderbook` data push.
#[derive(serde::Deserialize, Debug)]
struct GatewayOrderBook {
    bids: Vec<[f64; 2]>,
    asks: Vec<[f64; 2]>,
    #[serde(default)]
    timestamp: Option<i64>,
    /// Some producers send the exchange time under this name instead of
    /// `timestamp`. Keep both as plain optional fields so a frame that carries
    /// both cannot trigger a duplicate-field error.
    #[serde(default)]
    exchange_ts: Option<i64>,
    #[serde(default)]
    nonce: Option<u64>,
}

async fn connect_and_stream(
    config: &BridgeConfig,
    tick_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    derived_tx: &broadcast::Sender<Arc<DerivedFlowEvent>>,
    cancel_token: &CancellationToken,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    last_tick_cache: &Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>>,
    last_ob_cache: &Arc<std::sync::Mutex<Option<Arc<OrderBook>>>>,
    fetch_history_once: bool,
) -> anyhow::Result<()> {
    // Only chart and ticker panes ask CoinWatch for an OHLCV REST snapshot.
    // Other panes are stream-only; requesting chart history for each of them
    // produced traffic (and credit deductions) the web dashboard never makes.
    if config.fetch_history && fetch_history_once {
        info!(
            "gateway: fetching {} historical candles for {} {} {}",
            config.ohlcv_limit, config.exchange, config.symbol, config.timeframe
        );
        match fetch_ohlcv_history(config, None).await {
            Ok(candles) if !candles.is_empty() => {
                info!("gateway: {} candles fetched", candles.len());
                let arc = Arc::new(candles);
                if let Ok(mut cache) = history_cache.lock() {
                    *cache = Some(Arc::clone(&arc));
                }
                if ohlcv_tx.receiver_count() > 0 {
                    let _ = ohlcv_tx.send(arc);
                }
            }
            Ok(_) => warn!("gateway: OHLCV history returned empty"),
            Err(e) => error!("gateway: OHLCV history fetch failed: {e}"),
        }
    }

    let url = config.ws_url();
    if url.trim().is_empty() {
        anyhow::bail!("No API service URL configured");
    }
    let (ws_stream, _) = connect_async(&url).await?;
    info!("gateway: WebSocket connected to {url}");

    let (mut write, mut read) = ws_stream.split();

    // One protocol, negotiated in-band. CoinWatch relays the same frames from
    // web and native clients; there is no legacy `?legacy=1` path anymore.
    let hello = HelloFrame {
        op: "hello",
        protocol: 2,
        caps: ["trade_batch", "seq", "status", "strict_side"],
    };
    write
        .send(Message::Text(serde_json::to_string(&hello)?))
        .await?;

    let trade_since = if config.trade_since_current_bar {
        let frame_ms = BridgeConfig::timeframe_to_ms(&config.timeframe).max(1_000);
        let now = now_ms();
        Some(now - now.rem_euclid(frame_ms))
    } else {
        None
    };

    if config.ticker {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Ticker.id(),
            requirement: Requirement::Ticker {
                exchange: &config.exchange,
                symbol: &config.symbol,
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    if config.kline {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Ohlcv.id(),
            requirement: Requirement::Ohlcv {
                exchange: &config.exchange,
                symbol: &config.symbol,
                timeframe: &config.timeframe,
                lookback: config.ohlcv_limit,
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    if config.orderbook {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Orderbook.id(),
            requirement: Requirement::Orderbook {
                exchange: &config.exchange,
                symbol: &config.symbol,
                depth: config.orderbook_depth,
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    if config.trades {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Trades.id(),
            requirement: Requirement::Trades {
                exchange: &config.exchange,
                symbol: &config.symbol,
                limit: config.trade_limit,
                since: trade_since,
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    if config.delta {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Delta.id(),
            requirement: Requirement::Delta {
                exchange: &config.exchange,
                symbol: &config.symbol,
                timeframe: &config.timeframe,
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    if config.footprint {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Footprint.id(),
            requirement: Requirement::Footprint {
                exchange: &config.exchange,
                symbol: &config.symbol,
                timeframe: &config.timeframe,
                rows: 40,
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    if config.profile {
        let frame = SubscribeFrame {
            op: "sub",
            id: StreamChannel::Profile.id(),
            requirement: Requirement::Profile {
                exchange: &config.exchange,
                symbol: &config.symbol,
                timeframe: &config.timeframe,
                mode: "session",
            },
        };
        write
            .send(Message::Text(serde_json::to_string(&frame)?))
            .await?;
    }
    debug!(
        "gateway: subscriptions sent (ticker={}, kline={}, orderbook={}, trades={}, delta={}, footprint={}, profile={})",
        config.ticker,
        config.kline,
        config.orderbook,
        config.trades,
        config.delta,
        config.footprint,
        config.profile
    );

    let mut feed_channels: HashMap<String, StreamChannel> = HashMap::new();
    let mut feed_ids: HashMap<String, String> = HashMap::new();
    let mut last_seq: HashMap<String, u64> = HashMap::new();

    // Heartbeat ticker to keep the connection and its demand leases alive.
    let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
    heartbeat_interval.tick().await; // consume immediate first tick

    // Message loop.
    loop {
        tokio::select! {
            _ = heartbeat_interval.tick() => {
                if !config.ticker
                    && !config.kline
                    && !config.orderbook
                    && !config.trades
                    && !config.delta
                    && !config.footprint
                    && !config.profile
                {
                    continue;
                }
                let heartbeat = HeartbeatFrame {
                    op: "hb",
                    ts: now_ms(),
                };
                if let Ok(json) = serde_json::to_string(&heartbeat) {
                    if let Err(e) = write.send(Message::Text(json)).await {
                        warn!("gateway: failed to send heartbeat: {e}");
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(resync_id) = handle_gateway_frame(
                            &text,
                            config,
                            &mut feed_channels,
                            &mut feed_ids,
                            &mut last_seq,
                            tick_tx,
                            trade_tx,
                            ob_tx,
                            ohlcv_tx,
                            derived_tx,
                            history_cache,
                            last_tick_cache,
                            last_ob_cache,
                        ) {
                            let frame = ResyncFrame {
                                op: "resync",
                                id: &resync_id,
                            };
                            if let Ok(json) = serde_json::to_string(&frame) {
                                if let Err(e) = write.send(Message::Text(json)).await {
                                    warn!("gateway: failed to send resync: {e}");
                                    break;
                                }
                                info!("gateway: requested resync for {resync_id}");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("gateway: received close frame");
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("gateway: WebSocket error: {e}");
                        break;
                    }
                    None => {
                        info!("gateway: WebSocket stream ended");
                        break;
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                info!("gateway: cancellation received, closing WebSocket");
                let _ = write.send(Message::Close(None)).await;
                break;
            }
        }
    }

    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Translate one v2 gateway frame into the existing internal dispatch shape.
///
/// Returns a pane id when a sequence gap means the caller must send `resync`.
fn handle_gateway_frame(
    text: &str,
    config: &BridgeConfig,
    feed_channels: &mut HashMap<String, StreamChannel>,
    feed_ids: &mut HashMap<String, String>,
    last_seq: &mut HashMap<String, u64>,
    tick_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    derived_tx: &broadcast::Sender<Arc<DerivedFlowEvent>>,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    last_tick_cache: &Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>>,
    last_ob_cache: &Arc<std::sync::Mutex<Option<Arc<OrderBook>>>>,
) -> Option<String> {
    let Some(frame) = GatewayFrame::parse(text) else {
        warn!(
            "gateway: could not parse frame: {}",
            &text[..text.len().min(200)]
        );
        return None;
    };

    match frame.op.as_deref() {
        Some("hello") => None,
        Some("pong") => None,
        Some("ack") => {
            if let (Some(feed), Some(id)) = (frame.feed, frame.id) {
                if let Some(channel) = StreamChannel::from_id(&id) {
                    feed_channels.insert(feed.clone(), channel);
                    feed_ids.insert(feed, id);
                }
            }
            None
        }
        Some("snapshot") => {
            let Some(feed) = frame.feed else { return None };
            if !feed_channels.contains_key(&feed) {
                let channel = frame
                    .id
                    .as_deref()
                    .and_then(StreamChannel::from_id)
                    .or_else(|| channel_for_feed(&feed));
                if let Some(channel) = channel {
                    feed_channels.insert(feed.clone(), channel);
                    if let Some(id) = frame.id.as_deref() {
                        feed_ids.insert(feed.clone(), id.to_string());
                    } else {
                        feed_ids.insert(feed.clone(), channel.id().to_string());
                    }
                }
            }
            if let Some(seq) = frame.seq {
                last_seq.insert(feed.clone(), seq);
            }
            if let (Some(channel), Some(data)) = (feed_channels.get(&feed), frame.data) {
                dispatch_new_data(
                    *channel,
                    data,
                    config,
                    tick_tx,
                    trade_tx,
                    ob_tx,
                    ohlcv_tx,
                    derived_tx,
                    history_cache,
                    last_tick_cache,
                    last_ob_cache,
                );
            }
            None
        }
        Some("delta") => {
            let Some(feed) = frame.feed else { return None };
            let Some(channel) = feed_channels
                .get(&feed)
                .copied()
                .or_else(|| channel_for_feed(&feed))
            else {
                debug!("gateway: delta for unknown feed {feed}");
                return None;
            };
            feed_channels.entry(feed.clone()).or_insert(channel);

            let first = frame.first_seq.or(frame.seq);
            if let (Some(last), Some(first)) = (last_seq.get(&feed).copied(), first) {
                if first > last.saturating_add(1) {
                    warn!("gateway: sequence gap on {feed} ({last} -> {first}); resyncing");
                    return Some(
                        feed_ids
                            .get(&feed)
                            .cloned()
                            .unwrap_or_else(|| channel.id().to_string()),
                    );
                }
            }
            if let Some(seq) = frame.last_seq.or(frame.seq) {
                last_seq.insert(feed, seq);
            }

            if let Some(data) = frame.data {
                dispatch_new_data(
                    channel,
                    data,
                    config,
                    tick_tx,
                    trade_tx,
                    ob_tx,
                    ohlcv_tx,
                    derived_tx,
                    history_cache,
                    last_tick_cache,
                    last_ob_cache,
                );
            }
            None
        }
        Some("status") => {
            if frame.stale == Some(true) {
                warn!(
                    "gateway: feed {} is stale{}",
                    frame.feed.as_deref().unwrap_or("?"),
                    frame
                        .message
                        .map(|m| format!(": {m}"))
                        .unwrap_or_default()
                );
            }
            None
        }
        Some("error") => {
            let message = frame.message.unwrap_or_else(|| "gateway error".to_string());
            if message.to_lowercase().contains("credit")
                || message.to_lowercase().contains("insufficient")
            {
                crate::data::credits::CreditStore::record(
                    "Stream",
                    &message,
                    0,
                    1,
                    Some(&config.rest_base_url),
                );
            }
            warn!(
                "gateway: server error{}: {message}",
                frame
                    .code
                    .map(|code| format!(" ({code})"))
                    .unwrap_or_default()
            );
            None
        }
        other => {
            debug!("gateway: ignoring frame op {:?}", other);
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_new_data(
    channel: StreamChannel,
    data: serde_json::Value,
    config: &BridgeConfig,
    tick_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    derived_tx: &broadcast::Sender<Arc<DerivedFlowEvent>>,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    last_tick_cache: &Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>>,
    last_ob_cache: &Arc<std::sync::Mutex<Option<Arc<OrderBook>>>>,
) {
    if matches!(
        channel,
        StreamChannel::Delta | StreamChannel::Footprint | StreamChannel::Profile
    ) {
        let items = match data.as_array() {
            Some(rows) => rows.clone(),
            None => vec![data.clone()]
        };
        for item in items {
            let event = match channel {
                StreamChannel::Delta => serde_json::from_value::<DerivedDelta>(item)
                    .ok()
                    .map(DerivedFlowEvent::Delta),
                StreamChannel::Footprint => serde_json::from_value::<DerivedFootprint>(item)
                    .ok()
                    .map(DerivedFlowEvent::Footprint),
                StreamChannel::Profile => serde_json::from_value::<DerivedProfile>(item)
                    .ok()
                    .map(DerivedFlowEvent::Profile),
                _ => None
            };
            if let Some(event) = event {
                let _ = derived_tx.send(Arc::new(event));
            }
        }
        return;
    }

    let envelope = serde_json::json!({
        "type": "data",
        "channel": channel.id(),
        "exchange": config.exchange,
        "symbol": config.symbol,
        "timeframe": if channel == StreamChannel::Ohlcv { Some(config.timeframe.as_str()) } else { None },
        "data": data,
    });
    handle_message(
        &envelope.to_string(),
        config,
        tick_tx,
        trade_tx,
        ob_tx,
        ohlcv_tx,
        history_cache,
        last_tick_cache,
        last_ob_cache,
    );
}

fn handle_message(
    text: &str,
    config: &BridgeConfig,
    tick_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    last_tick_cache: &Arc<std::sync::Mutex<Option<Arc<UnifiedTick>>>>,
    last_ob_cache: &Arc<std::sync::Mutex<Option<Arc<OrderBook>>>>,
) {
    let msg: GatewayMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            warn!(
                "gateway: JSON parse error ({e}): {}",
                &text[..text.len().min(200)]
            );
            return;
        }
    };

    // Server error or info message
    if msg.msg_type.as_deref() == Some("info") {
        if let Some(m) = &msg.message {
            debug!("gateway: info message: {m}");
        }
        return;
    }

    if msg.msg_type.as_deref() == Some("error") {
        if let Some(m) = &msg.message {
            if m.to_lowercase().contains("credit") || m.to_lowercase().contains("insufficient") {
                crate::data::credits::CreditStore::record(
                    "Stream",
                    m,
                    0,
                    1,
                    Some(&config.rest_base_url),
                );
            }
            warn!("gateway: server error: {m}");
        }
        return;
    }

    // Data push — dispatch by channel
    let Some(data) = msg.data else { return };

    // A single gateway connection may multiplex several subscriptions, so drop
    // any push that belongs to a different exchange / symbol than this bridge.
    if !message_matches_config(&msg.exchange, &msg.symbol, &config.exchange, &config.symbol) {
        debug!(
            "gateway: ignoring {:?} push for {}/{} (bridge is {}/{})",
            msg.channel,
            msg.exchange.as_deref().unwrap_or("?"),
            msg.symbol.as_deref().unwrap_or("?"),
            config.exchange,
            config.symbol
        );
        return;
    }

    match msg.channel.as_deref() {
        Some("ticker") => {
            let target = if let Some(inner) = data.get("data").or_else(|| data.get("ticker")) {
                if inner.is_object() { inner } else { &data }
            } else {
                &data
            };

            let price_opt = target
                .get("last")
                .or_else(|| target.get("close"))
                .or_else(|| target.get("c"))
                .or_else(|| target.get("price"))
                .or_else(|| target.get("p"))
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

            if let Some(price) = price_opt {
                let raw_ts = target
                    .get("timestamp")
                    .or_else(|| target.get("exchange_ts"))
                    .or_else(|| target.get("t"))
                    .or_else(|| target.get("time"))
                    .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    });
                let ts = if raw_ts > 0 && raw_ts < 10_000_000_000 { raw_ts * 1000 } else { raw_ts };
                // Note: ticker push carries 24-hour cumulative market volume,
                // not an individual trade amount or candle volume.
                // UnifiedTick.amount represents individual traded amount, so set 0.0 here.
                let mut tick = UnifiedTick::new(
                    msg.exchange.as_deref().unwrap_or(&config.exchange),
                    msg.symbol.as_deref().unwrap_or(&config.symbol),
                    price,
                    0.0,
                    TradeSide::Unknown,
                    ts,
                );
                let open_24h = target
                    .get("open")
                    .or_else(|| target.get("o"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
                let high_24h = target
                    .get("high")
                    .or_else(|| target.get("h"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
                let low_24h = target
                    .get("low")
                    .or_else(|| target.get("l"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

                let change_24h = target
                    .get("changePercent")
                    .or_else(|| target.get("percentage"))
                    .or_else(|| target.get("P"))
                    .or_else(|| target.get("change_24h"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .or_else(|| {
                        let open = open_24h?;
                        if open > 0.0 && price > 0.0 {
                            Some(((price - open) / open) * 100.0)
                        } else {
                            None
                        }
                    });
                let volume_24h = target
                    .get("quote_volume")
                    .or_else(|| target.get("quoteVolume"))
                    .or_else(|| target.get("q"))
                    .or_else(|| target.get("volume"))
                    .or_else(|| target.get("base_volume"))
                    .or_else(|| target.get("v"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

                tick.change_24h_percent = change_24h;
                tick.quote_volume_24h = volume_24h;
                tick.open_24h = open_24h;
                tick.high_24h = high_24h;
                tick.low_24h = low_24h;
                let arc_tick = Arc::new(tick);
                if let Ok(mut cache) = last_tick_cache.lock() {
                    *cache = Some(Arc::clone(&arc_tick));
                }
                if let Ok(mut store) = crate::data::trade_store::TradeStore::global().lock() {
                    store.set_mark_price(&arc_tick.symbol, arc_tick.price);
                }
                let _ = tick_tx.send(arc_tick);
            }
        }

        Some("trades") => {
            let trades_list: Vec<GatewayTrade> = if let Ok(trades) = serde_json::from_value::<Vec<GatewayTrade>>(data.clone()) {
                trades
            } else if let Some(arr) = data.get("trades").or_else(|| data.get("data")).and_then(|v| v.as_array()) {
                arr.iter().filter_map(|item| serde_json::from_value::<GatewayTrade>(item.clone()).ok()).collect()
            } else if let Ok(single) = serde_json::from_value::<GatewayTrade>(data.clone()) {
                vec![single]
            } else {
                Vec::new()
            };

            // Build the batch first, then take the global TradeStore lock once.
            // Locking per trade serialised the whole tape behind the store
            // mutex; on a busy pair that is the difference between a live tape
            // and one that visibly lags the exchange.
            let mut ticks = Vec::with_capacity(trades_list.len());
            for trade in trades_list {
                if trade.price <= 0.0 || trade.amount <= 0.0 {
                    continue;
                }
                let raw_side = trade.side.as_deref().unwrap_or("").to_lowercase();
                let side = if raw_side.contains("buy") || raw_side == "b" || raw_side == "bid" {
                    TradeSide::Buy
                } else if raw_side.contains("sell") || raw_side == "s" || raw_side == "ask" {
                    TradeSide::Sell
                } else {
                    TradeSide::Unknown
                };

                let raw_ts = trade.timestamp;
                let ts = if raw_ts > 0 && raw_ts < 10_000_000_000 { raw_ts * 1000 } else { raw_ts };

                let mut tick = UnifiedTick::new(
                    msg.exchange.as_deref().unwrap_or(&config.exchange),
                    msg.symbol.as_deref().unwrap_or(&config.symbol),
                    trade.price,
                    trade.amount,
                    side,
                    ts,
                );
                tick.trade_id = trade.id;
                ticks.push(Arc::new(tick));
            }

            if !ticks.is_empty() {
                if let Ok(mut store) = crate::data::trade_store::TradeStore::global().lock() {
                    for tick in &ticks {
                        store.set_mark_price(&tick.symbol, tick.price);
                    }
                }
                for tick in ticks {
                    let _ = trade_tx.send(tick);
                }
            }
        }

        Some("orderbook") => {
            match serde_json::from_value::<GatewayOrderBook>(data) {
                Ok(raw) => {
                    let bids = raw
                        .bids
                        .into_iter()
                        .map(|[p, q]| PriceLevel::new(p, q))
                        .collect();
                    let asks = raw
                        .asks
                        .into_iter()
                        .map(|[p, q]| PriceLevel::new(p, q))
                        .collect();
                    let ts = raw.timestamp.or(raw.exchange_ts).unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    });
                    let exchange = msg.exchange.unwrap_or_else(|| config.exchange.clone());
                    let symbol = msg.symbol.unwrap_or_else(|| config.symbol.clone());
                    let mut book = OrderBook::new(exchange, symbol, bids, asks, ts);
                    book.nonce = raw.nonce;
                    let book = Arc::new(book);
                    if let Ok(mut cache) = last_ob_cache.lock() {
                        *cache = Some(Arc::clone(&book));
                    }
                    if let Some(mid) = book.mid_price() {
                        if let Ok(mut store) = crate::data::trade_store::TradeStore::global().lock() {
                            if store.get_mark_price(&book.symbol).is_none() {
                                store.set_mark_price(&book.symbol, mid);
                            }
                        }
                    }
                    if ob_tx.receiver_count() > 0 {
                        let _ = ob_tx.send(book);
                    }
                }
                Err(e) => warn!("gateway: orderbook data parse error: {e}"),
            }
        }

        Some("kline") | Some("ohlcv") => {
            let candles = parse_kline_payload(&data);
            if !candles.is_empty() {
                debug!(
                    "gateway: kline update ({} candles) latest_ts={}",
                    candles.len(),
                    candles.last().map(|c| c.timestamp).unwrap_or(0)
                );
                let arc = Arc::new(candles);
                let _ = ohlcv_tx.send(arc);
            } else {
                warn!("gateway: kline data parse produced 0 candles from: {:?}", data);
            }
        }

        other => {
            debug!("gateway: unknown channel {:?}", other);
        }
    }

    // Suppress unused-variable warning when history_cache is not written in this path.
    let _ = history_cache;
}

/// True when a gateway push belongs to the bridge's exchange/symbol.
///
/// Messages without an `exchange`/`symbol` field are accepted (the gateway
/// only sends those on single-subscription connections). Exchange ids are
/// compared case-insensitively and symbol separators (`/`, `-`, `_`, `:`) are
/// ignored so `BTC/USDT`, `BTC-USDT` and `BTC/USDT:USDT` all match.
fn message_matches_config(
    msg_exchange: &Option<String>,
    msg_symbol: &Option<String>,
    config_exchange: &str,
    config_symbol: &str,
) -> bool {
    let exchange_matches = match msg_exchange {
        Some(exchange) => exchange.trim().eq_ignore_ascii_case(config_exchange.trim()),
        None => true,
    };
    let symbol_matches = match msg_symbol {
        Some(symbol) => normalize_symbol_key(symbol) == normalize_symbol_key(config_symbol),
        None => true,
    };
    exchange_matches && symbol_matches
}

/// Lowercase a symbol and strip the separators exchanges disagree about.
fn normalize_symbol_key(symbol: &str) -> String {
    symbol
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_single_candle(val: &serde_json::Value) -> Option<OhlcvCandle> {
    if let Ok(candle) = serde_json::from_value::<OhlcvCandle>(val.clone()) {
        return Some(candle);
    }

    if let Some(arr) = val.as_array() {
        if arr.len() >= 5 {
            let t = arr[0].as_i64().or_else(|| arr[0].as_str().and_then(|s| s.parse().ok()))?;
            let o = arr[1].as_f64().or_else(|| arr[1].as_str().and_then(|s| s.parse().ok()))?;
            let h = arr[2].as_f64().or_else(|| arr[2].as_str().and_then(|s| s.parse().ok()))?;
            let l = arr[3].as_f64().or_else(|| arr[3].as_str().and_then(|s| s.parse().ok()))?;
            let c = arr[4].as_f64().or_else(|| arr[4].as_str().and_then(|s| s.parse().ok()))?;
            let v = arr.get(5).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0.0);
            let timestamp = if t > 0 && t < 10_000_000_000 { t * 1000 } else { t };
            return Some(OhlcvCandle { timestamp, open: o, high: h, low: l, close: c, volume: v });
        }
    }

    if let Some(obj) = val.as_object() {
        let t_val = obj.get("open_time")
            .or_else(|| obj.get("t"))
            .or_else(|| obj.get("timestamp"))
            .or_else(|| obj.get("time"))?;
        let o_val = obj.get("o").or_else(|| obj.get("open"))?;
        let h_val = obj.get("h").or_else(|| obj.get("high"))?;
        let l_val = obj.get("l").or_else(|| obj.get("low"))?;
        let c_val = obj.get("c").or_else(|| obj.get("close"))?;

        let t = t_val.as_i64().or_else(|| t_val.as_str().and_then(|s| s.parse().ok()))?;
        let o = o_val.as_f64().or_else(|| o_val.as_str().and_then(|s| s.parse().ok()))?;
        let h = h_val.as_f64().or_else(|| h_val.as_str().and_then(|s| s.parse().ok()))?;
        let l = l_val.as_f64().or_else(|| l_val.as_str().and_then(|s| s.parse().ok()))?;
        let c = c_val.as_f64().or_else(|| c_val.as_str().and_then(|s| s.parse().ok()))?;
        let v = obj.get("v")
            .or_else(|| obj.get("volume"))
            .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0.0);

        let timestamp = if t > 0 && t < 10_000_000_000 { t * 1000 } else { t };
        return Some(OhlcvCandle { timestamp, open: o, high: h, low: l, close: c, volume: v });
    }

    None
}

fn parse_kline_payload(data: &serde_json::Value) -> Vec<OhlcvCandle> {
    let mut out = Vec::new();

    if let Some(arr) = data.get("ohlcv").or_else(|| data.get("klines")).and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(candle) = parse_single_candle(item) {
                out.push(candle);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    if let Some(k) = data.get("k").or_else(|| data.get("kline")) {
        if let Some(candle) = parse_single_candle(k) {
            out.push(candle);
            return out;
        }
    }

    if let Some(arr) = data.as_array() {
        for item in arr {
            if let Some(candle) = parse_single_candle(item) {
                out.push(candle);
            }
        }
        if !out.is_empty() {
            return out;
        }
        if let Some(candle) = parse_single_candle(data) {
            out.push(candle);
            return out;
        }
    }

    if let Some(candle) = parse_single_candle(data) {
        out.push(candle);
    }
    out
}

// ---------------------------------------------------------------------------
// REST helper — OHLCV history only
// ---------------------------------------------------------------------------

/// Issue the metered OHLCV history request with the given bearer token.
async fn send_ohlcv_request(
    config: &BridgeConfig,
    url: &str,
    token: Option<&str>,
) -> anyhow::Result<reqwest::Response> {
    let mut req = reqwest::Client::new()
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Origin", &config.rest_base_url)
        .header("Referer", format!("{}/", &config.rest_base_url));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    Ok(req.timeout(Duration::from_secs(30)).send().await?)
}

/// Exchange the stored refresh token for a fresh access token.
///
/// The refreshed session is persisted (keyring + `~/.config/finter/session.json`)
/// so every other consumer of the session picks up the new credentials, and the
/// new access token is returned for the caller's immediate retry.
async fn refresh_session_token(config: &BridgeConfig) -> Option<String> {
    let stored = crate::data::session_store::load_session().ok().flatten()?;

    // Prefer the configured API origin, falling back to the hosted staging
    // origin used by the desktop client's auth flow.
    let base = {
        let configured = config.rest_base_url.trim().trim_end_matches('/');
        if configured.is_empty() {
            "https://staging.coinwat.ch".to_string()
        } else {
            configured.to_string()
        }
    };
    let url = format!("{base}/api/auth/refresh");
    let body = crate::data::models::RefreshRequest {
        refresh_token: stored.refresh_token.clone(),
    };

    let response = reqwest::Client::new()
        .post(&url)
        .header("Origin", &base)
        .header("Referer", format!("{base}/"))
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| warn!("gateway: session refresh request failed: {e}"))
        .ok()?;

    let status = response.status();
    let bytes = response.bytes().await.ok()?;
    if !status.is_success() {
        warn!("gateway: session refresh returned HTTP {status}");
        return None;
    }
    let auth: crate::data::models::AuthResponse = serde_json::from_slice(&bytes)
        .map_err(|e| warn!("gateway: session refresh response parse error: {e}"))
        .ok()?;
    if !auth.success {
        warn!(
            "gateway: session refresh rejected: {}",
            auth.error.unwrap_or_else(|| "unknown error".to_string())
        );
        return None;
    }

    let access_token = auth.access_token()?;
    let session = crate::data::models::AuthSession {
        user_id: auth.user_id().unwrap_or(stored.user_id),
        email: auth.email().unwrap_or(stored.email),
        access_token: access_token.clone(),
        refresh_token: auth.refresh_token().unwrap_or(stored.refresh_token),
        expires_at: auth.expires_at().or(stored.expires_at),
        name: auth.name().filter(|s| !s.is_empty()).or(stored.name),
        avatar_url: auth
            .avatar_url()
            .filter(|s| !s.is_empty())
            .or(stored.avatar_url),
    };
    if let Err(e) = crate::data::session_store::save_session(&session) {
        warn!("gateway: refreshed session could not be persisted: {e}");
    } else {
        info!("gateway: session refreshed and persisted");
    }
    Some(access_token)
}

pub async fn fetch_ohlcv_history(config: &BridgeConfig, until: Option<i64>) -> anyhow::Result<Vec<OhlcvCandle>> {
    if config.rest_base_url.trim().is_empty() {
        anyhow::bail!("No API service URL configured");
    }
    let url = config.ohlcv_history_url(until);
    // CoinWatch meters REST snapshots. Reuse immutable history briefly (and the
    // latest chart bootstrap longer) so reconnects or multiple panes do not
    // generate duplicate billable requests. Scope keys by credential digest:
    // users must never share a metered response/cache entry.
    let token = crate::data::session_store::load_session()
        .ok()
        .flatten()
        .map(|session| session.access_token)
        .or_else(|| config.access_token.clone());
    let token_digest = token.as_deref().map(|value| {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(value.as_bytes()))
    }).unwrap_or_else(|| "anonymous".to_string());
    let cache_key = format!("{token_digest}:{url}");
    if let Ok(mut cache) = history_request_cache().lock() {
        cache.retain(|_, entry| entry.expires_at > Instant::now());
        if let Some(entry) = cache.get(&cache_key) {
            return Ok(entry.candles.clone());
        }
    }
    debug!("gateway: GET {}", url.split('?').next().unwrap_or(&url));

    let response = send_ohlcv_request(config, &url, token.as_deref()).await?;
    let (response, cache_key) = if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        // The stored access token has expired (or was revoked). Refresh it once
        // with the stored refresh token, persist the new session and retry.
        warn!("gateway: OHLCV history returned 401 — attempting session refresh");
        match refresh_session_token(config).await {
            Some(refreshed) => {
                let digest = {
                    use sha2::{Digest, Sha256};
                    format!("{:x}", Sha256::digest(refreshed.as_bytes()))
                };
                let retry_key = format!("{digest}:{url}");
                let retry = send_ohlcv_request(config, &url, Some(&refreshed)).await?;
                (retry, retry_key)
            }
            None => (response, cache_key),
        }
    } else {
        (response, cache_key)
    };
    if !response.status().is_success() {
        let status = response.status();
        let error: serde_json::Value = response.json().await.unwrap_or_default();
        let message = error.get("message").and_then(|v| v.as_str()).unwrap_or("Market data request failed");
        if status.as_u16() == 402 || error.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits") {
            let balance = error.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
            let required = error.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
            crate::data::credits::CreditStore::record(
                "Chart",
                message,
                balance,
                required,
                Some(&config.rest_base_url),
            );
            anyhow::bail!("Insufficient credits: {} (balance {}, required {}). Manage credits at {}/account/credits", message,
                balance, required, config.rest_base_url);
        }
        anyhow::bail!("OHLCV history fetch failed: HTTP {}: {}", status, message);
    }

    let value: serde_json::Value = response.json().await?;
    if value.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits") {
        let message = value.get("message").and_then(|v| v.as_str()).unwrap_or("Not enough credits");
        let balance = value.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
        let required = value.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
        crate::data::credits::CreditStore::record(
            "Chart",
            message,
            balance,
            required,
            Some(&config.rest_base_url),
        );
        anyhow::bail!("Insufficient credits: {}", message);
    }
    let mut candles = Vec::new();

    // 1. Try { klines: [{ t, o, h, l, c, v }, ...] }
    if let Some(klines) = value.get("klines").and_then(|v| v.as_array()) {
        for k in klines {
            if let (Some(t_val), Some(o), Some(h), Some(l), Some(c), Some(v)) = (
                k.get("t"),
                k.get("o").and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))),
                k.get("h").and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))),
                k.get("l").and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))),
                k.get("c").and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))),
                k.get("v").and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))),
            ) {
                let raw_t = t_val.as_i64().or_else(|| t_val.as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
                let timestamp = if raw_t > 0 && raw_t < 10_000_000_000 {
                    raw_t * 1000
                } else {
                    raw_t
                };
                candles.push(OhlcvCandle {
                    timestamp,
                    open: o,
                    high: h,
                    low: l,
                    close: c,
                    volume: v,
                });
            }
        }
    } else {
        let raw = value.get("data").cloned()
            .or_else(|| value.get("candles").cloned())
            .unwrap_or_else(|| value.clone());
        // CoinWatch also returns compact tuple candles: [timestamp, open, high, low, close, volume].
        if let Some(rows) = raw.as_array() {
            candles = rows.iter().filter_map(|row| {
                let values = row.as_array()?;
                if values.len() < 6 { return None; }
                let number = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
                let raw_timestamp = values[0].as_i64()
                    .or_else(|| values[0].as_u64().map(|v| v as i64))
                    .or_else(|| values[0].as_str().and_then(|s| s.parse().ok()))?;
                let timestamp = if raw_timestamp > 0 && raw_timestamp < 10_000_000_000 { raw_timestamp * 1000 } else { raw_timestamp };
                Some(OhlcvCandle {
                    timestamp,
                    open: number(&values[1])?,
                    high: number(&values[2])?,
                    low: number(&values[3])?,
                    close: number(&values[4])?,
                    volume: number(&values[5])?,
                })
            }).collect();
        }
        if candles.is_empty() {
            if let Ok(parsed) = serde_json::from_value::<Vec<OhlcvCandle>>(raw) {
                candles = parsed;
            }
        }
    }

    // A normal latest snapshot can be reused for five minutes; time-bounded
    // chart gap-recovery history is intentionally kept for only 15 seconds.
    let ttl = if until.is_some() { Duration::from_secs(15) } else { Duration::from_secs(300) };
    if let Ok(mut cache) = history_request_cache().lock() {
        if cache.len() >= HISTORY_CACHE_MAX_ENTRIES {
            if let Some(oldest) = cache.iter().min_by_key(|(_, entry)| entry.expires_at).map(|(key, _)| key.clone()) {
                cache.remove(&oldest);
            }
        }
        cache.insert(cache_key, HistoryCacheEntry { candles: candles.clone(), expires_at: Instant::now() + ttl });
    }
    Ok(candles)
}

/// Fetch a window of historical L2 book snapshots.
///
/// `until` must be at/before the server's 1h-lagged cutoff; an entirely
/// recent window returns [`BookHistoryError::NotAvailable`]. The chart uses
/// these samples behind the live book floor and derives L1 from the top
/// levels.
pub async fn fetch_orderbook_history(
    config: &BridgeConfig,
    since: i64,
    until: i64,
    depth: u32,
    sample_ms: i64,
) -> Result<OrderbookHistory, BookHistoryError> {
    if config.rest_base_url.trim().is_empty() {
        return Err(BookHistoryError::Other(
            "No API service URL configured".to_string(),
        ));
    }
    if until <= 0 || since >= until {
        return Err(BookHistoryError::Other(
            "invalid book-history window".to_string(),
        ));
    }
    let url = config.orderbook_history_url(since, until, depth, sample_ms);
    fetch_book_history(config, &url, until).await
}

/// Fetch historical L1 (ticker) samples. Kept for API completeness; the chart
/// derives L1 from [`fetch_orderbook_history`] to avoid a second metered call.
pub async fn fetch_l1_history(
    config: &BridgeConfig,
    since: i64,
    until: i64,
    depth: u32,
    sample_ms: i64,
) -> Result<OrderbookHistory, BookHistoryError> {
    if config.rest_base_url.trim().is_empty() {
        return Err(BookHistoryError::Other(
            "No API service URL configured".to_string(),
        ));
    }
    if until <= 0 || since >= until {
        return Err(BookHistoryError::Other(
            "invalid ticker-history window".to_string(),
        ));
    }
    let url = config.ticker_history_url(since, until, depth, sample_ms);
    fetch_book_history(config, &url, until).await
}

async fn fetch_book_history(
    config: &BridgeConfig,
    url: &str,
    until: i64,
) -> Result<OrderbookHistory, BookHistoryError> {
    // Mirrors `fetch_ohlcv_history`: cache per credential so metered responses
    // are never shared between users, refresh once on 401, and surface 402 to
    // the credit store.
    let token = crate::data::session_store::load_session()
        .ok()
        .flatten()
        .map(|session| session.access_token)
        .or_else(|| config.access_token.clone());
    let token_digest = token
        .as_deref()
        .map(|value| {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(value.as_bytes()))
        })
        .unwrap_or_else(|| "anonymous".to_string());
    let cache_key = format!("{token_digest}:{url}");
    if let Ok(mut cache) = book_history_request_cache().lock() {
        cache.retain(|_, entry| entry.expires_at > Instant::now());
        if let Some(entry) = cache.get(&cache_key) {
            return Ok(entry.history.clone());
        }
    }
    debug!("gateway: GET {}", url.split('?').next().unwrap_or(url));

    let response = send_ohlcv_request(config, url, token.as_deref())
        .await
        .map_err(|error| BookHistoryError::Other(error.to_string()))?;
    let (response, cache_key) = if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        warn!("gateway: book history returned 401 — attempting session refresh");
        match refresh_session_token(config).await {
            Some(refreshed) => {
                let digest = {
                    use sha2::{Digest, Sha256};
                    format!("{:x}", Sha256::digest(refreshed.as_bytes()))
                };
                let retry_key = format!("{digest}:{url}");
                let retry = send_ohlcv_request(config, url, Some(&refreshed))
                    .await
                    .map_err(|error| BookHistoryError::Other(error.to_string()))?;
                (retry, retry_key)
            }
            None => (response, cache_key),
        }
    } else {
        (response, cache_key)
    };

    if !response.status().is_success() {
        let status = response.status();
        let error: serde_json::Value = response.json().await.unwrap_or_default();
        let message = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Market data request failed");
        if status.as_u16() == 402
            || error.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits")
        {
            let balance = error.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
            let required = error.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
            crate::data::credits::CreditStore::record(
                "Chart",
                message,
                balance,
                required,
                Some(&config.rest_base_url),
            );
            return Err(BookHistoryError::Credits(format!(
                "{} (balance {}, required {})",
                message, balance, required
            )));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(BookHistoryError::Auth);
        }
        if status == reqwest::StatusCode::BAD_REQUEST
            && (message.to_ascii_lowercase().contains("not available")
                || error
                    .get("error")
                    .and_then(|v| v.as_str())
                    .is_some_and(|code| code.contains("not_available")))
        {
            return Err(BookHistoryError::NotAvailable);
        }
        return Err(BookHistoryError::Other(format!(
            "book history fetch failed: HTTP {}: {}",
            status, message
        )));
    }

    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|error| BookHistoryError::Other(error.to_string()))?;
    if value.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits") {
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Not enough credits");
        let balance = value.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
        let required = value.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
        crate::data::credits::CreditStore::record(
            "Chart",
            message,
            balance,
            required,
            Some(&config.rest_base_url),
        );
        return Err(BookHistoryError::Credits(format!(
            "{} (balance {}, required {})",
            message, balance, required
        )));
    }
    let history = parse_orderbook_history(value);

    // Windows touching the live edge can change while an async backfill runs,
    // so cache them briefly; older windows are immutable enough for a minute.
    let near_live_edge = until > now_ms() - 3 * 60 * 60 * 1000;
    let ttl = if near_live_edge {
        Duration::from_secs(15)
    } else {
        Duration::from_secs(60)
    };
    if let Ok(mut cache) = book_history_request_cache().lock() {
        if cache.len() >= BOOK_HISTORY_CACHE_MAX_ENTRIES {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            cache_key,
            BookHistoryCacheEntry {
                history: history.clone(),
                expires_at: Instant::now() + ttl,
            },
        );
    }
    Ok(history)
}

/// Fetch one page of persisted raw trades.
///
/// Mirrors [`fetch_book_history`]: cache per credential, refresh once on 401,
/// surface 402 to the credit store, and map the relay's "not available" 400 to
/// [`TradeHistoryError::NotAvailable`]. Callers must treat this as metered and
/// only invoke it on an explicit user action.
pub async fn fetch_trades_history_page(
    config: &BridgeConfig,
    url: &str,
) -> Result<TradeHistoryPage, TradeHistoryError> {
    let token = crate::data::session_store::load_session()
        .ok()
        .flatten()
        .map(|session| session.access_token)
        .or_else(|| config.access_token.clone());
    let token_digest = token
        .as_deref()
        .map(|value| {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(value.as_bytes()))
        })
        .unwrap_or_else(|| "anonymous".to_string());
    let cache_key = format!("{token_digest}:{url}");
    if let Ok(mut cache) = trade_history_request_cache().lock() {
        cache.retain(|_, entry| entry.expires_at > Instant::now());
        if let Some(entry) = cache.get(&cache_key) {
            return Ok(entry.page.clone());
        }
    }
    debug!("gateway: GET {}", url.split('?').next().unwrap_or(url));

    let response = send_ohlcv_request(config, url, token.as_deref())
        .await
        .map_err(|error| TradeHistoryError::Other(error.to_string()))?;
    let (response, cache_key) = if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        warn!("gateway: trades history returned 401 — attempting session refresh");
        match refresh_session_token(config).await {
            Some(refreshed) => {
                let digest = {
                    use sha2::{Digest, Sha256};
                    format!("{:x}", Sha256::digest(refreshed.as_bytes()))
                };
                let retry_key = format!("{digest}:{url}");
                let retry = send_ohlcv_request(config, url, Some(&refreshed))
                    .await
                    .map_err(|error| TradeHistoryError::Other(error.to_string()))?;
                (retry, retry_key)
            }
            None => (response, cache_key),
        }
    } else {
        (response, cache_key)
    };

    if !response.status().is_success() {
        let status = response.status();
        let error: serde_json::Value = response.json().await.unwrap_or_default();
        let message = error
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Market data request failed");
        if status.as_u16() == 402
            || error.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits")
        {
            let balance = error.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
            let required = error.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
            crate::data::credits::CreditStore::record(
                "Chart",
                message,
                balance,
                required,
                Some(&config.rest_base_url),
            );
            return Err(TradeHistoryError::Credits(format!(
                "{} (balance {}, required {})",
                message, balance, required
            )));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(TradeHistoryError::Auth);
        }
        if status == reqwest::StatusCode::BAD_REQUEST
            && (message.to_ascii_lowercase().contains("not available")
                || error
                    .get("error")
                    .and_then(|v| v.as_str())
                    .is_some_and(|code| code.contains("not_available")))
        {
            return Err(TradeHistoryError::NotAvailable);
        }
        return Err(TradeHistoryError::Other(format!(
            "trade history fetch failed: HTTP {}: {}",
            status, message
        )));
    }

    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|error| TradeHistoryError::Other(error.to_string()))?;
    if value.get("error").and_then(|v| v.as_str()) == Some("insufficient_credits") {
        let message = value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Not enough credits");
        let balance = value.get("balance").and_then(|v| v.as_i64()).unwrap_or(0);
        let required = value.get("required").and_then(|v| v.as_i64()).unwrap_or(0);
        crate::data::credits::CreditStore::record(
            "Chart",
            message,
            balance,
            required,
            Some(&config.rest_base_url),
        );
        return Err(TradeHistoryError::Credits(format!(
            "{} (balance {}, required {})",
            message, balance, required
        )));
    }
    let page = parse_trade_history(value);

    // Windows touching the live edge can change while the sink catches up;
    // older windows are immutable enough for a minute.
    let near_live_edge = page.until > now_ms() - 3 * 60 * 60 * 1000;
    let ttl = if near_live_edge {
        Duration::from_secs(15)
    } else {
        Duration::from_secs(60)
    };
    if let Ok(mut cache) = trade_history_request_cache().lock() {
        if cache.len() >= TRADE_HISTORY_CACHE_MAX_ENTRIES {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            cache_key,
            TradeHistoryCacheEntry {
                page: page.clone(),
                expires_at: Instant::now() + ttl,
            },
        );
    }
    Ok(page)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_cfg() -> BridgeConfig {
        BridgeConfig {
            rest_base_url: "https://coinwat.ch".to_string(),
            exchange: "binance".to_string(),
            symbol: "BTC/USDT".to_string(),
            sandbox: false,
            timeframe: "1m".to_string(),
            ohlcv_limit: 500,
            access_token: None,
            ticker: true,
            trades: true,
            orderbook: true,
            kline: true,
            fetch_history: true,
            trade_limit: 100,
            trade_since_current_bar: false,
            orderbook_depth: 20,
            delta: false,
            footprint: false,
            profile: false,
        }
    }

    #[test]
    fn default_dev_config() {
        let cfg = BridgeConfig::default_dev("kraken", "ETH/USD");
        assert_eq!(cfg.exchange, "kraken");
        assert_eq!(cfg.symbol, "ETH/USD");
        assert!(!cfg.sandbox);
        assert_eq!(cfg.timeframe, "1m");
        assert_eq!(cfg.ohlcv_limit, 500);
    }

    #[test]
    fn book_history_urls_match_the_coinwatch_contract() {
        let cfg = base_cfg();
        let url = cfg.orderbook_history_url(1_700_000_000_000, 1_700_000_900_000, 20, 1_000);
        assert!(url.starts_with("https://coinwat.ch/api/v1/orderbook/history?"));
        assert!(url.contains("exchange=binance"));
        assert!(url.contains("symbol=BTC%2FUSDT"));
        assert!(url.contains("since=1700000000000"));
        assert!(url.contains("until=1700000900000"));
        assert!(url.contains("limit=900"));
        assert!(url.contains("depth=20"));
        assert!(url.contains("sample_ms=1000"));
        assert!(url.contains("type=spot"));
        assert!(url.contains("source=native"));

        // Depth is normalised to the two supported values and a one-second
        // window asks for one sample.
        let l1 = cfg.ticker_history_url(1_700_000_000_000, 1_700_000_001_000, 50, 1_000);
        assert!(l1.starts_with("https://coinwat.ch/api/v1/ticker/history?"));
        assert!(l1.contains("type=spot"));
        assert!(l1.contains("depth=50"));
        assert!(l1.contains("limit=1"));

        let mut perp = base_cfg();
        perp.exchange = "binanceusdm".to_string();
        let perp_url =
            perp.orderbook_history_url(1_700_000_000_000, 1_700_000_900_000, 50, 1_000);
        assert!(perp_url.contains("type=swap"));

        let mut colon_perp = base_cfg();
        colon_perp.symbol = "BTC/USDT:USDT".to_string();
        let colon_url =
            colon_perp.orderbook_history_url(1_700_000_000_000, 1_700_000_900_000, 50, 1_000);
        assert!(colon_url.contains("type=swap"));

        let mut futures = base_cfg();
        futures.exchange = "krakenfutures".to_string();
        let futures_url =
            futures.orderbook_history_url(1_700_000_000_000, 1_700_000_900_000, 50, 1_000);
        assert!(futures_url.contains("type=future"));
        // The route path stays the same for every market type.
        assert!(futures_url.contains("/api/v1/orderbook/history?"));
    }

    #[test]
    fn trades_history_url_matches_the_coinwatch_contract() {
        let cfg = base_cfg();
        let url =
            cfg.trades_history_url(1_700_000_000_000, 1_700_000_900_000, 20_000, "desc", None);
        assert!(url.starts_with("https://coinwat.ch/api/v1/trades/history?"));
        assert!(url.contains("exchange=binance"));
        assert!(url.contains("symbol=BTC%2FUSDT"));
        assert!(url.contains("since=1700000000000"));
        assert!(url.contains("until=1700000900000"));
        assert!(url.contains("limit=20000"));
        assert!(url.contains("type=spot"));
        assert!(url.contains("order=desc"));

        // A cursor page must not carry `since`: the relay rejects both.
        let cursor_url = cfg.trades_history_url(
            1_700_000_000_000,
            1_700_000_900_000,
            20_000,
            "desc",
            Some("MTcwMDAwMDAwMDAwMDp0cmFkZS0x"),
        );
        assert!(cursor_url.contains("cursor=MTcwMDAwMDAwMDAwMDp0cmFkZS0x"));
        assert!(!cursor_url.contains("since="));

        let mut perp = base_cfg();
        perp.exchange = "binanceusdm".to_string();
        let perp_url =
            perp.trades_history_url(1_700_000_000_000, 1_700_000_900_000, 20_000, "desc", None);
        assert!(perp_url.contains("type=swap"));

        // Limits are clamped to the relay's 1..=20000 range and order is
        // normalised.
        let clamped =
            cfg.trades_history_url(1_700_000_000_000, 1_700_000_900_000, 999_999, "ASC", None);
        assert!(clamped.contains("limit=20000"));
        assert!(clamped.contains("order=asc"));
    }

    #[test]
    fn book_history_payload_parses_canonical_and_legacy() {
        let canonical = serde_json::json!({
            "depth": 50,
            "sample_ms": 1000,
            "queued": false,
            "partial": false,
            "data": [{"ts": 1_700_000_000_000i64, "bids": [[100.0, 1.0]], "asks": [[101.0, 1.0]]}]
        });
        let history = crate::data::book_history::parse_orderbook_history(canonical);
        assert_eq!(history.data.len(), 1);
        assert_eq!(history.depth, 50);

        let legacy = serde_json::json!({
            "success": true,
            "data": [{"ts": 1_700_000_000, "bids": [[10.0, 2.0]], "asks": []}]
        });
        let history = crate::data::book_history::parse_orderbook_history(legacy);
        assert_eq!(history.data[0].ts, 1_700_000_000_000);
        assert_eq!(history.data[0].best_bid(), Some(10.0));
    }

    #[tokio::test]
    async fn v2_frames_dispatch_ticker_trades_candles_and_detect_gaps() {
        let cfg = base_cfg();
        let (ticks, mut tick_rx) = broadcast::channel(16);
        let (trades, mut trade_rx) = broadcast::channel(16);
        let (books, _) = broadcast::channel(16);
        let (candles, mut candle_rx) = broadcast::channel(16);
        let (derived, _) = broadcast::channel(16);
        let history = Arc::new(std::sync::Mutex::new(None));
        let last_tick = Arc::new(std::sync::Mutex::new(None));
        let last_book = Arc::new(std::sync::Mutex::new(None));
        let mut feed_channels = HashMap::new();
        let mut feed_ids = HashMap::new();
        let mut last_seq = HashMap::new();

        let mut frame = |text: &str| {
            handle_gateway_frame(
                text,
                &cfg,
                &mut feed_channels,
                &mut feed_ids,
                &mut last_seq,
                &ticks,
                &trades,
                &books,
                &candles,
                &derived,
                &history,
                &last_tick,
                &last_book,
            )
        };

        assert_eq!(
            frame(r#"{"op":"ack","id":"ticker","feed":"ticker:binance:BTC/USDT","seq":1}"#),
            None
        );
        assert_eq!(
            frame(r#"{"op":"delta","feed":"ticker:binance:BTC/USDT","seq":2,"data":{"last":110,"open":100,"high":120,"low":90,"quote_volume":1234,"percentage":10}}"#),
            None
        );
        let tick = tick_rx.try_recv().expect("ticker tick");
        assert!((tick.price - 110.0).abs() < f64::EPSILON);
        assert_eq!(tick.open_24h, Some(100.0));
        assert_eq!(tick.quote_volume_24h, Some(1234.0));

        assert_eq!(
            frame(r#"{"op":"ack","id":"trades","feed":"trades:binance:BTC/USDT","seq":2}"#),
            None
        );
        assert_eq!(
            frame(r#"{"op":"delta","feed":"trades:binance:BTC/USDT","seq":4,"first_seq":3,"last_seq":4,"data":[{"trade_id":"t1","exchange_ts":1700000000000,"side":"buy","price":110,"amount":1},{"trade_id":"t2","exchange_ts":1700000000001,"side":null,"price":111,"amount":2}]}"#),
            None
        );
        let first = trade_rx.try_recv().expect("trade 1");
        let second = trade_rx.try_recv().expect("trade 2");
        assert_eq!(first.trade_id.as_deref(), Some("t1"));
        assert_eq!(first.side, TradeSide::Buy);
        assert_eq!(second.side, TradeSide::Unknown);

        assert_eq!(
            frame(r#"{"op":"ack","id":"kline","feed":"ohlcv:binance:BTC/USDT:1m","seq":4}"#),
            None
        );
        assert_eq!(
            frame(r#"{"op":"delta","feed":"ohlcv:binance:BTC/USDT:1m","seq":5,"data":{"open_time":1700000060000,"open":110,"high":115,"low":109,"close":114,"volume":12,"timeframe":"1m","is_closed":false,"source":"exchange"}}"#),
            None
        );
        let candle = candle_rx.try_recv().expect("candle");
        assert_eq!(candle[0].timestamp, 1_700_000_060_000);
        assert!((candle[0].close - 114.0).abs() < f64::EPSILON);

        // A missing sequence returns the pane id that must be resynced.
        assert_eq!(
            frame(r#"{"op":"delta","feed":"trades:binance:BTC/USDT","seq":9,"first_seq":9,"last_seq":9,"data":[]}"#),
            Some("trades".to_string())
        );
    }

    #[test]
    fn ws_url_derives_from_rest_base() {
        assert_eq!(base_cfg().ws_url(), "wss://coinwat.ch/ws");
    }

    #[test]
    fn ws_url_uses_the_coinwatch_staging_origin() {
        let mut cfg = base_cfg();
        cfg.rest_base_url = "https://staging.coinwat.ch".to_string();
        assert_eq!(cfg.ws_url(), "wss://staging.coinwat.ch/ws");
    }

    #[test]
    fn ws_url_empty_when_no_base() {
        let mut cfg = base_cfg();
        cfg.rest_base_url = String::new();
        assert_eq!(cfg.ws_url(), "");
    }

    #[test]
    fn ws_url_works_with_http_scheme() {
        let mut cfg = base_cfg();
        cfg.rest_base_url = "http://localhost:3000".to_string();
        assert_eq!(cfg.ws_url(), "ws://localhost:3000/ws");
    }

    #[test]
    fn ticker_snapshot_url_is_metered_and_attributed_to_native_watchlist() {
        let url = base_cfg().ticker_snapshot_url("Watchlist");
        assert!(url.contains("/api/v1/ticker?"), "url={url}");
        assert!(url.contains("exchange=binance"), "url={url}");
        assert!(url.contains("symbol=BTC%2FUSDT"), "url={url}");
        assert!(url.contains("module=Watchlist%20%C2%B7%20BTC%2FUSDT"), "url={url}");
        assert!(url.ends_with("source=native"), "url={url}");
    }

    #[test]
    fn ohlcv_history_url() {
        let url = base_cfg().ohlcv_history_url(None);
        assert!(url.contains("timeframe=1m"), "url={url}");
        assert!(url.contains("limit=500"), "url={url}");
        assert!(url.contains("symbol=BTC%2FUSDT"), "url={url}");
    }

    #[test]
    fn ohlcv_history_url_with_until_sends_only_until() {
        let url = base_cfg().ohlcv_history_url(Some(1_600_000_000_000));
        assert!(!url.contains("&since="), "url={url}");
        assert!(url.contains("&until=1600000000000"), "url={url}");
    }

    #[test]
    fn message_filter_accepts_matching_exchange_and_symbol() {
        assert!(message_matches_config(
            &Some("binance".into()),
            &Some("BTC-USDT".into()),
            "binance",
            "BTC/USDT",
        ));
        // Exchange ids are case-insensitive, separators in symbols are ignored.
        assert!(message_matches_config(
            &Some("BINANCE".into()),
            &Some("BTC/USDT".into()),
            "binance",
            "BTC-USDT",
        ));
        // Messages without routing fields are accepted.
        assert!(message_matches_config(&None, &None, "binance", "BTC/USDT"));
    }

    #[test]
    fn message_filter_rejects_foreign_exchange_or_symbol() {
        assert!(!message_matches_config(
            &Some("okx".into()),
            &Some("BTC/USDT".into()),
            "binance",
            "BTC/USDT",
        ));
        assert!(!message_matches_config(
            &Some("binance".into()),
            &Some("ETH/USDT".into()),
            "binance",
            "BTC/USDT",
        ));
        assert!(!message_matches_config(
            &Some("binance".into()),
            &Some("BTC/USDT:USDT".into()),
            "binance",
            "BTC/USDT",
        ));
    }

    #[test]
    fn gateway_message_trades_parses() {
        let json = r#"{
            "channel":"trades","exchange":"binance","symbol":"BTC/USDT",
            "data":[{"timestamp":1700000000000,"side":"buy","price":67000.5,"amount":0.25}]
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse");
        assert_eq!(msg.channel.as_deref(), Some("trades"));
        let trades: Vec<GatewayTrade> =
            serde_json::from_value(msg.data.unwrap()).expect("parse data");
        assert_eq!(trades.len(), 1);
        assert!((trades[0].price - 67_000.5).abs() < f64::EPSILON);
        assert_eq!(trades[0].side.as_deref(), Some("buy"));
    }

    #[test]
    fn gateway_message_trades_accepts_realtime_fields() {
        let json = r#"{
            "channel":"trades","exchange":"binance","symbol":"BTC/USDT",
            "data":[{
                "event_id":"evt-1","trade_id":"6880359509","exchange_ts":1795095253528,
                "side":"sell","price":63724.01,"amount":0.00016
            }]
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse");
        let trades: Vec<GatewayTrade> =
            serde_json::from_value(msg.data.unwrap()).expect("parse data");
        assert_eq!(trades[0].timestamp, 1_795_095_253_528);
        assert_eq!(trades[0].id.as_deref(), Some("6880359509"));
        assert_eq!(trades[0].side.as_deref(), Some("sell"));
    }

    #[test]
    fn gateway_message_orderbook_parses() {
        let json = r#"{
            "channel":"orderbook","exchange":"binance","symbol":"BTC/USDT",
            "data":{"bids":[[67000.0,1.5]],"asks":[[67010.0,1.0]],"timestamp":1700000000000}
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse");
        let ob: GatewayOrderBook = serde_json::from_value(msg.data.unwrap()).expect("parse data");
        assert_eq!(ob.bids.len(), 1);
        assert!((ob.bids[0][0] - 67_000.0).abs() < f64::EPSILON);
        assert_eq!(ob.timestamp, Some(1_700_000_000_000));
    }

    #[test]
    fn gateway_message_orderbook_accepts_both_time_fields() {
        let json = r#"{
            "channel":"orderbook","exchange":"binance","symbol":"BTC/USDT",
            "data":{"bids":[[67000.0,1.5]],"asks":[[67010.0,1.0]],
                    "timestamp":1700000000000,"exchange_ts":1699999999000}
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse");
        let ob: GatewayOrderBook = serde_json::from_value(msg.data.unwrap()).expect("parse data");
        assert_eq!(ob.timestamp, Some(1_700_000_000_000));
        assert_eq!(ob.exchange_ts, Some(1_699_999_999_000));
    }

    #[test]
    fn gateway_live_kline_payload_parses() {
        // Captured from wss://ws.coinwat.ch/ws on 2026-10-03.
        let json = r#"{
            "type":"data","exchange":"binance","symbol":"BTC-USDT","channel":"kline","timeframe":"1m",
            "payload":{"t":1791014580,"o":84597.55,"h":84597.55,"l":84592,"c":84592.02,"v":7.04415}
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse");
        assert_eq!(msg.msg_type.as_deref(), Some("data"));
        assert_eq!(msg.channel.as_deref(), Some("kline"));
        assert_eq!(msg.timeframe.as_deref(), Some("1m"));
        let candles = parse_kline_payload(&msg.data.expect("payload"));
        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].timestamp, 1_791_014_580_000);
        assert!((candles[0].close - 84_592.02).abs() < f64::EPSILON);
    }

    #[test]
    fn gateway_info_ack_parses() {
        let msg: GatewayMessage = serde_json::from_str(
            r#"{"type":"info","message":"Subscribed to kline:1m for BTC-USDT on binance"}"#,
        )
        .expect("parse");
        assert_eq!(msg.msg_type.as_deref(), Some("info"));
        assert!(msg.data.is_none());
    }

    #[test]
    fn gateway_error_parses() {
        let msg: GatewayMessage = serde_json::from_str(
            r#"{"type":"error","message":"Unsupported exchange"}"#,
        )
        .expect("parse");
        assert_eq!(msg.msg_type.as_deref(), Some("error"));
        assert_eq!(msg.message.as_deref(), Some("Unsupported exchange"));
    }

    #[test]
    fn gateway_message_with_duplicate_data_field_parses() {
        // Test handling of duplicate "data" field or both "data" and "payload"
        let json = r#"{
            "type":"data","exchange":"binance","symbol":"BTC-USDT","channel":"ticker",
            "data":{"symbol":"BTC-USDT","timestamp":1792380983006,"datetime":"2026-10-19T03:36:23.006Z","open":81844.39,"high":82817.1,"low":81000.0,"close":82500.0,"last":82500.0},
            "data":{"symbol":"BTC-USDT","last":82500.0}
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse with duplicate data field");
        assert_eq!(msg.channel.as_deref(), Some("ticker"));
        assert!(msg.data.is_some());
    }

    #[test]
    fn gateway_message_with_both_data_and_payload_parses() {
        let json = r#"{
            "type":"data","exchange":"binance","symbol":"BTC-USDT","channel":"ticker",
            "payload":{"symbol":"BTC-USDT","last":82500.0},
            "data":{"symbol":"BTC-USDT","timestamp":1792380983006,"last":82500.0}
        }"#;
        let msg: GatewayMessage = serde_json::from_str(json).expect("parse with both data and payload");
        assert_eq!(msg.channel.as_deref(), Some("ticker"));
        assert!(msg.data.is_some());
    }

    #[tokio::test]
    async fn last_tick_and_orderbook_replay_on_subscribe() {
        let (tick_tx, _) = broadcast::channel(16);
        let (trade_tx, _) = broadcast::channel(16);
        let (ob_tx, _) = broadcast::channel(16);
        let (ohlcv_tx, _) = broadcast::channel(16);
        let (derived_tx, _) = broadcast::channel(16);

        let cached_tick = Arc::new(UnifiedTick::new("binance", "BTC/USDT", 68500.0, 1.0, TradeSide::Buy, 1000));
        let cached_book = Arc::new(OrderBook::new(
            "binance".to_string(),
            "BTC/USDT".to_string(),
            vec![PriceLevel::new(68490.0, 1.0)],
            vec![PriceLevel::new(68510.0, 1.0)],
            1000,
        ));

        let bridge = Bridge {
            tick_sender: tick_tx,
            trade_sender: trade_tx,
            ob_sender: ob_tx,
            ohlcv_sender: ohlcv_tx,
            derived_sender: derived_tx,
            last_ohlcv_history: Arc::new(std::sync::Mutex::new(None)),
            last_tick: Arc::new(std::sync::Mutex::new(Some(Arc::clone(&cached_tick)))),
            last_orderbook: Arc::new(std::sync::Mutex::new(Some(Arc::clone(&cached_book)))),
            cancel_token: CancellationToken::new(),
            _supervisor: tokio::spawn(async {}),
        };

        // When a subscriber attaches after bridge startup, cached tick is replayed immediately
        let mut tick_rx = bridge.subscribe();
        let replayed_tick = tick_rx.try_recv().expect("cached tick must be replayed");
        assert_eq!(replayed_tick.price, 68500.0);

        // When an orderbook subscriber attaches, cached orderbook is replayed immediately
        let mut ob_rx = bridge.subscribe_orderbook();
        let replayed_book = ob_rx.try_recv().expect("cached orderbook must be replayed");
        assert_eq!(replayed_book.mid_price(), Some(68500.0));
    }

    #[tokio::test]
    async fn ticker_request_coalescing_deduplicates_concurrent_fetches() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        static REQ_COUNT: AtomicUsize = AtomicUsize::new(0);
        REQ_COUNT.store(0, Ordering::SeqCst);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n > 0 {
                    REQ_COUNT.fetch_add(1, Ordering::SeqCst);
                    // Add a tiny delay to ensure both callers hit the in-flight window
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    let body = r#"{"last":69000,"open":68000,"high":70000,"low":67000,"quoteVolume":500000,"percentage":1.47}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                }
            }
        });

        let mut cfg1 = base_cfg();
        cfg1.rest_base_url = format!("http://127.0.0.1:{port}");
        cfg1.symbol = "COALESCE/USDT".to_string();

        let cfg2 = cfg1.clone();

        // Spawn two concurrent requests for the same symbol/exchange
        let handle1 = tokio::spawn(async move {
            fetch_ticker_snapshot(&cfg1, "Chart").await
        });
        let handle2 = tokio::spawn(async move {
            fetch_ticker_snapshot(&cfg2, "Execution").await
        });

        let res1 = handle1.await.unwrap();
        let res2 = handle2.await.unwrap();

        assert!(res1.is_some());
        assert!(res2.is_some());
        assert_eq!(res1.unwrap()["last"].as_f64(), Some(69000.0));
        assert_eq!(res2.unwrap()["last"].as_f64(), Some(69000.0));

        // Only 1 outbound HTTP request was made because in-flight coalescing deduplicated them
        assert_eq!(REQ_COUNT.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
