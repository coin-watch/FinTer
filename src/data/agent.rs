//! OpenAI-compatible dashboard analysis and trading client with automatic model discovery.
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::data::models::OhlcvCandle;
use crate::data::trade_store::{
    self, MarketType, TradeOrder, TradeOrderType, TradeSide, TradeStore, TradingMode,
};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChatMessageRecord {
    pub prompt: String,
    pub response: String,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Receipts for the trade directives in `response`, captured the single time
    /// they were executed. Re-parsing or reloading never re-executes a trade.
    #[serde(default)]
    pub trade_receipts: Vec<TradeActionResult>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConversationRecord {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub updated_at_ts: i64,
    pub messages: Vec<ChatMessageRecord>,
}

fn conversations_file_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("finter/agent_conversations.json"))
}

/// Loads all saved conversations sorted from newest to oldest.
pub fn load_all_conversations() -> Vec<ConversationRecord> {
    let Some(path) = conversations_file_path() else { return Vec::new(); };
    let Ok(bytes) = std::fs::read(&path) else {
        // Also check if any legacy individual files exist in agent-history/ and import them
        let mut migrated = Vec::new();
        if let Some(dir) = dirs::config_dir().map(|p| p.join("finter/agent-history")) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(data) = std::fs::read(&file_path) {
                            #[derive(serde::Deserialize)]
                            struct LegacyMsg { prompt: String, response: String, #[serde(default)] failed: bool }
                            if let Ok(msgs) = serde_json::from_slice::<Vec<LegacyMsg>>(&data) {
                                if !msgs.is_empty() {
                                    let id = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("migrated").to_string();
                                    let title = msgs.first().map(|m| m.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or(&m.prompt).chars().take(40).collect::<String>()).unwrap_or_else(|| "Past Conversation".into());
                                    let now = chrono::Local::now();
                                    migrated.push(ConversationRecord {
                                        id,
                                        title,
                                        updated_at: now.format("%b %d, %H:%M").to_string(),
                                        updated_at_ts: now.timestamp_millis(),
                                        messages: msgs.into_iter().map(|m| ChatMessageRecord {
                                            prompt: m.prompt,
                                            response: m.response,
                                            failed: m.failed,
                                            timestamp: None,
                                            // Legacy history is display-only: never replay
                                            // past trade directives on migration.
                                            trade_receipts: Vec::new(),
                                        }).collect(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        if !migrated.is_empty() {
            let _ = save_all_conversations(&migrated);
        }
        return migrated;
    };
    let mut list = serde_json::from_slice::<Vec<ConversationRecord>>(&bytes).unwrap_or_default();
    list.sort_by(|a, b| b.updated_at_ts.cmp(&a.updated_at_ts));
    list
}

pub fn load_conversation(id: &str) -> Option<ConversationRecord> {
    load_all_conversations().into_iter().find(|c| c.id == id)
}

pub fn save_conversation(conv: &ConversationRecord) -> Result<()> {
    let mut list = load_all_conversations();
    if let Some(existing) = list.iter_mut().find(|c| c.id == conv.id) {
        *existing = conv.clone();
    } else {
        list.insert(0, conv.clone());
    }
    list.sort_by(|a, b| b.updated_at_ts.cmp(&a.updated_at_ts));
    save_all_conversations(&list)
}

pub fn delete_conversation(id: &str) -> Result<()> {
    let mut list = load_all_conversations();
    list.retain(|c| c.id != id);
    save_all_conversations(&list)
}

pub fn save_all_conversations(list: &[ConversationRecord]) -> Result<()> {
    use std::io::Write;
    let path = conversations_file_path().context("Config directory unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(list)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, path)?;
    Ok(())
}

const MAX_CONTEXT_BYTES: usize = 200_000;

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct LocalConfig {
    pub base_url: String,
    pub api_key: String,
}

pub fn provider_config() -> LocalConfig {
    let saved = dirs::config_dir().and_then(|p| std::fs::read(p.join("finter/agent_config.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<LocalConfig>(&bytes).ok());
    saved.unwrap_or_else(|| LocalConfig {
        base_url: std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
        api_key: std::env::var("FINTER_AI_API_KEY").unwrap_or_default(),
    })
}

pub fn save_provider_config(config: &LocalConfig) -> Result<()> {
    use std::io::Write;
    let dir = dirs::config_dir().context("Local configuration directory unavailable")?.join("finter");
    std::fs::create_dir_all(&dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(dir.join("agent_config.json"))?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(config)?)?;
    Ok(())
}

/// Discovers available models from OpenAI, OpenRouter, 9router, Ollama, and other OpenAI-compatible proxies.
pub async fn discover_models() -> Result<Vec<String>> {
    let cfg = provider_config();
    let key = if !cfg.api_key.trim().is_empty() {
        Some(cfg.api_key)
    } else {
        std::env::var("FINTER_AI_API_KEY").ok().filter(|k| !k.trim().is_empty())
    };
    let raw_base = if !cfg.base_url.trim().is_empty() {
        cfg.base_url
    } else {
        std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into())
    };
    let clean_base = raw_base
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/models");
    let url = format!("{}/models", clean_base);
    reqwest::Url::parse(&url).context("FINTER_AI_BASE_URL is invalid")?;

    let mut request = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()?
        .get(&url)
        .header("User-Agent", "FinTer/0.1.4")
        .header("HTTP-Referer", "https://finter.app")
        .header("X-Title", "FinTer");

    if let Some(ref k) = key {
        request = request.bearer_auth(k);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Model discovery failed ({status}): {err_text}"));
    }

    let value: Value = response.json().await?;
    let mut models = Vec::new();

    // 1. Check standard OpenAI / OpenRouter / 9router `{ "data": [ ... ] }`
    if let Some(arr) = value.get("data").and_then(Value::as_array) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(id) = item.get("name").and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                models.push(s.to_string());
            }
        }
    } else if let Some(arr) = value.get("models").and_then(Value::as_array) {
        // 2. Check Ollama or alternate `{ "models": [ ... ] }`
        for item in arr {
            if let Some(id) = item.get("name").or_else(|| item.get("id")).and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                models.push(s.to_string());
            }
        }
    } else if let Some(arr) = value.as_array() {
        // 3. Top-level array `[ { "id": ... } ]`
        for item in arr {
            if let Some(id) = item.get("id").or_else(|| item.get("name")).and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                models.push(s.to_string());
            }
        }
    }

    models.retain(|m| !m.trim().is_empty());
    models.sort();
    models.dedup();

    if models.is_empty() {
        return Err(anyhow!("Provider returned no models. Check the API base URL and credentials in Settings, then refresh models."));
    }

    Ok(models)
}

const SYSTEM_PROMPT: &str = r#"You are FinTer's autonomous trading agent. You analyse the supplied dashboard snapshot, distinguish facts from inference, explicitly identify missing/stale data, and you can execute trades directly on the FinTer trade desk. This is informational, not financial advice.

## Executing trades

You can place orders, cancel resting orders, and close open positions by emitting a fenced `trade` block. The application parses the JSON and executes it immediately against the FinTer trade store; the result (order id, status, fill price, quantity) is shown to the user as a receipt. Emit one fenced block per action, each on its own lines, and never wrap the JSON in extra prose or markdown inside the fence.

Place an order:

```trade
{"action":"place_order","symbol":"BTC/USDT","side":"buy","order_type":"market","margin":1000,"leverage":5,"market_type":"futures","mode":"paper"}
```

```trade
{"action":"place_order","symbol":"ETH/USDT","side":"sell","order_type":"limit","price":3200,"margin":500,"leverage":10,"market_type":"futures","mode":"paper"}
```

Cancel a resting order:

```trade
{"action":"cancel_order","order_id":"1a2b3c4d"}
```

Close an open position:

```trade
{"action":"close_position","pos_id":"1a2b3c4d","current_price":64250.5}
```

Allowed keys for `place_order`:
- action: "place_order" (required).
- symbol: BASE/QUOTE, for example BTC/USDT (required).
- side: buy, sell, long, or short. On spot, "sell" disposes of an existing holding instead of opening a short.
- order_type: market (default) or limit. Limit orders require `price` and rest as OPEN until the market reaches it.
- price: optional limit price. For market orders it is ignored; the desk uses the latest mark price.
- margin: quote-currency margin to commit (required, greater than 0).
- leverage: integer leverage, default 1. Spot orders must use 1x.
- market_type: spot or futures (margin is also accepted), default futures.
- mode: paper (default) or live. Only use live when the user explicitly asks for a live order.

Rules:
- Only emit a `trade` block when the user asks you to trade, or clearly approves a trade you proposed. Never trade speculatively or without a request.
- Default to paper mode unless the user explicitly asks to trade live.
- Keep `margin` within the account balance shown in the snapshot. If a trade would be rejected, explain why instead of emitting a block.
- After each `trade` block, briefly describe what you executed and why. The app reports the authoritative result; never invent an order id or fill.
- You may combine `trade` and `chart` blocks in one reply.

## Inline charts

You can render candlestick charts inline in this conversation. When the user asks to chart, plot, visualise, or inspect price action, emit a fenced block exactly like this, on its own lines:

```chart
{"exchange":"binance","symbol":"BTC/USDT","timeframe":"1h","candles":200}
```

For an explicit historical window, add `start` and `end`. The app fetches exactly that window from the exchange, paginating as needed (up to 5000 candles):

```chart
{"exchange":"binance","symbol":"BTC/USDT","timeframe":"1d","start":"2020-11-21","end":"2020-12-21"}
```

The application fetches the requested OHLCV data and renders a static chart at that point in your reply. Allowed keys:
- exchange: a CCXT venue id such as binance, binanceusdm, bybit, okx, kucoin, kucoinfutures.
- symbol: BASE/QUOTE, for example BTC/USDT.
- timeframe: 1m, 5m, 15m, 30m, 1h, 4h, 1d, 1w.
- style: optional visual style: candles (default), hollow, ohlc, area, or heikin_ashi.
- candles: optional integer between 1 and 500 (default 200). Used for the most recent snapshot when start/end are omitted.
- start: optional start of a historical window. Accepts ISO dates/times (2020-11-21, 2020-11-21T00:00:00Z) or Unix milliseconds.
- end: optional end of a historical window. Date-only values include the whole day.
- title: optional short label.

Without start/end, the app fetches the most recent `candles` snapshot. With start/end, it fetches the exact historical window, not just the latest candles. Use a larger timeframe for long ranges; the app rejects windows estimated above 5000 candles. You may place several chart blocks in one reply and continue your analysis before and after each. Never invent candle values; the app supplies the data. When the user does not name a market, reuse the exchange, symbol, and timeframe from the dashboard snapshot if available. Do not wrap the JSON in extra prose or markdown inside the fence."#;

/// Sends read-only dashboard context and user prompt to the selected model.
pub async fn analyze(prompt: &str, context: Value, selected_model: &str) -> Result<String> {
    let cfg = provider_config();
    let key = if !cfg.api_key.trim().is_empty() {
        Some(cfg.api_key)
    } else {
        std::env::var("FINTER_AI_API_KEY").ok().filter(|k| !k.trim().is_empty())
    };
    let raw_base = if !cfg.base_url.trim().is_empty() {
        cfg.base_url
    } else {
        std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into())
    };
    let clean_base = raw_base
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/models");
    let url = format!("{}/chat/completions", clean_base);
    reqwest::Url::parse(&url).context("FINTER_AI_BASE_URL is invalid")?;

    let model = if selected_model.trim().is_empty() || selected_model == "Select model" {
        std::env::var("FINTER_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into())
    } else {
        selected_model.to_string()
    };

    let mut serialized = serde_json::to_string(&context)?;
    if serialized.len() > MAX_CONTEXT_BYTES {
        let mut end = MAX_CONTEXT_BYTES;
        while !serialized.is_char_boundary(end) { end -= 1; }
        serialized.truncate(end);
        serialized.push_str("…[truncated]");
    }

    let body = json!({
        "model": model,
        // Some OpenAI-compatible proxies default to SSE streaming. We consume a
        // regular JSON response here because the UI performs its own typing effect.
        "stream": false,
        "messages": [
            {
                "role": "system",
                "content": SYSTEM_PROMPT
            },
            {
                "role": "user",
                "content": format!("Dashboard snapshot:\n{}\n\nUser request:\n{}", serialized, prompt)
            }
        ]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let mut request = client
        .post(&url)
        .header("User-Agent", "FinTer/0.1.4")
        .header("HTTP-Referer", "https://finter.app")
        .header("X-Title", "FinTer")
        .json(&body);

    if let Some(ref k) = key {
        request = request.bearer_auth(k);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        let is_credits_error = status.as_u16() == 402
            || (status.as_u16() == 429 && (err_text.to_lowercase().contains("quota") || err_text.to_lowercase().contains("credit")))
            || err_text.to_lowercase().contains("insufficient_quota")
            || err_text.to_lowercase().contains("insufficient_credits");

        if is_credits_error {
            crate::data::credits::CreditStore::record(
                "AI Agent",
                &format!("AI request failed: insufficient credits or quota ({status})"),
                0,
                0,
                None,
            );
        }
        return Err(anyhow!("AI request failed ({status}): {err_text}"));
    }

    let value: Value = response.json().await?;
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("AI provider returned no message content"))
}

// ---------------------------------------------------------------------------
// Trade directives
// ---------------------------------------------------------------------------

/// A structured trade action the assistant asked to execute.
///
/// The model emits a fenced `trade` block containing this JSON. The app parses
/// the block and executes it immediately against [`TradeStore::global`].
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TradeAction {
    /// Open an order (market executes immediately, limit rests as OPEN).
    PlaceOrder {
        #[serde(default)]
        symbol: String,
        #[serde(default)]
        side: String,
        #[serde(default, deserialize_with = "deserialize_optional_string")]
        order_type: Option<String>,
        #[serde(default, deserialize_with = "deserialize_optional_f64")]
        price: Option<f64>,
        #[serde(default, deserialize_with = "deserialize_optional_f64")]
        margin: Option<f64>,
        #[serde(default, deserialize_with = "deserialize_optional_f64")]
        leverage: Option<f64>,
        #[serde(default)]
        market_type: Option<String>,
        #[serde(default)]
        mode: Option<String>,
    },
    /// Cancel a resting OPEN order by id.
    CancelOrder {
        #[serde(default, alias = "id")]
        order_id: String,
    },
    /// Close an open position by id at an optional price.
    ClosePosition {
        #[serde(default, alias = "id", alias = "position_id")]
        pos_id: String,
        #[serde(default, deserialize_with = "deserialize_optional_f64")]
        current_price: Option<f64>,
    },
}

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    Ok(match raw {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text
            .trim()
            .trim_start_matches('$')
            .replace([',', '_'], "")
            .parse::<f64>()
            .ok(),
        _ => None,
    })
}

fn parse_trade_side(raw: &str) -> Option<TradeSide> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "buy" | "long" | "bid" => Some(TradeSide::Long),
        "sell" | "short" | "ask" => Some(TradeSide::Short),
        _ => None,
    }
}

fn parse_trading_mode(raw: Option<&str>) -> TradingMode {
    match raw.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "live" | "real" => TradingMode::Live,
        _ => TradingMode::Paper,
    }
}

impl TradeAction {
    /// Short label for the receipt header, e.g. "Place order".
    pub fn kind_label(&self) -> &'static str {
        match self {
            TradeAction::PlaceOrder { .. } => "Place order",
            TradeAction::CancelOrder { .. } => "Cancel order",
            TradeAction::ClosePosition { .. } => "Close position",
        }
    }

    /// True when the action targets the live ledger.
    pub fn is_live(&self) -> bool {
        match self {
            TradeAction::PlaceOrder { mode, .. } => parse_trading_mode(mode.as_deref()) == TradingMode::Live,
            _ => false,
        }
    }

    /// Validates the action before it touches the trade store.
    fn validate(&self) -> Result<()> {
        match self {
            TradeAction::PlaceOrder { symbol, side, order_type, price, margin, leverage, .. } => {
                if symbol.trim().is_empty() {
                    return Err(anyhow!("Trade directive is missing a symbol."));
                }
                if parse_trade_side(side).is_none() {
                    return Err(anyhow!(
                        "Invalid side '{side}': use buy, sell, long, or short."
                    ));
                }
                if let Some(order_type) = order_type.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                    match order_type.to_ascii_lowercase().as_str() {
                        "market" | "limit" => {}
                        other => return Err(anyhow!("Invalid order_type '{other}': use market or limit.")),
                    }
                }
                if order_type.as_deref().map(|t| t.trim().eq_ignore_ascii_case("limit")).unwrap_or(false)
                    && !price.map(|p| p.is_finite() && p > 0.0).unwrap_or(false)
                {
                    return Err(anyhow!("Limit orders require a positive price."));
                }
                if !margin.map(|m| m.is_finite() && m > 0.0).unwrap_or(false) {
                    return Err(anyhow!("Trade directive requires a positive margin."));
                }
                if let Some(leverage) = leverage {
                    if !leverage.is_finite() || *leverage < 1.0 {
                        return Err(anyhow!("Leverage must be at least 1."));
                    }
                }
                Ok(())
            }
            TradeAction::CancelOrder { order_id } => {
                if order_id.trim().is_empty() {
                    return Err(anyhow!("Cancel directive is missing an order_id."));
                }
                Ok(())
            }
            TradeAction::ClosePosition { pos_id, .. } => {
                if pos_id.trim().is_empty() {
                    return Err(anyhow!("Close directive is missing a pos_id."));
                }
                Ok(())
            }
        }
    }
}

