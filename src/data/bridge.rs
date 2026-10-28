/// CoinWatch WebSocket gateway client.
///
/// Connects to `wss://ws.coinwat.ch/ws` and subscribes to market channels:
///
/// - `ticker`    → [`UnifiedTick`] stream (real-time price updates)
/// - `trades`    → [`UnifiedTick`] stream
/// - `orderbook` → [`OrderBook`] snapshot stream
/// - `kline`     → [`OhlcvCandle`] stream (server pushes ~every 2 s)
///
/// OHLCV history (200 candles) is fetched once via REST at startup so the
/// chart has data immediately; live kline updates then merge on top.
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

use crate::data::client::{service_base_url, tokio_runtime};
use crate::data::models::OhlcvCandle;
use crate::data::orderbook::{OrderBook, PriceLevel};
use crate::data::tick::{TradeSide, UnifiedTick};

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
    /// Optional initial REST snapshot channel and its CoinWatch dashboard limit.
    pub snapshot: Option<(&'static str, u32, &'static str)>,
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
            snapshot: None,
        }
    }

    pub fn production(exchange: &str, symbol: &str) -> Self {
        Self::default_dev(exchange, symbol)
    }

    /// CoinWatch market stream endpoint. Override only for local gateway work.
    pub fn ws_url(&self) -> String {
        std::env::var("FINTER_WS_URL").unwrap_or_else(|_| {
            let base = self.rest_base_url.trim_end_matches('/');
            if base.contains("coinwat.ch") {
                "wss://ws.coinwat.ch/ws".to_string()
            } else if let Some(origin) = base.strip_prefix("https://") {
                format!("wss://{origin}/ws")
            } else if let Some(origin) = base.strip_prefix("http://") {
                format!("ws://{origin}/ws")
            } else {
                "wss://ws.coinwat.ch/ws".to_string()
            }
        })
    }

    /// Gateway expects punctuation-delimited symbols as dashes, not slash pairs.
    fn ws_symbol(&self) -> String {
        self.symbol
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect()
    }

    /// REST URL for the initial 200-candle history fetch.
    fn timeframe_to_ms(timeframe: &str) -> i64 {
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
            // SvelteKit routes to proxyToLive if either 'since' or 'until' is present
            let interval_ms = Self::timeframe_to_ms(&self.timeframe);
            let limit = self.ohlcv_limit as i64;
            let since = u - limit * interval_ms;
            url.push_str(&format!("&since={}&until={}", since, u));
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
    /// Cached full OHLCV history — replayed to new subscribers immediately.
    last_ohlcv_history: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
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
        let cancel_token = CancellationToken::new();
        let last_ohlcv_history: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>> =
            Arc::new(std::sync::Mutex::new(None));

        let tick_tx = tick_sender.clone();
        let trade_tx = trade_sender.clone();
        let ob_tx = ob_sender.clone();
        let ohlcv_tx = ohlcv_sender.clone();
        let token = cancel_token.clone();
        let history_cache = Arc::clone(&last_ohlcv_history);

        let supervisor = tokio_runtime().spawn(async move {
            supervision_loop(config, tick_tx, trade_tx, ob_tx, ohlcv_tx, token, history_cache).await;
        });

        Ok(Self {
            tick_sender,
            trade_sender,
            ob_sender,
            ohlcv_sender,
            last_ohlcv_history,
            cancel_token,
            _supervisor: supervisor,
        })
    }

    /// Subscribe to the trade tick stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<UnifiedTick>> {
        self.tick_sender.subscribe()
    }

    /// Subscribe to actual executed market trades stream.
    pub fn subscribe_trades(&self) -> broadcast::Receiver<Arc<UnifiedTick>> {
        self.trade_sender.subscribe()
    }

    /// Subscribe to the order book snapshot stream.
    pub fn subscribe_orderbook(&self) -> broadcast::Receiver<Arc<OrderBook>> {
        self.ob_sender.subscribe()
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

    /// Number of active trade tick subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tick_sender.receiver_count()
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
    cancel_token: CancellationToken,
    history_cache: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
) {
    let mut initial_snapshot_done = false;
    loop {
        if cancel_token.is_cancelled() {
            info!("gateway: supervision loop cancelled");
            return;
        }

        info!(
            "gateway: connecting to {} ({} {})",
            config.ws_url(),
            config.exchange,
            config.symbol
        );

        match connect_and_stream(
            &config,
            &tick_tx,
            &trade_tx,
            &ob_tx,
            &ohlcv_tx,
            &cancel_token,
            &history_cache,
            !initial_snapshot_done,
        )
        .await
        {
            Ok(_) => info!("gateway: WebSocket closed cleanly"),
            Err(e) => error!("gateway: WebSocket error: {e}"),
        }
        initial_snapshot_done = true;

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

/// Outbound subscribe message.
#[derive(serde::Serialize)]
struct SubscribeMsg<'a> {
    action: &'a str,
    channel: &'a str,
    exchange: &'a str,
    symbol: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeframe: Option<&'a str>,
}

/// Top-level envelope for every message from the gateway.
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
    #[serde(default, alias = "id", alias = "a")]
    pub id: Option<String>,
    #[serde(default, alias = "t", alias = "T", alias = "time")]
    pub timestamp: i64,
    #[serde(default, alias = "s", alias = "S")]
    pub side: String,
    #[serde(default, alias = "p", alias = "px")]
    pub price: f64,
    #[serde(default, alias = "q", alias = "qty", alias = "size", alias = "sz")]
    pub amount: f64,
}

