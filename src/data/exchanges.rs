use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::{CoinWatchError, Result};
use crate::data::models::{ExchangeInfo, MarketInfo, OhlcvCandle, SymbolInfo};

/// Parse a raw API response body for exchanges, flexibly supporting plain string arrays,
/// objects with `id`/`value`/`label`, and wrapped envelopes (`{ "exchanges": [...] }` or `{ "data": [...] }`).
pub fn parse_exchanges_body(body: &str) -> Result<Vec<ExchangeInfo>> {
    let val: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| CoinWatchError::Deserialization(e.to_string()))?;

    // Check if it's an error envelope
    if let Some(false) = val.get("success").and_then(|v| v.as_bool()) {
        let msg = val.get("error").or_else(|| val.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("API error");
        return Err(CoinWatchError::Api {
            status: 400,
            message: msg.to_string(),
        });
    }

    let target = val.get("data").unwrap_or(&val);
    let raw_list = if let Some(arr) = target.as_array() {
        arr.clone()
    } else if let Some(arr) = target.get("exchanges").and_then(|v| v.as_array()) {
        arr.clone()
    } else if let Some(arr) = val.get("exchanges").and_then(|v| v.as_array()) {
        arr.clone()
    } else {
        return Err(CoinWatchError::Api {
            status: 200,
            message: "Unexpected exchanges response format".into(),
        });
    };

    let mut result = Vec::new();
    for item in raw_list {
        if let Some(s) = item.as_str() {
            result.push(ExchangeInfo::from_id(s));
        } else if let Ok(info) = serde_json::from_value::<ExchangeInfo>(item.clone()) {
            result.push(info.normalized());
        } else if let Some(obj) = item.as_object() {
            let id = obj.get("id")
                .or_else(|| obj.get("value"))
                .or_else(|| obj.get("exchange"))
                .or_else(|| obj.get("exchange_id"))
                .or_else(|| obj.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let label = obj.get("label")
                .or_else(|| obj.get("name"))
                .or_else(|| obj.get("exchange"))
                .and_then(|v| v.as_str())
                .unwrap_or(id);
            let value = obj.get("value")
                .or_else(|| obj.get("exchange_id"))
                .or_else(|| obj.get("exchange"))
                .or_else(|| obj.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(id);
            result.push(ExchangeInfo::new(id, value, label));
        }
    }
    Ok(result)
}

/// Parse a raw API response body for symbols, flexibly supporting symbol objects,
/// plain string arrays, and wrapped envelopes (`{ "symbols": [...] }` or `{ "data": [...] }`).
pub fn parse_symbols_body(body: &str) -> Result<Vec<SymbolInfo>> {
    let val: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| CoinWatchError::Deserialization(e.to_string()))?;

    // Check if it's an error envelope
    if let Some(false) = val.get("success").and_then(|v| v.as_bool()) {
        let msg = val.get("error").or_else(|| val.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("API error");
        return Err(CoinWatchError::Api {
            status: 400,
            message: msg.to_string(),
        });
    }

    let target = val.get("data").unwrap_or(&val);
    let raw_list = if let Some(arr) = target.as_array() {
        arr.clone()
    } else if let Some(arr) = target.get("symbols").and_then(|v| v.as_array()) {
        arr.clone()
    } else if let Some(arr) = val.get("symbols").and_then(|v| v.as_array()) {
        arr.clone()
    } else {
        return Err(CoinWatchError::Api {
            status: 200,
            message: "Unexpected symbols response format".into(),
        });
    };

    let mut result = Vec::new();
    for item in raw_list {
        if let Some(s) = item.as_str() {
            result.push(SymbolInfo::from_symbol(s));
        } else if let Ok(sym) = serde_json::from_value::<SymbolInfo>(item.clone()) {
            if sym.active {
                result.push(sym.normalized());
            }
        } else if let Some(obj) = item.as_object() {
            let symbol_str = obj.get("symbol")
                .or_else(|| obj.get("s"))
                .or_else(|| obj.get("pair"))
                .or_else(|| obj.get("name"))
                .and_then(|v| v.as_str());
            let active = obj.get("active").or_else(|| obj.get("is_active")).and_then(|v| v.as_bool()).unwrap_or(true);
            if active {
                if let Some(sym) = symbol_str {
                    let mut s_info = SymbolInfo::from_symbol(sym);
                    if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
                        s_info.symbol_type = t.to_string();
                    }
                    if let Some(base) = obj.get("base").or_else(|| obj.get("base_currency")).or_else(|| obj.get("baseAsset")).and_then(|v| v.as_str()) {
                        s_info.base = base.to_string();
                    }
                    if let Some(quote) = obj.get("quote").or_else(|| obj.get("quote_currency")).or_else(|| obj.get("quoteAsset")).and_then(|v| v.as_str()) {
                        s_info.quote = quote.to_string();
                    }
                    result.push(s_info.normalized());
                }
            }
        }
    }
    Ok(result)
}

/// Parameters for fetching OHLCV candle data.
pub struct OhlcvParams {
    pub exchange_id: String,
    pub symbol: String,
    pub timeframe: Option<String>,
    pub since: Option<i64>,
    pub limit: Option<u32>,
    pub until: Option<i64>,
}

impl ApiClient {
    /// List all supported exchanges asynchronously, applying login session Bearer auth when available.
    /// Uses CoinWatch endpoint `GET /api/v1/exchanges`.
    pub async fn list_exchanges_async(&self) -> Result<Vec<ExchangeInfo>> {
        let resp = self.auth_get("/api/v1/exchanges").send().await?;
        let status = resp.status();
        let text = resp.text().await.map_err(CoinWatchError::Network)?;
        if !status.is_success() {
            return Err(CoinWatchError::Api {
                status: status.as_u16(),
                message: format!("GET /api/v1/exchanges returned HTTP {status}: {text}"),
            });
        }
        parse_exchanges_body(&text)
    }

    /// List all supported exchanges.
    /// GET /api/v1/exchanges (carries Bearer token if logged in)
    ///
    /// Blocking — run from a background thread.
    pub fn list_exchanges(&self) -> Result<Vec<ExchangeInfo>> {
        let rt = tokio_runtime();
        rt.block_on(self.list_exchanges_async())
    }

    /// List market types for an exchange (e.g. spot, future, swap).
    /// Mirrors CoinWatch client fallback.
    pub fn list_markets(&self, _exchange_id: &str) -> Result<Vec<MarketInfo>> {
        Ok(vec![MarketInfo {
            value: "spot".to_string(),
            label: "Spot".to_string(),
            market_type: "spot".to_string(),
        }])
    }

    /// List trading symbols for an exchange asynchronously, applying login session Bearer auth when available.
    /// Uses CoinWatch endpoint `GET /api/v1/symbols?exchange=<id>`.
    pub async fn list_symbols_async(
        &self,
        exchange_id: &str,
        market_type: Option<&str>,
    ) -> Result<Vec<SymbolInfo>> {
        let lower_exchange = exchange_id.trim().to_lowercase();
        let mut query = vec![("exchange", lower_exchange.as_str())];
        if let Some(mt) = market_type {
            if !mt.is_empty() {
                query.push(("type", mt));
            }
        }

        let resp = self.auth_get("/api/v1/symbols").query(&query).send().await?;
        let status = resp.status();
        let text = resp.text().await.map_err(CoinWatchError::Network)?;
        if !status.is_success() {
            return Err(CoinWatchError::Api {
                status: status.as_u16(),
                message: format!("GET /api/v1/symbols?exchange={lower_exchange} returned HTTP {status}: {text}"),
            });
        }
        parse_symbols_body(&text)
    }

    /// List trading symbols for an exchange, optionally filtered by market type.
    /// GET /api/v1/symbols?exchange=<id> (carries Bearer token if logged in)
    ///
    /// Blocking — run from a background thread.
    pub fn list_symbols(
        &self,
        exchange_id: &str,
        market_type: Option<&str>,
    ) -> Result<Vec<SymbolInfo>> {
        let rt = tokio_runtime();
        rt.block_on(self.list_symbols_async(exchange_id, market_type))
    }

    /// List available timeframes for an exchange (e.g. "1m", "5m", "1h", "1d").
    /// Mirrors CoinWatch `defaultTimeframes`.
    pub fn list_timeframes(&self, _exchange_id: &str) -> Result<Vec<String>> {
        Ok(vec![
            "1s".into(), "1m".into(), "3m".into(), "5m".into(), "15m".into(),
            "30m".into(), "1h".into(), "2h".into(), "4h".into(), "6h".into(),
            "8h".into(), "12h".into(), "1d".into(), "1w".into(), "1M".into(),
        ])
    }

    /// Fetch OHLCV candle data for a symbol on an exchange.
    /// GET /api/v1/ohlcv?exchange=...&symbol=...&timeframe=...&since=...&limit=...&until=...
    ///
    /// Blocking — run from a background thread.
    pub fn fetch_ohlcv(&self, params: &OhlcvParams) -> Result<Vec<OhlcvCandle>> {
        let rt = tokio_runtime();
        let lower_exchange = params.exchange_id.trim().to_lowercase();
        let symbol = params.symbol.clone();
        let timeframe = params.timeframe.clone().unwrap_or_else(|| "1m".to_string());
        let since = params.since;
        let limit = params.limit.unwrap_or(100);
        let until = params.until;

        rt.block_on(async {
            let mut req = self.auth_get("/api/v1/ohlcv")
                .query(&[
                    ("exchange", lower_exchange.as_str()),
                    ("symbol", symbol.as_str()),
                    ("timeframe", timeframe.as_str()),
                    ("limit", &limit.to_string()),
                ]);

            if let Some(since) = since {
                req = req.query(&[("since", &since.to_string())]);
            }
            if let Some(until) = until {
                req = req.query(&[("until", &until.to_string())]);
            }

            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Fetch the latest OHLCV candles for a symbol.
    /// GET /api/v1/ohlcv?exchange=...&symbol=...&timeframe=...&limit=...
    ///
    /// Blocking — run from a background thread.
    pub fn fetch_latest_ohlcv(
        &self,
        exchange_id: &str,
        symbol: &str,
        timeframe: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<OhlcvCandle>> {
        self.fetch_ohlcv(&OhlcvParams {
            exchange_id: exchange_id.to_string(),
            symbol: symbol.to_string(),
            timeframe: timeframe.map(str::to_string),
            since: None,
            limit,
            until: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ohlcv_candle_deserialization() {
        let json = r#"{"timestamp":1700000000000,"open":35000.0,"high":35100.0,"low":34900.0,"close":35050.0,"volume":123.45}"#;
        let candle: OhlcvCandle = serde_json::from_str(json).unwrap();
        assert_eq!(candle.timestamp, 1700000000000);
        assert_eq!(candle.open, 35000.0);
        assert_eq!(candle.close, 35050.0);
    }

    #[test]
    fn test_ohlcv_candle_iso8601_timestamp() {
        // Test parsing ISO 8601 timestamp format from REST API
        let json = r#"{"timestamp":"2026-04-08T06:50:00.000Z","open":71767.01,"high":71793.89,"low":71755.55,"close":71776.01,"volume":6.11903}"#;
        let candle: OhlcvCandle = serde_json::from_str(json).unwrap();
        // 2026-04-08T06:50:00.000Z should parse to milliseconds
        assert!(candle.timestamp > 0);
        assert_eq!(candle.open, 71767.01);
        assert_eq!(candle.close, 71776.01);
    }

    #[test]
    fn test_exchange_info_deserialization() {
        let json = r#"{"id":"binance","value":"binance","label":"Binance"}"#;
        let info: ExchangeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "binance");
        assert_eq!(info.label, "Binance");
    }

    #[test]
    fn test_symbol_info_deserialization() {
        let json = r#"{"symbol":"BTC/USDT","base":"BTC","quote":"USDT","baseAsset":"btc","quoteAsset":"usdt","type":"spot","active":true}"#;
        let sym: SymbolInfo = serde_json::from_str(json).unwrap();
        assert_eq!(sym.symbol, "BTC/USDT");
        assert_eq!(sym.base_asset, Some("btc".to_string()));
        assert!(sym.active);
    }

    #[test]
    fn test_parse_exchanges_body_plain_strings_and_wrapped() {
        let json_plain = r#"["binance","bybit","okx"]"#;
        let list1 = parse_exchanges_body(json_plain).unwrap();
        assert_eq!(list1.len(), 3);
        assert_eq!(list1[0].id, "binance");
        assert_eq!(list1[0].label, "BINANCE");

        let json_wrapped = r#"{"exchanges":[{"id":"kraken","label":"Kraken"}]}"#;
        let list2 = parse_exchanges_body(json_wrapped).unwrap();
        assert_eq!(list2.len(), 1);
        assert_eq!(list2[0].id, "kraken");
        assert_eq!(list2[0].label, "Kraken");
    }

    #[test]
    fn test_parse_symbols_body_compact_and_wrapped() {
        let json_compact = r#"[{"symbol":"ETH/USDT","active":true},{"symbol":"INACTIVE/USDT","active":false}]"#;
        let list1 = parse_symbols_body(json_compact).unwrap();
        assert_eq!(list1.len(), 1);
        assert_eq!(list1[0].symbol, "ETH/USDT");
        assert_eq!(list1[0].base, "ETH");
        assert_eq!(list1[0].quote, "USDT");
        assert_eq!(list1[0].symbol_type, "spot");

        let json_wrapped = r#"{"symbols":["BTC/USDT","SOL/USDT"]}"#;
        let list2 = parse_symbols_body(json_wrapped).unwrap();
        assert_eq!(list2.len(), 2);
        assert_eq!(list2[0].symbol, "BTC/USDT");
        assert_eq!(list2[0].base, "BTC");
        assert_eq!(list2[1].symbol, "SOL/USDT");
        assert_eq!(list2[1].base, "SOL");
    }
}