/// The authoritative outcome of executing a [`TradeAction`].
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TradeActionResult {
    pub action: String,
    pub ok: bool,
    pub order_id: Option<String>,
    pub symbol: Option<String>,
    pub side: Option<String>,
    pub order_type: Option<String>,
    pub market_type: Option<String>,
    pub mode: Option<String>,
    pub status: Option<String>,
    pub price: Option<f64>,
    pub quantity: Option<f64>,
    pub notional: Option<f64>,
    pub leverage: Option<u32>,
    pub margin: Option<f64>,
    /// Realised P&L for a closed position.
    pub pnl: Option<f64>,
    pub message: String,
}

impl TradeActionResult {
    fn from_order(action: &str, order: &TradeOrder) -> Self {
        let side = match (order.market_type, order.side) {
            (MarketType::Spot, TradeSide::Short) => "sell",
            (_, TradeSide::Long) => "buy",
            (_, TradeSide::Short) => "sell",
        };
        Self {
            action: action.to_string(),
            ok: true,
            order_id: Some(order.id.clone()),
            symbol: Some(order.symbol.clone()),
            side: Some(side.to_string()),
            order_type: Some(match order.order_type {
                TradeOrderType::Market => "market",
                TradeOrderType::Limit => "limit",
            }
            .to_string()),
            market_type: Some(order.market_type.as_str().to_string()),
            mode: Some(order.mode.as_str().to_lowercase()),
            status: Some(order.status.to_string()),
            price: Some(order.price),
            quantity: Some(order.quantity),
            notional: Some(order.notional),
            leverage: Some(order.leverage),
            margin: Some(order.margin),
            pnl: None,
            message: format!("{} {} {} @ {}", order.status, order.symbol, side.to_uppercase(), order.price),
        }
    }

