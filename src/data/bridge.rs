/// CoinWatch WebSocket gateway client.
///
/// Connects to `wss://coinwat.ch/api/v1/ws` and subscribes to three channels:
///
/// - `trades`    → [`UnifiedTick`] stream (server pushes ~every 2 s)
/// - `orderbook` → [`OrderBook`] snapshot stream (server pushes ~every 2 s)
/// - `kline`     → [`OhlcvCandle`] stream (server pushes ~every 2 s)
///
/// OHLCV history (200 candles) is fetched once via REST at startup so the
/// chart has data immediately; live kline updates then merge on top.
///
/// Dropping the handle cancels the supervision loop and closes the connection.
use std::sync::Arc;

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
        }
    }

    pub fn production(exchange: &str, symbol: &str) -> Self {
        Self::default_dev(exchange, symbol)
    }

    /// CoinWatch market stream endpoint. Override only for local gateway work.
    pub fn ws_url(&self) -> String {
        std::env::var("FINTER_WS_URL").unwrap_or_else(|_| {
            let base = self.rest_base_url.trim_end_matches('/');
            if let Some(origin) = base.strip_prefix("https://") {
                format!("wss://{origin}/api/v1/ws")
            } else if let Some(origin) = base.strip_prefix("http://") {
                format!("ws://{origin}/api/v1/ws")
            } else {
                String::new()
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

    pub fn ohlcv_history_url(&self, until: Option<i64>) -> String {
        let mut url = format!(
            "{}/api/v1/ohlcv?exchange={}&symbol={}&timeframe={}&limit={}",
            self.rest_base_url,
            urlencoding::encode(&self.exchange),
            urlencoding::encode(&self.symbol),
            urlencoding::encode(&self.timeframe),
            self.ohlcv_limit,
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
        let (ob_sender, _) = broadcast::channel(OB_BROADCAST_CAPACITY);
        let (ohlcv_sender, _) = broadcast::channel(OHLCV_BROADCAST_CAPACITY);
        let cancel_token = CancellationToken::new();
        let last_ohlcv_history: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>> =
            Arc::new(std::sync::Mutex::new(None));

        let tick_tx = tick_sender.clone();
        let ob_tx = ob_sender.clone();
        let ohlcv_tx = ohlcv_sender.clone();
        let token = cancel_token.clone();
        let history_cache = Arc::clone(&last_ohlcv_history);

        let supervisor = tokio_runtime().spawn(async move {
            supervision_loop(config, tick_tx, ob_tx, ohlcv_tx, token, history_cache).await;
        });

        Ok(Self {
            tick_sender,
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
    ob_tx: broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    cancel_token: CancellationToken,
    history_cache: Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
) {
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
            &ob_tx,
            &ohlcv_tx,
            &cancel_token,
            &history_cache,
        )
        .await
        {
            Ok(_) => info!("gateway: WebSocket closed cleanly"),
            Err(e) => error!("gateway: WebSocket error: {e}"),
        }

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
#[derive(serde::Deserialize, Debug)]
struct GatewayMessage {
    /// Set on subscription ack: `"info"` / `"data"`.
    #[serde(default)]
    #[serde(rename = "type")]
    msg_type: Option<String>,
    /// Set on data pushes: `"trades"` / `"orderbook"` / `"kline"` / `"ticker"`.
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    exchange: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    timeframe: Option<String>,
    /// Payload — shape depends on `channel`.
    #[serde(default, alias = "payload")]
    data: Option<serde_json::Value>,
    /// Server-side error string or info message.
    #[serde(default)]
    message: Option<String>,
}

/// A single trade entry in a `trades` data push.
#[derive(serde::Deserialize, Debug)]
struct GatewayTrade {
    timestamp: i64,
    side: String,
    price: f64,
    amount: f64,
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
    ob_tx: &broadcast::Sender<Arc<OrderBook>>,
    ohlcv_tx: &broadcast::Sender<Arc<Vec<OhlcvCandle>>>,
    cancel_token: &CancellationToken,
    history_cache: &Arc<std::sync::Mutex<Option<Arc<Vec<OhlcvCandle>>>>>,
) -> anyhow::Result<()> {
    // Fetch full OHLCV history via REST before opening the WebSocket so the
    // chart has something to show immediately.
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

    // Connect WebSocket.
    let url = config.ws_url();
    let (ws_stream, _) = connect_async(&url).await?;
    info!("gateway: WebSocket connected to {url}");

    let (mut write, mut read) = ws_stream.split();

    // Gateway handles subscription control messages serially. Sending several
    // messages before it acknowledges each one silently drops later channels.
    let ws_symbol = config.ws_symbol();
    for msg in [
        SubscribeMsg {
            action: "subscribe",
            channel: "ticker",
            exchange: &config.exchange,
            symbol: &ws_symbol,
            timeframe: None,
        },
        SubscribeMsg {
            action: "subscribe",
            channel: "kline",
            exchange: &config.exchange,
            symbol: &ws_symbol,
            timeframe: Some(&config.timeframe),
        },
    ] {
        write.send(Message::Text(serde_json::to_string(&msg)?)).await?;
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    let ack: serde_json::Value = serde_json::from_str(&text)?;
                    match ack.get("type").and_then(serde_json::Value::as_str) {
                        Some("info") => break,
                        Some("error") => anyhow::bail!(
                            "gateway subscription failed: {}",
                            ack.get("message").and_then(serde_json::Value::as_str).unwrap_or("unknown error")
                        ),
                        _ => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => anyhow::bail!("gateway closed while waiting for subscription acknowledgement"),
            }
        }
    }
    debug!("gateway: subscriptions acknowledged (ticker, kline/{})", config.timeframe);

    // Message loop.
    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_message(&text, config, tick_tx, ob_tx, ohlcv_tx, history_cache);
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
            info!("gateway: info message: {m}");
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
            if let Some(price) = data.get("c").or_else(|| data.get("last")).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))) {
                let tick = UnifiedTick::new(
                    msg.exchange.as_deref().unwrap_or(&config.exchange),
                    msg.symbol.as_deref().unwrap_or(&config.symbol),
                    price,
                    0.0,
                    TradeSide::Unknown,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                );
                let _ = tick_tx.send(Arc::new(tick));
            }
        }

        Some("trades") => {
            if tick_tx.receiver_count() == 0 {
                return;
            }
            match serde_json::from_value::<Vec<GatewayTrade>>(data) {
                Ok(trades) => {
                    for trade in trades {
                        let side = match trade.side.as_str() {
                            "buy" | "Buy" => TradeSide::Buy,
                            "sell" | "Sell" => TradeSide::Sell,
                            _ => TradeSide::Unknown,
                        };
                        let tick = UnifiedTick::new(
                            msg.exchange.as_deref().unwrap_or(&config.exchange),
                            msg.symbol.as_deref().unwrap_or(&config.symbol),
                            trade.price,
                            trade.amount,
                            side,
                            trade.timestamp,
                        );
                        let _ = tick_tx.send(Arc::new(tick));
                    }
                }
                Err(e) => warn!("gateway: trades data parse error: {e}"),
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

        Some("kline") => {
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
        if arr.len() >= 6 {
            let t = arr[0].as_i64().or_else(|| arr[0].as_str().and_then(|s| s.parse().ok()))?;
            let o = arr[1].as_f64().or_else(|| arr[1].as_str().and_then(|s| s.parse().ok()))?;
            let h = arr[2].as_f64().or_else(|| arr[2].as_str().and_then(|s| s.parse().ok()))?;
            let l = arr[3].as_f64().or_else(|| arr[3].as_str().and_then(|s| s.parse().ok()))?;
            let c = arr[4].as_f64().or_else(|| arr[4].as_str().and_then(|s| s.parse().ok()))?;
            let v = arr[5].as_f64().or_else(|| arr[5].as_str().and_then(|s| s.parse().ok()))?;
            let timestamp = if t > 0 && t < 10_000_000_000 { t * 1000 } else { t };
            return Some(OhlcvCandle { timestamp, open: o, high: h, low: l, close: c, volume: v });
        }
    }

    if let Some(obj) = val.as_object() {
        if let (Some(t_val), Some(o_val), Some(h_val), Some(l_val), Some(c_val), Some(v_val)) = (
            obj.get("t").or_else(|| obj.get("timestamp")),
            obj.get("o").or_else(|| obj.get("open")),
            obj.get("h").or_else(|| obj.get("high")),
            obj.get("l").or_else(|| obj.get("low")),
            obj.get("c").or_else(|| obj.get("close")),
            obj.get("v").or_else(|| obj.get("volume")),
        ) {
            let t = t_val.as_i64().or_else(|| t_val.as_str().and_then(|s| s.parse().ok()))?;
            let o = o_val.as_f64().or_else(|| o_val.as_str().and_then(|s| s.parse().ok()))?;
            let h = h_val.as_f64().or_else(|| h_val.as_str().and_then(|s| s.parse().ok()))?;
            let l = l_val.as_f64().or_else(|| l_val.as_str().and_then(|s| s.parse().ok()))?;
            let c = c_val.as_f64().or_else(|| c_val.as_str().and_then(|s| s.parse().ok()))?;
            let v = v_val.as_f64().or_else(|| v_val.as_str().and_then(|s| s.parse().ok()))?;
            let timestamp = if t > 0 && t < 10_000_000_000 { t * 1000 } else { t };
            return Some(OhlcvCandle { timestamp, open: o, high: h, low: l, close: c, volume: v });
        }
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
        return out;
    }

    if let Some(arr) = data.as_array() {
        if !arr.is_empty() && arr[0].is_array() {
            for item in arr {
                if let Some(candle) = parse_single_candle(item) {
                    out.push(candle);
                }
            }
        } else if let Some(candle) = parse_single_candle(data) {
            out.push(candle);
        }
        return out;
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
    debug!("gateway: GET {url}");

    let mut req = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .header("Origin", &config.rest_base_url)
        .header("Referer", format!("{}/", &config.rest_base_url));
    if let Ok(Some(session)) = crate::data::session_store::load_session() {
        req = req.bearer_auth(session.access_token);
    } else if let Some(token) = &config.access_token {
        req = req.bearer_auth(token);
    }
    let response = req.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("OHLCV history fetch failed: HTTP {}", response.status());
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
    } else if let Ok(parsed) = serde_json::from_value::<Vec<OhlcvCandle>>(
        value.get("data").cloned().unwrap_or_else(|| value.get("candles").cloned().unwrap_or(value.clone())),
    ) {
        candles = parsed;
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
        assert_eq!(base_cfg().ws_url(), "wss://coinwat.ch/api/v1/ws");
    }

    #[test]
    fn ws_url_works_with_http_scheme() {
        let mut cfg = base_cfg();
        cfg.rest_base_url = "http://localhost:3000".to_string();
        assert_eq!(cfg.ws_url(), "ws://localhost:3000/api/v1/ws");
    }

    #[test]
    fn ohlcv_history_url() {
        let url = base_cfg().ohlcv_history_url(None);
        assert!(url.contains("timeframe=1m"), "url={url}");
        assert!(url.contains("limit=500"), "url={url}");
        assert!(url.contains("symbol=BTC%2FUSDT"), "url={url}");
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
}