/// Order book payload in an `orderbook` data push.
#[derive(serde::Deserialize, Debug)]
struct GatewayOrderBook {
    bids: Vec<[f64; 2]>,
    asks: Vec<[f64; 2]>,
    timestamp: Option<i64>,
}

async fn connect_and_stream(
    config: &BridgeConfig,
    tick_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    cancel_token: &CancellationToken,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
    should_fetch_initial_snapshot: bool,
) -> anyhow::Result<()> {
    // Only chart and ticker panes ask CoinWatch for an OHLCV REST snapshot.
    // Other panes are stream-only; requesting chart history for each of them
    // produced traffic (and credit deductions) the web dashboard never makes.
    if config.fetch_history && should_fetch_initial_snapshot {
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

    // REST seeds a bridge once, not once per socket connection. In particular,
    // trades reconnects must not replay a paid limit-60 snapshot into the tape.
    if should_fetch_initial_snapshot {
        if let Some((channel, limit, module)) = config.snapshot {
            match channel {
            "ticker" => {
                let url = config.ticker_snapshot_url(module);
                let token = crate::data::session_store::load_session().ok().flatten().map(|s| s.access_token).or_else(|| config.access_token.clone());
                let mut req = reqwest::Client::new().get(&url).header("Origin", &config.rest_base_url);
                if let Some(t) = token { req = req.bearer_auth(t); }
                // A failed bootstrap is intentionally non-fatal: the live WS ticker
                // subscription below remains the source of subsequent updates.
                if let Ok(resp) = req.send().await {
                    if resp.status().is_success() {
                        if let Ok(data) = resp.json::<serde_json::Value>().await {
                            let envelope = serde_json::json!({
                                "type": "data",
                                "channel": "ticker",
                                "exchange": config.exchange,
                                "symbol": config.symbol,
                                "data": data,
                            });
                            handle_message(&envelope.to_string(), config, tick_tx, trade_tx, ob_tx, ohlcv_tx, history_cache);
                        }
                    } else {
                        warn!("gateway: ticker bootstrap returned HTTP {}", resp.status());
                    }
                }
            }
            "orderbook" => {
                let url = format!("{}/api/v1/orderbook?exchange={}&symbol={}&limit={}&module={}&source=native",
                    config.rest_base_url,
                    urlencoding::encode(&config.exchange.to_lowercase()),
                    urlencoding::encode(&config.symbol),
                    limit,
                    urlencoding::encode(&format!("{} · {} (limit {})", module, config.symbol, limit)),
                );
                let token = crate::data::session_store::load_session().ok().flatten().map(|s| s.access_token).or_else(|| config.access_token.clone());
                let mut req = reqwest::Client::new().get(&url).header("Origin", &config.rest_base_url);
                if let Some(t) = token { req = req.bearer_auth(t); }
                if let Ok(resp) = req.send().await {
                    if let Ok(val) = resp.json::<serde_json::Value>().await {
                        let bids = val.get("bids").and_then(|v| v.as_array()).map(|arr| {
                            arr.iter().filter_map(|row| {
                                let r = row.as_array()?;
                                Some(PriceLevel::new(r.get(0)?.as_f64()?, r.get(1)?.as_f64()?))
                            }).collect()
                        }).unwrap_or_default();
                        let asks = val.get("asks").and_then(|v| v.as_array()).map(|arr| {
                            arr.iter().filter_map(|row| {
                                let r = row.as_array()?;
                                Some(PriceLevel::new(r.get(0)?.as_f64()?, r.get(1)?.as_f64()?))
                            }).collect()
                        }).unwrap_or_default();
                        let book = Arc::new(OrderBook::new(
                            config.exchange.clone(),
                            config.symbol.clone(),
                            bids,
                            asks,
                            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0),
                        ));
                        let _ = ob_tx.send(book);
                    }
                }
            }
            "trades" => {
                let url = format!("{}/api/v1/trades?exchange={}&symbol={}&limit={}&module={}&source=native",
                    config.rest_base_url,
                    urlencoding::encode(&config.exchange.to_lowercase()),
                    urlencoding::encode(&config.symbol),
                    limit,
                    urlencoding::encode(&format!("{} · {} (limit {})", module, config.symbol, limit)),
                );
                let token = crate::data::session_store::load_session().ok().flatten().map(|s| s.access_token).or_else(|| config.access_token.clone());
                let mut req = reqwest::Client::new().get(&url).header("Origin", &config.rest_base_url);
                if let Some(t) = token { req = req.bearer_auth(t); }
                if let Ok(resp) = req.send().await {
                    if let Ok(val) = resp.json::<serde_json::Value>().await {
                        let trades = val.get("trades").and_then(|v| v.as_array()).or_else(|| val.as_array());
                        if let Some(arr) = trades {
                            for item in arr.iter().rev() {
                                if let (Some(p), Some(a)) = (
                                    item.get("price").and_then(|v| v.as_f64()),
                                    item.get("amount").and_then(|v| v.as_f64()),
                                ) {
                                    let raw_side = item.get("side").and_then(|v| v.as_str()).unwrap_or("unknown");
                                    let side = if raw_side.contains("buy") { TradeSide::Buy } else if raw_side.contains("sell") { TradeSide::Sell } else { TradeSide::Unknown };
                                    let ts = item.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
                                    let tick = UnifiedTick::new(&config.exchange, &config.symbol, p, a, side, ts);
                                    let _ = trade_tx.send(Arc::new(tick));
                                }
                            }
                        }
                    }
                }
            }
                _ => {}
            }
        }
    }
    let url = config.ws_url();
    let (ws_stream, _) = connect_async(&url).await?;
    info!("gateway: WebSocket connected to {url}");

    let (mut write, mut read) = ws_stream.split();

    // Gateway handles subscription control messages serially. Sending several
    // messages before it acknowledges each one silently drops later channels.
    let ws_symbol = config.ws_symbol();
    let mut subscriptions = Vec::new();
    if config.ticker {
        subscriptions.push(SubscribeMsg { action: "subscribe", channel: "ticker", exchange: &config.exchange, symbol: &ws_symbol, timeframe: None });
    }
    if config.kline {
        subscriptions.push(SubscribeMsg { action: "subscribe", channel: "kline", exchange: &config.exchange, symbol: &ws_symbol, timeframe: Some(&config.timeframe) });
    }
    if config.orderbook {
        subscriptions.push(SubscribeMsg { action: "subscribe", channel: "orderbook", exchange: &config.exchange, symbol: &ws_symbol, timeframe: None });
    }
    if config.trades {
        subscriptions.push(SubscribeMsg { action: "subscribe", channel: "trades", exchange: &config.exchange, symbol: &ws_symbol, timeframe: None });
    }
    for msg in subscriptions {
        write.send(Message::Text(serde_json::to_string(&msg)?)).await?;
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    let ack: serde_json::Value = serde_json::from_str(&text)?;
                    let is_info = ack.get("type").and_then(serde_json::Value::as_str) == Some("info");
                    let is_sub = ack.get("action").and_then(serde_json::Value::as_str) == Some("subscribed");
                    let is_ok = ack.get("status").and_then(serde_json::Value::as_str) == Some("ok");
                    if is_info || is_sub || is_ok {
                        break;
                    }
                    if ack.get("type").and_then(serde_json::Value::as_str) == Some("error") || ack.get("error").is_some() {
                        let err_msg = ack.get("message")
                            .or_else(|| ack.get("error"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown error");
                        anyhow::bail!("gateway subscription failed: {}", err_msg);
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => anyhow::bail!("gateway closed while waiting for subscription acknowledgement"),
            }
        }
    }
    debug!("gateway: subscriptions acknowledged (ticker, kline/{}, orderbook, trades)", config.timeframe);

    // Heartbeat ticker to keep the connection alive (matching CoinWatch client interval).
    let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(15));
    heartbeat_interval.tick().await; // consume immediate first tick

    // Message loop.
    loop {
        tokio::select! {
            _ = heartbeat_interval.tick() => {
                if !config.ticker && !config.kline && !config.orderbook && !config.trades {
                    continue;
                }
                let heartbeat = SubscribeMsg {
                    action: "subscribe",
                    channel: if config.ticker { "ticker" } else if config.kline { "kline" } else if config.orderbook { "orderbook" } else { "trades" },
                    exchange: &config.exchange,
                    symbol: &ws_symbol,
                    timeframe: if config.kline && !config.ticker { Some(&config.timeframe) } else { None },
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
                        handle_message(&text, config, tick_tx, trade_tx, ob_tx, ohlcv_tx, history_cache);
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

fn handle_message(
    text: &str,
    config: &BridgeConfig,
    tick_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    trade_tx: &broadcast::Sender<Arc<UnifiedTick>>,
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
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
            warn!("gateway: server error: {m}");
        }
        return;
    }

    // Data push — dispatch by channel
    let Some(data) = msg.data else { return };

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
                    .get("quoteVolume")
                    .or_else(|| target.get("q"))
                    .or_else(|| target.get("volume"))
                    .or_else(|| target.get("v"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

                tick.change_24h_percent = change_24h;
                tick.quote_volume_24h = volume_24h;
                tick.open_24h = open_24h;
                tick.high_24h = high_24h;
                tick.low_24h = low_24h;
                let _ = tick_tx.send(Arc::new(tick));
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

            for trade in trades_list {
                if trade.price <= 0.0 || trade.amount <= 0.0 {
                    continue;
                }
                let raw_side = trade.side.to_lowercase();
                let side = if raw_side.contains("buy") || raw_side == "b" || raw_side == "bid" {
                    TradeSide::Buy
                } else if raw_side.contains("sell") || raw_side == "s" || raw_side == "ask" {
                    TradeSide::Sell
                } else {
                    TradeSide::Unknown
                };

                let raw_ts = trade.timestamp;
                let ts = if raw_ts > 0 && raw_ts < 10_000_000_000 { raw_ts * 1000 } else { raw_ts };

                let tick = Arc::new(UnifiedTick::new(
                    msg.exchange.as_deref().unwrap_or(&config.exchange),
                    msg.symbol.as_deref().unwrap_or(&config.symbol),
                    trade.price,
                    trade.amount,
                    side,
                    ts,
                ));

                let _ = trade_tx.send(tick);
            }
        }

        Some("orderbook") => {
            if ob_tx.receiver_count() == 0 {
                return;
            }
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
                    let ts = raw.timestamp.unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    });
                    let exchange = msg.exchange.unwrap_or_else(|| config.exchange.clone());
                    let symbol = msg.symbol.unwrap_or_else(|| config.symbol.clone());
                    let _ = ob_tx.send(Arc::new(OrderBook::new(exchange, symbol, bids, asks, ts)));
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
        let t_val = obj.get("t")
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

pub async fn fetch_ohlcv_history(config: &BridgeConfig, until: Option<i64>) -> anyhow::Result<Vec<OhlcvCandle>> {
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

    let mut req = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Origin", &config.rest_base_url)
        .header("Referer", format!("{}/", &config.rest_base_url));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let response = req.timeout(Duration::from_secs(30)).send().await?;
    if !response.status().is_success() {
        let status = response.status();
        let error: serde_json::Value = response.json().await.unwrap_or_default();
        let message = error.get("message").and_then(|v| v.as_str()).unwrap_or("Market data request failed");
        if status.as_u16() == 402 {
            anyhow::bail!("Insufficient credits: {} (balance {}, required {}). Manage credits at {}/account/credits", message,
                error.get("balance").and_then(|v| v.as_i64()).unwrap_or(0),
                error.get("required").and_then(|v| v.as_i64()).unwrap_or(0), config.rest_base_url);
        }
        anyhow::bail!("OHLCV history fetch failed: HTTP {}: {}", status, message);
    }

    let value: serde_json::Value = response.json().await?;
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
            snapshot: None,
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
    fn ws_url_derives_from_rest_base() {
        assert_eq!(base_cfg().ws_url(), "wss://ws.coinwat.ch/ws");
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

    #[tokio::test]
    async fn ticker_bootstrap_seeds_feed_and_is_skipped_on_reconnect() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = base_cfg();
        cfg.rest_base_url = format!("http://{}", listener.local_addr().unwrap());
        cfg.fetch_history = false;
        cfg.snapshot = Some(("ticker", 0, "Watchlist"));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /api/v1/ticker?"));
            let body = r#"{"last":110,"open":100,"high":120,"low":90,"quoteVolume":1234,"percentage":10}"#;
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            drop(socket);
            // Close incoming WebSocket attempts immediately; connect_and_stream fails cleanly
            if let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        let (ticks, mut rx) = broadcast::channel(8);
        let (trades, _) = broadcast::channel(8);
        let (books, _) = broadcast::channel(8);
        let (candles, _) = broadcast::channel(8);
        let cache = Arc::new(std::sync::Mutex::new(None));
        let cancel = CancellationToken::new();
        let _ = connect_and_stream(&cfg, &ticks, &trades, &books, &candles, &cancel, &cache, true).await;
        server.await.unwrap();
        let tick = rx.try_recv().expect("REST response must seed the watchlist feed");
        assert_eq!(tick.price, 110.0);
        assert_eq!(tick.open_24h, Some(100.0));
        assert_eq!(tick.high_24h, Some(120.0));
        assert_eq!(tick.low_24h, Some(90.0));
        assert_eq!(tick.quote_volume_24h, Some(1234.0));
        assert_eq!(tick.change_24h_percent, Some(10.0));
        let _ = connect_and_stream(&cfg, &ticks, &trades, &books, &candles, &cancel, &cache, false).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn ticker_bootstrap_failure_continues_to_ws() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = base_cfg();
        cfg.rest_base_url = format!("http://{}", listener.local_addr().unwrap());
        cfg.fetch_history = false;
        cfg.snapshot = Some(("ticker", 0, "Watchlist"));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let n = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /api/v1/ticker?"));
            let body = r#"{"error":"service_unavailable"}"#;
            socket.write_all(format!("HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            drop(socket);
            if let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        let (ticks, mut rx) = broadcast::channel(8);
        let (trades, _) = broadcast::channel(8);
        let (books, _) = broadcast::channel(8);
        let (candles, _) = broadcast::channel(8);
        let cache = Arc::new(std::sync::Mutex::new(None));
        let cancel = CancellationToken::new();
        let _ = connect_and_stream(&cfg, &ticks, &trades, &books, &candles, &cancel, &cache, true).await;
        server.await.unwrap();
        // Since REST returned 500, no tick was seeded from REST, but connect_and_stream finished without panicking
        assert!(rx.try_recv().is_err());
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
        assert_eq!(trades[0].side, "buy");
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
}