    fn failure(action: &str, error: impl std::fmt::Display) -> Self {
        Self {
            action: action.to_string(),
            ok: false,
            order_id: None,
            symbol: None,
            side: None,
            order_type: None,
            market_type: None,
            mode: None,
            status: None,
            price: None,
            quantity: None,
            notional: None,
            leverage: None,
            margin: None,
            pnl: None,
            message: error.to_string(),
        }
    }
}

/// Executes a parsed trade directive against the global trade store.
///
/// Market orders fill immediately at the supplied price (or the current mark
/// price when omitted); limit orders rest as OPEN. Spot sells dispose of an
/// existing holding instead of opening a short.
pub fn execute_trade_directive(action: &TradeAction) -> Result<TradeActionResult> {
    action.validate()?;
    let mut store = TradeStore::global()
        .lock()
        .map_err(|_| anyhow!("Trade store lock is poisoned."))?;

    match action {
        TradeAction::PlaceOrder {
            symbol,
            side,
            order_type,
            price,
            margin,
            leverage,
            market_type,
            mode,
        } => {
            let side = parse_trade_side(side)
                .ok_or_else(|| anyhow!("Invalid side '{side}'."))?;
            let order_type = match order_type.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(raw) if raw.eq_ignore_ascii_case("limit") => TradeOrderType::Limit,
                _ => TradeOrderType::Market,
            };
            let market_type = MarketType::from_str(market_type.as_deref().unwrap_or("futures"));
            let mode = parse_trading_mode(mode.as_deref());
            let margin = margin.ok_or_else(|| anyhow!("Trade directive requires a margin."))?;
            let leverage = leverage.unwrap_or(1.0).max(1.0).round() as u32;

            let symbol = trade_store::normalize_symbol(symbol);
            let exec_price = match (order_type, price) {
                (TradeOrderType::Limit, Some(price)) => *price,
                _ => store
                    .get_mark_price(&symbol)
                    .or(*price)
                    .ok_or_else(|| {
                        anyhow!(
                            "No market price available for {symbol}; supply a limit price or wait for a live quote."
                        )
                    })?,
            };

            let result = if market_type == MarketType::Spot && side == TradeSide::Short {
                // Spot sells dispose of an existing holding; the quote amount is
                // converted to base quantity at the execution price.
                let quantity = margin / exec_price;
                store.execute_spot_sell(mode, symbol, order_type, exec_price, quantity)
            } else {
                store.execute_order_typed(
                    mode,
                    market_type,
                    symbol,
                    side,
                    order_type,
                    exec_price,
                    margin,
                    leverage,
                )
            };
            result
                .map(|order| TradeActionResult::from_order("place_order", &order))
                .map_err(|error| anyhow!("{error}"))
        }
        TradeAction::CancelOrder { order_id } => store
            .cancel_order(order_id.trim())
            .map(|order| TradeActionResult::from_order("cancel_order", &order))
            .map_err(|error| anyhow!("{error}")),
        TradeAction::ClosePosition { pos_id, current_price } => {
            let pos_id = pos_id.trim();
            let pnl = store
                .positions
                .iter()
                .find(|position| position.id == pos_id)
                .map(|position| {
                    let mark = current_price
                        .filter(|price| price.is_finite() && *price > 0.0)
                        .or_else(|| store.get_mark_price(&position.symbol))
                        .unwrap_or(position.entry_price);
                    match position.side {
                        TradeSide::Long => (mark - position.entry_price) * position.quantity,
                        TradeSide::Short => (position.entry_price - mark) * position.quantity,
                    }
                });
            store
                .close_position(pos_id, current_price.filter(|price| price.is_finite() && *price > 0.0))
                .map(|order| {
                    let mut result = TradeActionResult::from_order("close_position", &order);
                    result.pnl = pnl;
                    result
                })
                .map_err(|error| anyhow!("{error}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Inline charts
// ---------------------------------------------------------------------------

/// A chart the assistant asked to render inline in the conversation.
///
/// The model emits a fenced `chart` block containing this JSON. The UI parses
/// the block, fetches real OHLCV data, and renders a compact candlestick chart
/// at that point in the reply.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChartRequest {
    #[serde(default)]
    pub exchange: Option<String>,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub timeframe: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Optional visual style: candles, hollow, ohlc, area, or heikin_ashi.
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_usize")]
    pub candles: Option<usize>,
    /// Optional start of an explicit historical window (ISO date/time or Unix ms).
    #[serde(
        default,
        alias = "since",
        alias = "from",
        deserialize_with = "deserialize_optional_string"
    )]
    pub start: Option<String>,
    /// Optional end of an explicit historical window (ISO date/time or Unix ms).
    #[serde(
        default,
        alias = "until",
        alias = "to",
        deserialize_with = "deserialize_optional_string"
    )]
    pub end: Option<String>,
    /// Parsed start in Unix milliseconds.
    #[serde(skip)]
    pub start_ms: Option<i64>,
    /// Parsed end in Unix milliseconds.
    #[serde(skip)]
    pub end_ms: Option<i64>,
    /// Validation error for an unparseable or oversized range.
    #[serde(skip)]
    pub range_error: Option<String>,
}

