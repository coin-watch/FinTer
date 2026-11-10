//! OpenAI-compatible, read-only dashboard analysis client with automatic model discovery.
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::data::models::OhlcvCandle;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChatMessageRecord {
    pub prompt: String,
    pub response: String,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub timestamp: Option<String>,
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

const SYSTEM_PROMPT: &str = r#"You are FinTer's read-only market analysis agent. Analyse only the supplied dashboard snapshot, distinguish facts from inference, explicitly identify missing/stale data, and never claim to place or execute trades. This is informational, not financial advice.

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

/// A parsed assistant message: markdown prose interleaved with chart requests.
#[derive(Clone, Debug, PartialEq)]
pub enum MessageSegment {
    Markdown(String),
    Chart(ChartRequest),
}

fn is_chart_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return false;
    };
    let language = rest.trim().to_ascii_lowercase();
    language == "chart"
        || language == "finter-chart"
        || language.starts_with("chart ")
        || language.starts_with("finter-chart ")
}

fn parse_chart_request(body: &str) -> Option<ChartRequest> {
    let request: ChartRequest = serde_json::from_str(body.trim()).ok()?;
    if request.symbol.trim().is_empty() {
        return None;
    }
    Some(request.normalized())
}

/// Splits an assistant reply into markdown and inline chart segments.
///
/// Invalid or unclosed chart fences are preserved as markdown so no model output
/// is silently discarded.
pub fn parse_message_segments(text: &str) -> Vec<MessageSegment> {
    let mut segments = Vec::new();
    let mut markdown = String::new();
    let mut lines = text.split_inclusive('\n').peekable();

    while let Some(line) = lines.next() {
        if !is_chart_fence(line) {
            markdown.push_str(line);
            continue;
        }

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
                if let Some(request) = parse_chart_request(&body) {
                    if !markdown.is_empty() {
                        segments.push(MessageSegment::Markdown(std::mem::take(&mut markdown)));
                    }
                    segments.push(MessageSegment::Chart(request));
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

/// Removes chart fences while a reply is still being typed out.
///
/// An unclosed fence is dropped through to the end of the streamed text so the
/// user never sees half-written JSON.
pub fn strip_chart_directives(text: &str) -> String {
    let mut output = String::new();
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if !is_chart_fence(line) {
            output.push_str(line);
            continue;
        }
        while let Some(inner) = lines.next() {
            if inner.trim_start().starts_with("```") {
                break;
            }
        }
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
}
