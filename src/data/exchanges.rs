use crate::data::client::{ApiClient, parse_api_response, tokio_runtime};
use crate::data::error::Result;
use crate::data::models::{ExchangeInfo, MarketInfo, OhlcvCandle, SymbolInfo};

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
    /// List all supported exchanges.
    /// GET /api/exchanges (public, no auth required)
    ///
    /// Blocking — run from a background thread.
    pub fn list_exchanges(&self) -> Result<Vec<ExchangeInfo>> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url("/api/exchanges");

        rt.block_on(async {
            let resp = http.get(&url).send().await?;
            parse_api_response(resp).await
        })
    }

    /// List market types for an exchange (e.g. spot, future, swap).
    /// GET /api/exchanges/{exchangeId}/markets (public)
    ///
    /// Blocking — run from a background thread.
    pub fn list_markets(&self, exchange_id: &str) -> Result<Vec<MarketInfo>> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url(&format!("/api/exchanges/{}/markets", exchange_id));

        rt.block_on(async {
            let resp = http.get(&url).send().await?;
            parse_api_response(resp).await
        })
    }

    /// List trading symbols for an exchange, optionally filtered by market type.
    /// GET /api/exchanges/{exchangeId}/symbols?marketType=spot (public)
    ///
    /// Blocking — run from a background thread.
    pub fn list_symbols(
        &self,
        exchange_id: &str,
        market_type: Option<&str>,
    ) -> Result<Vec<SymbolInfo>> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url(&format!("/api/exchanges/{}/symbols", exchange_id));
        let mt = market_type.map(|s| s.to_string());

        rt.block_on(async {
            let mut req = http.get(&url);
            if let Some(ref mt) = mt {
                req = req.query(&[("marketType", mt.as_str())]);
            }
            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// List available timeframes for an exchange (e.g. "1m", "5m", "1h", "1d").
    /// GET /api/exchanges/{exchangeId}/timeframes (public)
    ///
    /// Blocking — run from a background thread.
    pub fn list_timeframes(&self, exchange_id: &str) -> Result<Vec<String>> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url(&format!("/api/exchanges/{}/timeframes", exchange_id));

        rt.block_on(async {
            let resp = http.get(&url).send().await?;
            parse_api_response(resp).await
        })
    }

    /// Fetch OHLCV candle data for a symbol on an exchange.
    /// GET /api/exchanges/{exchangeId}/ohlcv?symbol=...&timeframe=...&since=...&limit=...&until=... (public)
    ///
    /// Blocking — run from a background thread.
    pub fn fetch_ohlcv(&self, params: &OhlcvParams) -> Result<Vec<OhlcvCandle>> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url(&format!("/api/exchanges/{}/ohlcv", params.exchange_id));
        let symbol = params.symbol.clone();
        let timeframe = params.timeframe.clone();
        let since = params.since;
        let limit = params.limit;
        let until = params.until;

        rt.block_on(async {
            let mut req = http.get(&url).query(&[("symbol", &symbol)]);

            if let Some(ref tf) = timeframe {
                req = req.query(&[("timeframe", tf)]);
            }
            if let Some(since) = since {
                req = req.query(&[("since", &since.to_string())]);
            }
            if let Some(limit) = limit {
                req = req.query(&[("limit", &limit.to_string())]);
            }
            if let Some(until) = until {
                req = req.query(&[("until", &until.to_string())]);
            }

            let resp = req.send().await?;
            parse_api_response(resp).await
        })
    }

    /// Fetch the latest OHLCV candles for a symbol.
    /// GET /api/exchanges/{exchangeId}/ohlcv/latest?symbol=...&timeframe=...&limit=... (public)
    ///
    /// Blocking — run from a background thread.
    pub fn fetch_ohlcv_latest(
        &self,
        exchange_id: &str,
        symbol: &str,
        timeframe: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<OhlcvCandle>> {
        let rt = tokio_runtime();
        let http = self.http().clone();
        let url = self.url(&format!("/api/exchanges/{}/ohlcv/latest", exchange_id));
        let sym = symbol.to_string();
        let tf = timeframe.map(|s| s.to_string());

        rt.block_on(async {
            let mut req = http.get(&url).query(&[("symbol", &sym)]);

            if let Some(ref tf) = tf {
                req = req.query(&[("timeframe", tf.as_str())]);
            }
            if let Some(lim) = limit {
                req = req.query(&[("limit", &lim.to_string())]);
            }

            let resp = req.send().await?;
            parse_api_response(resp).await
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
}