fn deserialize_optional_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    Ok(match raw {
        Some(Value::Number(number)) => number.as_u64().map(|value| value as usize),
        Some(Value::String(text)) => text.trim().parse::<usize>().ok(),
        _ => None,
    })
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<Value>::deserialize(deserializer)?;
    Ok(match raw {
        Some(Value::String(text)) => Some(text),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    })
}

impl ChartRequest {
    /// Normalises user/model-supplied values into gateway-safe identifiers.
    pub fn normalized(mut self) -> Self {
        self.exchange = Some(normalize_exchange(self.exchange.as_deref().unwrap_or("")));
        self.symbol = normalize_symbol(&self.symbol);
        self.timeframe = Some(normalize_timeframe(self.timeframe.as_deref().unwrap_or("")));
        self.style = Some(normalize_chart_style(self.style.as_deref().unwrap_or("")));
        self.candles = Some(self.candle_limit() as usize);
        self.title = self
            .title
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty());

        self.start_ms = None;
        self.end_ms = None;
        self.range_error = None;

        if let Some(raw) = self.start.as_deref().map(str::trim).filter(|raw| !raw.is_empty()) {
            match parse_date_ms(raw, false) {
                Some(ms) => self.start_ms = Some(ms),
                None => self.range_error = Some(format!("Invalid chart start date: {raw}")),
            }
        }
        if let Some(raw) = self.end.as_deref().map(str::trim).filter(|raw| !raw.is_empty()) {
            match parse_date_ms(raw, true) {
                Some(ms) => self.end_ms = Some(ms),
                None => self.range_error = Some(format!("Invalid chart end date: {raw}")),
            }
        }
        if self.range_error.is_none() {
            if let (Some(start), Some(end)) = (self.start_ms, self.end_ms) {
                if start >= end {
                    self.range_error =
                        Some("The chart start date must be before the end date.".to_string());
                }
            }
        }
        self
    }

    pub fn exchange(&self) -> &str {
        self.exchange.as_deref().unwrap_or("binance")
    }

    pub fn timeframe(&self) -> &str {
        self.timeframe.as_deref().unwrap_or("1h")
    }

    pub fn style(&self) -> &str {
        self.style.as_deref().unwrap_or("candles")
    }

    pub fn candle_limit(&self) -> u32 {
        self.candles.unwrap_or(200).clamp(1, 500) as u32
    }

    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| format!("{} · {}", self.symbol, self.timeframe()))
    }

    /// True when the request targets an explicit historical window.
    pub fn is_range(&self) -> bool {
        self.start_ms.is_some() || self.end_ms.is_some()
    }

    /// Human-readable window label for the chart header.
    pub fn range_label(&self) -> Option<String> {
        let start = self.start_ms?;
        let end = self.end_ms?;
        let format = |ms: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
                .map(|datetime| datetime.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| ms.to_string())
        };
        Some(format!("{} → {}", format(start), format(end)))
    }

    /// Stable identity for the UI entity cache.
    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.exchange(),
            self.symbol,
            self.timeframe(),
            self.style(),
            self.candle_limit(),
            self.start.as_deref().unwrap_or(""),
            self.end.as_deref().unwrap_or("")
        )
    }
}

/// Parses an ISO date/time, a human date such as `Nov 21, 2020`, or Unix
/// milliseconds into Unix milliseconds. Date-only end values include the whole
/// day.
fn parse_date_ms(raw: &str, end_of_day: bool) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    if let Ok(number) = raw.parse::<i64>() {
        return Some(if number.abs() < 10_000_000_000 {
            number * 1000
        } else {
            number
        });
    }

    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(datetime.timestamp_millis());
    }

    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let time = if end_of_day {
            date.and_hms_milli_opt(23, 59, 59, 999)
        } else {
            date.and_hms_opt(0, 0, 0)
        }?;
        return Some(time.and_utc().timestamp_millis());
    }

    for format in ["%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"] {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, format) {
            let time = if end_of_day {
                date.and_hms_milli_opt(23, 59, 59, 999)
            } else {
                date.and_hms_opt(0, 0, 0)
            }?;
            return Some(time.and_utc().timestamp_millis());
        }
    }

    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(datetime) = chrono::NaiveDateTime::parse_from_str(raw, format) {
            return Some(datetime.and_utc().timestamp_millis());
        }
    }

    None
}

fn normalize_chart_style(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "line" | "area" => "area".to_string(),
        "ohlc" | "bar" | "bars" => "ohlc".to_string(),
        "hollow" | "stroke" | "up_stroke" => "hollow".to_string(),
        "heikin" | "heikin_ashi" | "heikin-ashi" => "heikin_ashi".to_string(),
        _ => "candles".to_string(),
    }
}

fn normalize_exchange(raw: &str) -> String {
    let exchange = raw
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '_'], "");
    match exchange.as_str() {
        "" => "binance".to_string(),
        "binancefutures" | "binanceusdm" => "binanceusdm".to_string(),
        other => other.to_string(),
    }
}

fn normalize_symbol(raw: &str) -> String {
    let symbol = raw.trim().to_ascii_uppercase().replace(['-', '_'], "/");
    if symbol.is_empty() || symbol.contains('/') {
        return symbol;
    }
    for quote in ["USDT", "USDC", "USD", "BTC", "ETH", "BNB", "EUR", "TRY"] {
        if let Some(base) = symbol.strip_suffix(quote) {
            if !base.is_empty() {
                return format!("{base}/{quote}");
            }
        }
    }
    symbol
}

fn normalize_timeframe(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "1h".to_string();
    }
    match raw.to_ascii_lowercase().as_str() {
        "60" | "60m" | "1hr" | "1hour" => return "1h".to_string(),
        "240" | "240m" | "4hr" | "4hour" => return "4h".to_string(),
        "1440" | "d" | "day" | "daily" => return "1d".to_string(),
        "w" | "week" | "weekly" => return "1w".to_string(),
        "month" | "monthly" | "1mo" => return "1M".to_string(),
        _ => {}
    }
    let split = raw
        .find(|character: char| character.is_alphabetic())
        .unwrap_or(raw.len());
    let (number, unit) = raw.split_at(split);
    let amount = number.parse::<u32>().unwrap_or(1);
    let unit = match unit.trim() {
        "s" | "S" => "s",
        "m" | "min" => "m",
        "h" | "H" | "hr" => "h",
        "d" | "D" => "d",
        "w" | "W" => "w",
        "M" | "mo" | "MO" => "M",
        _ => "h",
    };
    format!("{amount}{unit}")
}

/// A parsed assistant message: markdown prose interleaved with chart requests
/// and executed trade directives.
#[derive(Clone, Debug, PartialEq)]
pub enum MessageSegment {
    Markdown(String),
    Chart(ChartRequest),
    /// A trade directive and, once executed, its authoritative result.
    Trade(TradeAction, Option<TradeActionResult>),
}

/// The directive language of a fenced block, if it is one FinTer understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectiveFence {
    Chart,
    Trade,
}

fn directive_fence(line: &str) -> Option<DirectiveFence> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("```")?;
    let language = rest.trim().to_ascii_lowercase();
    let language = language.split_whitespace().next().unwrap_or("");
    match language {
        "chart" | "finter-chart" => Some(DirectiveFence::Chart),
        "trade" | "finter-trade" | "agent-action" | "finter-agent-action" => {
            Some(DirectiveFence::Trade)
        }
        _ => None,
    }
}

fn parse_chart_request(body: &str) -> Option<ChartRequest> {
    let request: ChartRequest = serde_json::from_str(body.trim()).ok()?;
    if request.symbol.trim().is_empty() {
        return None;
    }
    Some(request.normalized())
}

/// Parses a `trade` / `agent-action` block body into an action, without
/// executing it. An unparseable block yields `None` so the caller can preserve
/// the raw text as markdown.
fn parse_trade_action(body: &str) -> Option<TradeAction> {
    serde_json::from_str::<TradeAction>(body.trim()).ok()
}

impl TradeAction {
    /// Wire name of the action, used for receipts and error reporting.
    pub fn action_name(&self) -> String {
        match self {
            TradeAction::PlaceOrder { .. } => "place_order",
            TradeAction::CancelOrder { .. } => "cancel_order",
            TradeAction::ClosePosition { .. } => "close_position",
        }
        .to_string()
    }
}

/// Splits an assistant reply into markdown, inline chart, and trade segments.
///
/// This is a **pure** parse: trade directives are returned as
/// `MessageSegment::Trade(action, None)` and are *never* executed here. Callers
/// must run [`execute_trade_directives_in_text`] exactly once when a reply
/// arrives and attach the returned receipts to the message, so that later
/// renders or reloads only display historical results.
///
/// Invalid or unclosed fences are preserved as markdown so no model output is
/// silently discarded.
pub fn parse_message_segments(text: &str) -> Vec<MessageSegment> {
    parse_segments(text, &[])
}

/// Splits an assistant reply into segments, attaching previously executed trade
/// receipts to their matching `trade` directives in order.
///
/// Receipts are matched positionally (the n-th receipt belongs to the n-th
/// trade block). Missing receipts leave the segment as `Trade(action, None)`.
/// This function never executes anything.
pub fn parse_message_segments_with_results(
    text: &str,
    receipts: &[TradeActionResult],
) -> Vec<MessageSegment> {
    parse_segments(text, receipts)
}

fn parse_segments(text: &str, receipts: &[TradeActionResult]) -> Vec<MessageSegment> {
    let mut segments = Vec::new();
    let mut markdown = String::new();
    let mut lines = text.split_inclusive('\n').peekable();
    let mut receipt_index = 0usize;

    while let Some(line) = lines.next() {
        let Some(kind) = directive_fence(line) else {
            markdown.push_str(line);
            continue;
        };

        let mut body = String::new();
        let mut closing = None;
        while let Some(inner) = lines.next() {
            if inner.trim_start().starts_with("```") {
                closing = Some(inner);
                break;
            }
            body.push_str(inner);
        }

        match closing {
            Some(close) => {
                let segment = match kind {
                    DirectiveFence::Chart => {
                        parse_chart_request(&body).map(MessageSegment::Chart)
                    }
                    DirectiveFence::Trade => parse_trade_action(&body).map(|action| {
                        let result = receipts.get(receipt_index).cloned();
                        receipt_index += 1;
                        MessageSegment::Trade(action, result)
                    }),
                };
                if let Some(segment) = segment {
                    if !markdown.is_empty() {
                        segments.push(MessageSegment::Markdown(std::mem::take(&mut markdown)));
                    }
                    // Trade receipts are rendered as cards, so keep the blank line
                    // that followed the closing fence to separate them from the prose
                    // below. Charts render inline and keep their existing spacing.
                    let keep_trailing_newline = kind == DirectiveFence::Trade;
                    segments.push(segment);
                    if keep_trailing_newline && close.ends_with('\n') {
                        markdown.push('\n');
                    }
                } else {
                    markdown.push_str(line);
                    markdown.push_str(&body);
                    markdown.push_str(close);
                    if !close.ends_with('\n') {
                        markdown.push('\n');
                    }
                }
            }
            None => {
                markdown.push_str(line);
                markdown.push_str(&body);
            }
        }
    }

    if !markdown.is_empty() {
        segments.push(MessageSegment::Markdown(markdown));
    }
    segments
}

/// Extracts the trade directives from `text` without executing them.
///
/// Unparseable blocks are skipped (they stay visible as markdown).
pub fn parse_trade_directives_in_text(text: &str) -> Vec<TradeAction> {
    let mut actions = Vec::new();
    for (kind, body) in fenced_blocks(text) {
        if kind != DirectiveFence::Trade {
            continue;
        }
        if let Some(action) = parse_trade_action(&body) {
            actions.push(action);
        }
    }
    actions
}

/// Executes every `trade` directive in `text` exactly once, in order.
///
/// This is the *only* entry point that executes directives from a reply. Call it
/// once when an assistant response arrives, persist the receipts, and render
/// them later via [`parse_message_segments_with_results`]. Calling it again on
/// the same text would place duplicate orders, so never call it from a render
/// path or when loading saved conversations.
pub fn execute_trade_directives_in_text(text: &str) -> Vec<(TradeAction, TradeActionResult)> {
    parse_trade_directives_in_text(text)
        .into_iter()
        .map(|action| {
            let result = execute_trade_directive(&action).unwrap_or_else(|error| {
                TradeActionResult::failure(&action.action_name(), error)
            });
            (action, result)
        })
        .collect()
}

/// Executes the trade directives in a completed assistant reply once and
/// returns just the receipts, ready to store on the message record.
pub fn execute_trades_in_reply(text: &str) -> Vec<TradeActionResult> {
    execute_trade_directives_in_text(text)
        .into_iter()
        .map(|(_, result)| result)
        .collect()
}

/// Iterates over the complete fenced directive blocks in `text`, yielding the
/// fence kind and its body. Unclosed fences are ignored.
fn fenced_blocks(text: &str) -> Vec<(DirectiveFence, String)> {
    let mut blocks = Vec::new();
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let Some(kind) = directive_fence(line) else {
            continue;
        };
        let mut body = String::new();
        let mut closed = false;
        for inner in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push_str(inner);
        }
        if closed {
            blocks.push((kind, body));
        }
    }
    blocks
}

/// True when `line` is a partially streamed directive fence opener, e.g. "``",
/// "```", "```tra", or "```agent-a". Such a line is withheld while typing so
/// the marker never flickers into the visible reply.
fn is_partial_directive_opener(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix("``") else {
        return false;
    };
    let language = rest.trim_start_matches('`').trim().to_ascii_lowercase();
    if language.is_empty() {
        return true;
    }
    [
        "chart",
        "finter-chart",
        "trade",
        "finter-trade",
        "agent-action",
        "finter-agent-action",
    ]
    .iter()
    .any(|known| known.starts_with(&language))
}

/// Removes chart and trade fences while a reply is still being typed out.
///
/// An unclosed fence is dropped through to the end of the streamed text so the
/// user never sees half-written JSON. A fence whose opening line has not fully
/// arrived yet (for example a trailing "``" or "```tra") is dropped as well, so
/// directive markers never flicker into the typing view.
pub fn strip_chart_directives(text: &str) -> String {
    let mut output = String::new();
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if directive_fence(line).is_some() {
            while let Some(inner) = lines.next() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
            }
            continue;
        }
        // A trailing, still-streaming fence opener such as "``", "```tra" or
        // "```agent-a" is held back until the language is known.
        if !line.ends_with('\n') && is_partial_directive_opener(line) {
            continue;
        }
        output.push_str(line);
    }
    output
}

/// Maximum number of candles a single inline chart may request across all
/// paginated gateway calls.
const MAX_RANGE_CANDLES: i64 = 5_000;
/// Per-request page size used while walking a historical window backwards.
const RANGE_REQUEST_LIMIT: u32 = 500;

/// Fetches the OHLCV data for an inline chart request.
///
/// Without `start`/`end`, this returns the most recent `candles` snapshot.
/// With a historical window, it paginates backwards from `end` using the
/// gateway's `since`/`until` parameters and returns exactly the candles inside
/// the requested range.
pub async fn fetch_chart_candles(request: &ChartRequest) -> Result<Vec<OhlcvCandle>> {
    if let Some(error) = &request.range_error {
        return Err(anyhow!("{error}"));
    }

    let mut config = crate::data::bridge::BridgeConfig::default_dev(
        request.exchange(),
        &request.symbol,
    );
    config.timeframe = request.timeframe().to_string();

    let interval_ms = crate::data::bridge::BridgeConfig::timeframe_to_ms(&config.timeframe).max(1);
    let now = chrono::Utc::now().timestamp_millis();

    let (start, end) = match (request.start_ms, request.end_ms) {
        (Some(start), Some(end)) => (start, end.min(now)),
        (Some(start), None) => (start, now),
        (None, Some(end)) => {
            // "End only" means the requested number of candles ending there.
            let end = end.min(now);
            config.ohlcv_limit = request.candle_limit();
            let mut candles =
                crate::data::bridge::fetch_ohlcv_history(&config, Some(end)).await?;
            normalize_candles(&mut candles);
            candles.retain(|candle| candle.timestamp <= end);
            truncate_candles(&mut candles, request.candle_limit() as usize);
            return Ok(candles);
        }
        (None, None) => {
            config.ohlcv_limit = request.candle_limit();
            let mut candles = crate::data::bridge::fetch_ohlcv_history(&config, None).await?;
            normalize_candles(&mut candles);
            truncate_candles(&mut candles, request.candle_limit() as usize);
            return Ok(candles);
        }
    };

    if start >= end {
        return Err(anyhow!(
            "The chart start date must be before the end date."
        ));
    }

    let estimated = ((end - start) / interval_ms).max(0) + 1;
    if estimated > MAX_RANGE_CANDLES {
        return Err(anyhow!(
            "Requested range covers about {estimated} candles; the limit is {MAX_RANGE_CANDLES}. Use a larger timeframe or a narrower window."
        ));
    }

    let mut candles = Vec::new();
    let mut cursor = end;
    let max_requests = (MAX_RANGE_CANDLES / RANGE_REQUEST_LIMIT as i64) + 2;
    let mut requests = 0;

    while cursor >= start && requests < max_requests {
        // Fetch only the candles still needed for this window, capped by the
        // gateway's per-request page size.
        let remaining = ((cursor - start) / interval_ms).max(0) + 1;
        config.ohlcv_limit = remaining.min(RANGE_REQUEST_LIMIT as i64).max(1) as u32;

        let batch = crate::data::bridge::fetch_ohlcv_history(&config, Some(cursor)).await?;
        if batch.is_empty() {
            break;
        }

        let earliest = batch
            .iter()
            .map(|candle| candle.timestamp)
            .min()
            .unwrap_or(cursor);
        candles.extend(batch);

        if earliest <= start {
            break;
        }

        let next_cursor = earliest.saturating_sub(1);
        if next_cursor >= cursor {
            break;
        }
        cursor = next_cursor;
        requests += 1;
    }

    candles.retain(|candle| candle.timestamp >= start && candle.timestamp <= end);
    normalize_candles(&mut candles);
    Ok(candles)
}

fn normalize_candles(candles: &mut Vec<OhlcvCandle>) {
    candles.retain(|candle| {
        candle.timestamp > 0
            && candle.open.is_finite()
            && candle.high.is_finite()
            && candle.low.is_finite()
            && candle.close.is_finite()
    });
    candles.sort_by_key(|candle| candle.timestamp);
    candles.dedup_by_key(|candle| candle.timestamp);
}

fn truncate_candles(candles: &mut Vec<OhlcvCandle>, limit: usize) {
    if candles.len() > limit {
        let keep_from = candles.len() - limit;
        *candles = candles.split_off(keep_from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limit_is_bounded() {
        assert!(MAX_CONTEXT_BYTES <= 200_000);
    }

    #[test]
    fn clean_base_url_logic() {
        for (raw, expected) in [
            ("https://api.openai.com/v1", "https://api.openai.com/v1/models"),
            ("https://api.openai.com/v1/", "https://api.openai.com/v1/models"),
            ("https://openrouter.ai/api/v1/chat/completions", "https://openrouter.ai/api/v1/models"),
            ("http://localhost:11434/v1", "http://localhost:11434/v1/models"),
        ] {
            let clean = raw
                .trim_end_matches('/')
                .trim_end_matches("/chat/completions")
                .trim_end_matches("/models");
            assert_eq!(format!("{}/models", clean), expected);
        }
    }

    #[test]
    fn parses_inline_chart_between_markdown() {
        let reply = "Momentum is improving.\n\n```chart\n{\"exchange\":\"binance\",\"symbol\":\"BTCUSDT\",\"timeframe\":\"4h\",\"candles\":150}\n```\n\nWatch the breakout.";
        let segments = parse_message_segments(reply);

        assert_eq!(segments.len(), 3);
        assert_eq!(
            segments[0],
            MessageSegment::Markdown("Momentum is improving.\n\n".to_string())
        );
        match &segments[1] {
            MessageSegment::Chart(request) => {
                assert_eq!(request.exchange(), "binance");
                assert_eq!(request.symbol, "BTC/USDT");
                assert_eq!(request.timeframe(), "4h");
                assert_eq!(request.candle_limit(), 150);
            }
            other => panic!("expected chart, got {other:?}"),
        }
        assert_eq!(
            segments[2],
            MessageSegment::Markdown("\nWatch the breakout.".to_string())
        );
    }

    #[test]
    fn invalid_chart_block_is_preserved_as_markdown() {
        let reply = "```chart\nnot json\n```";
        let segments = parse_message_segments(reply);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            MessageSegment::Markdown(text) => assert!(text.contains("not json")),
            other => panic!("expected markdown, got {other:?}"),
        }
    }

    #[test]
    fn chart_defaults_are_bounded() {
        let request: ChartRequest =
            serde_json::from_str("{\"symbol\":\"ethusdt\"}").expect("valid request");
        let request = request.normalized();
        assert_eq!(request.exchange(), "binance");
        assert_eq!(request.symbol, "ETH/USDT");
        assert_eq!(request.timeframe(), "1h");
        assert_eq!(request.candle_limit(), 200);

        let request: ChartRequest =
            serde_json::from_str("{\"symbol\":\"BTC/USDT\",\"candles\":9999}").expect("valid request");
        assert_eq!(request.normalized().candle_limit(), 500);

        let request: ChartRequest =
            serde_json::from_str("{\"symbol\":\"BTC/USDT\",\"candles\":5}").expect("valid request");
        assert_eq!(request.normalized().candle_limit(), 5);
    }

    #[test]
    fn chart_style_is_normalized() {
        let request: ChartRequest =
            serde_json::from_str("{\"symbol\":\"BTC/USDT\",\"style\":\"line\"}")
                .expect("valid request");
        assert_eq!(request.normalized().style(), "area");

        let request: ChartRequest =
            serde_json::from_str("{\"symbol\":\"BTC/USDT\"}").expect("valid request");
        assert_eq!(request.normalized().style(), "candles");
    }

    #[test]
    fn parses_historical_range() {
        let reply = "```chart\n{\"symbol\":\"BTC/USDT\",\"timeframe\":\"1d\",\"start\":\"2020-11-21\",\"end\":\"2020-12-21\"}\n```";
        let segments = parse_message_segments(reply);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            MessageSegment::Chart(request) => {
                assert!(request.is_range());
                assert!(request.start_ms.is_some());
                assert!(request.end_ms.is_some());
                assert!(request.start_ms.unwrap() < request.end_ms.unwrap());
                let label = request.range_label().expect("range label");
                assert!(label.contains("2020-11-21"), "label={label}");
                assert!(label.contains("2020-12-21"), "label={label}");
            }
            other => panic!("expected chart, got {other:?}"),
        }
    }

    #[test]
    fn invalid_range_is_reported() {
        let request: ChartRequest =
            serde_json::from_str("{\"symbol\":\"BTC/USDT\",\"start\":\"not-a-date\"}")
                .expect("valid request");
        let request = request.normalized();
        assert!(request.range_error.is_some());
        assert!(!request.is_range());
    }

    #[test]
    fn date_only_end_includes_whole_day() {
        let request = serde_json::from_str::<ChartRequest>(
            "{\"symbol\":\"BTC/USDT\",\"start\":\"2020-11-21\",\"end\":\"2020-12-21\"}",
        )
        .expect("valid request")
        .normalized();
        let end = request.end_ms.expect("parsed end");
        let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(end).unwrap();
        assert_eq!(datetime.format("%H:%M:%S").to_string(), "23:59:59");
    }

    #[test]
    fn parses_human_date_strings() {
        let request = serde_json::from_str::<ChartRequest>(
            "{\"symbol\":\"BTC/USDT\",\"start\":\"Nov 21, 2020\",\"end\":\"December 21, 2020\"}",
        )
        .expect("valid request")
        .normalized();
        assert!(request.start_ms.is_some());
        assert!(request.end_ms.is_some());
        assert!(request.range_error.is_none());
    }

    #[test]
    fn timeframe_normalization_preserves_minutes_and_months() {
        assert_eq!(normalize_timeframe("1m"), "1m");
        assert_eq!(normalize_timeframe("15m"), "15m");
        assert_eq!(normalize_timeframe("1M"), "1M");
        assert_eq!(normalize_timeframe("4h"), "4h");
        assert_eq!(normalize_timeframe("60"), "1h");
    }

    #[test]
    fn strips_chart_directives_while_typing() {
        let complete = "Before\n```chart\n{\"symbol\":\"BTC/USDT\"}\n```\nAfter";
        assert_eq!(strip_chart_directives(complete), "Before\nAfter");

        let streaming = "Before\n```chart\n{\"symbol\":\"BTC/USDT\"";
        assert_eq!(strip_chart_directives(streaming), "Before\n");

        let normal_code = "Before\n```rust\nfn main() {}\n```\nAfter";
        assert_eq!(strip_chart_directives(normal_code), normal_code);
    }

    #[test]
    fn strips_trade_directives_while_typing() {
        let complete = "Buying now.\n```trade\n{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\"}\n```\nDone.";
        assert_eq!(strip_chart_directives(complete), "Buying now.\nDone.");

        let agent_action = "```agent-action\n{\"action\":\"cancel_order\",\"order_id\":\"abc\"}\n```";
        assert_eq!(strip_chart_directives(agent_action), "");

        let streaming = "Buying\n```trade\n{\"action\":\"place_";
        assert_eq!(strip_chart_directives(streaming), "Buying\n");
    }

    #[test]
    fn partial_fence_openers_do_not_flicker() {
        for partial in ["Before\n``", "Before\n```", "Before\n```tra", "Before\n```agent-a"] {
            assert_eq!(strip_chart_directives(partial), "Before\n", "input={partial}");
        }
        // A complete opener without a body yet hides the whole (empty) block.
        assert_eq!(strip_chart_directives("Before\n```trade"), "Before\n");
        // Non-directive fences are untouched.
        assert_eq!(strip_chart_directives("Before\n```rust"), "Before\n```rust");
    }

    #[test]
    fn parses_trade_action_variants() {
        let action: TradeAction = serde_json::from_str(
            "{\"action\":\"place_order\",\"symbol\":\"btcusdt\",\"side\":\"long\",\"order_type\":\"limit\",\"price\":\"64000\",\"margin\":1000,\"leverage\":5,\"market_type\":\"futures\",\"mode\":\"paper\"}",
        )
        .expect("valid place_order");
        match &action {
            TradeAction::PlaceOrder { symbol, side, order_type, price, margin, leverage, market_type, mode } => {
                assert_eq!(symbol, "btcusdt");
                assert_eq!(side, "long");
                assert_eq!(order_type.as_deref(), Some("limit"));
                assert_eq!(*price, Some(64_000.0));
                assert_eq!(*margin, Some(1_000.0));
                assert_eq!(*leverage, Some(5.0));
                assert_eq!(market_type.as_deref(), Some("futures"));
                assert_eq!(mode.as_deref(), Some("paper"));
            }
            other => panic!("expected place_order, got {other:?}"),
        }
        assert_eq!(action.kind_label(), "Place order");
        assert!(!action.is_live());

        let cancel: TradeAction =
            serde_json::from_str("{\"action\":\"cancel_order\",\"order_id\":\"abc123\"}").expect("valid cancel");
        assert_eq!(cancel.action_name(), "cancel_order");

        let close: TradeAction = serde_json::from_str(
            "{\"action\":\"close_position\",\"pos_id\":\"abc123\",\"current_price\":64250.5}",
        )
        .expect("valid close");
        assert_eq!(close.action_name(), "close_position");
    }

    #[test]
    fn trade_action_validation_rejects_bad_input() {
        let missing_symbol: TradeAction = serde_json::from_str(
            "{\"action\":\"place_order\",\"side\":\"buy\",\"margin\":100}",
        )
        .expect("parses");
        assert!(missing_symbol.validate().is_err());

        let bad_side: TradeAction = serde_json::from_str(
            "{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"sideways\",\"margin\":100}",
        )
        .expect("parses");
        assert!(bad_side.validate().is_err());

        let limit_without_price: TradeAction = serde_json::from_str(
            "{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\",\"order_type\":\"limit\",\"margin\":100}",
        )
        .expect("parses");
        assert!(limit_without_price.validate().is_err());

        let no_margin: TradeAction = serde_json::from_str(
            "{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\"}",
        )
        .expect("parses");
        assert!(no_margin.validate().is_err());
    }

    #[test]
    fn parses_trade_block_without_executing() {
        // A rejected action still yields a Trade segment, but parsing is pure:
        // no result is attached until the caller executes the reply once.
        let reply = "Placing.\n```trade\n{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\",\"margin\":-5}\n```\nDone.";
        let segments = parse_message_segments(reply);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], MessageSegment::Markdown("Placing.\n".to_string()));
        match &segments[1] {
            MessageSegment::Trade(action, result) => {
                assert_eq!(action.action_name(), "place_order");
                assert!(result.is_none(), "parsing must not execute trades");
            }
            other => panic!("expected trade, got {other:?}"),
        }
        assert_eq!(segments[2], MessageSegment::Markdown("\nDone.".to_string()));
    }

    #[test]
    fn agent_action_fence_is_a_trade_directive() {
        let reply = "```agent-action\n{\"action\":\"cancel_order\"}\n```";
        let segments = parse_message_segments(reply);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            MessageSegment::Trade(action, result) => {
                assert_eq!(action.action_name(), "cancel_order");
                assert!(result.is_none());
            }
            other => panic!("expected trade, got {other:?}"),
        }
    }

    #[test]
    fn receipts_are_attached_to_matching_trade_segments() {
        let reply = "A\n```trade\n{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\",\"margin\":-5}\n```\nB\n```trade\n{\"action\":\"cancel_order\"}\n```";
        let receipt = TradeActionResult::failure("place_order", "rejected");
        let segments = parse_message_segments_with_results(reply, &[receipt.clone()]);
        let trades: Vec<_> = segments
            .iter()
            .filter_map(|segment| match segment {
                MessageSegment::Trade(action, result) => Some((action.action_name(), result.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(trades.len(), 2);
        assert_eq!(trades[0].1, Some(receipt));
        // Missing receipts stay unexecuted rather than inventing a result.
        assert_eq!(trades[1].1, None);
    }

    #[test]
    fn parsing_never_executes_trades() {
        // Count the orders in the global store before and after repeated parses.
        let reply = "```trade\n{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\",\"order_type\":\"limit\",\"price\":100,\"margin\":10}\n```";
        let before = TradeStore::global().lock().unwrap().orders.len();
        for _ in 0..5 {
            let _ = parse_message_segments(reply);
            let _ = parse_message_segments_with_results(reply, &[]);
        }
        let after = TradeStore::global().lock().unwrap().orders.len();
        assert_eq!(before, after, "rendering/parsing must not place orders");
    }

    #[test]
    fn executes_directives_once_per_explicit_call() {
        let reply = "```trade\n{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\",\"order_type\":\"limit\",\"price\":100,\"margin\":10}\n```\n```trade\n{\"action\":\"place_order\",\"symbol\":\"BTC/USDT\",\"side\":\"buy\",\"margin\":-1}\n```";
        let directives = parse_trade_directives_in_text(reply);
        assert_eq!(directives.len(), 2);

        let before = TradeStore::global().lock().unwrap().orders.len();
        let results = execute_trade_directives_in_text(reply);
        let after = TradeStore::global().lock().unwrap().orders.len();
        assert_eq!(results.len(), 2);
        assert!(results[0].1.ok);
        assert!(!results[1].1.ok);
        // Exactly one order was placed for the two directives.
        assert_eq!(after, before + 1);

        // Re-parsing the same text with the stored receipts does not execute again.
        let receipts: Vec<TradeActionResult> =
            results.into_iter().map(|(_, result)| result).collect();
        let segments = parse_message_segments_with_results(reply, &receipts);
        assert_eq!(
            segments
                .iter()
                .filter(|segment| matches!(segment, MessageSegment::Trade(..)))
                .count(),
            2
        );
        let after_render = TradeStore::global().lock().unwrap().orders.len();
        assert_eq!(after_render, after, "re-rendering must not place orders");
    }

    #[test]
    fn message_record_persists_trade_receipts() {
        let record = ChatMessageRecord {
            prompt: "buy".into(),
            response: "ok".into(),
            failed: false,
            timestamp: None,
            trade_receipts: vec![TradeActionResult::failure("place_order", "rejected")],
        };
        let json = serde_json::to_string(&record).expect("serialize");
        let decoded: ChatMessageRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.trade_receipts.len(), 1);
        assert!(!decoded.trade_receipts[0].ok);

        // Older records without the field still load with no receipts.
        let legacy: ChatMessageRecord =
            serde_json::from_str("{\"prompt\":\"p\",\"response\":\"r\"}").expect("legacy");
        assert!(legacy.trade_receipts.is_empty());
    }

    #[test]
    fn invalid_trade_block_is_preserved_as_markdown() {
        let reply = "```trade\nnot json\n```";
        let segments = parse_message_segments(reply);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            MessageSegment::Markdown(text) => assert!(text.contains("not json")),
            other => panic!("expected markdown, got {other:?}"),
        }
    }
}
