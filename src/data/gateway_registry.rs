/// Gateway registry — live exchange and symbol lists from `coinwat.ch`.
///
/// Populated asynchronously in the background at startup; falls back to
/// built-in defaults until the API responds so the UI is never blocked.
///
/// Access via the process-wide singleton [`GatewayRegistry::global()`].
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::{error, info};

use crate::data::client::{service_base_url, tokio_runtime};
use crate::data::exchanges::{parse_exchanges_body, parse_symbols_body};
use crate::data::models::{ExchangeInfo, SymbolInfo};
use crate::data::session_store;

// ---------------------------------------------------------------------------
// Defaults — shown while the API fetch is in flight
// ---------------------------------------------------------------------------

/// WS-gateway-compatible exchanges (mirrors `SUPPORTED_EXCHANGES` in
/// `exchange-stream.do.ts`).
const DEFAULT_EXCHANGES: &[&str] = &[
    "binance",
    "binanceusdm",
    "binancecoinm",
    "bybit",
    "okx",
    "kraken",
    "krakenfutures",
    "coinbaseexchange",
    "kucoin",
    "kucoinfutures",
    "mexc",
    "htx",
    "bitfinex",
    "gemini",
];

const DEFAULT_SYMBOLS: &[&str] = &[
    "BTC/USDT",
    "ETH/USDT",
    "SOL/USDT",
    "BNB/USDT",
    "XRP/USDT",
    "ADA/USDT",
    "DOGE/USDT",
    "MATIC/USDT",
];

/// REST base shared with `ApiClient` and `BridgeConfig`.
/// Its build-channel default is staging on beta and production on release.
fn gateway_base() -> String {
    service_base_url()
}

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RegistryInner {
    exchanges: Vec<String>,
    exchange_infos: Vec<ExchangeInfo>,
    /// exchange_id → list of symbol strings
    symbols: HashMap<String, Vec<String>>,
    /// exchange_id → list of rich SymbolInfo
    symbol_infos: HashMap<String, Vec<SymbolInfo>>,
    /// set of exchange IDs for which a symbol fetch is already in flight
    symbol_fetches_in_flight: std::collections::HashSet<String>,
}

// ---------------------------------------------------------------------------
// GatewayRegistry
// ---------------------------------------------------------------------------

/// Process-wide registry of available exchanges and their trading symbols,
/// loaded lazily from the `coinwat.ch` gateway.
pub struct GatewayRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

static REGISTRY: OnceLock<GatewayRegistry> = OnceLock::new();

impl GatewayRegistry {
    /// Returns the process-wide singleton, initialising it on first call.
    ///
    /// The first call also kicks off a background fetch for the exchange list.
    pub fn global() -> &'static Self {
        REGISTRY.get_or_init(|| {
            let reg = Self {
                inner: Arc::new(Mutex::new(RegistryInner::default())),
            };
            reg.prefetch_exchanges();
            reg
        })
    }

    // -----------------------------------------------------------------------
    // Accessors — always return something (default until loaded)
    // -----------------------------------------------------------------------

    /// Returns the current exchange list as IDs.
    ///
    /// Returns built-in defaults until the API fetch completes.
    pub fn exchanges(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        if inner.exchanges.is_empty() {
            DEFAULT_EXCHANGES.iter().map(|s| s.to_string()).collect()
        } else {
            inner.exchanges.clone()
        }
    }

    /// Returns the full exchange metadata list.
    pub fn exchange_infos(&self) -> Vec<ExchangeInfo> {
        let inner = self.inner.lock().unwrap();
        if inner.exchange_infos.is_empty() {
            DEFAULT_EXCHANGES
                .iter()
                .map(|&id| ExchangeInfo::from_id(id))
                .collect()
        } else {
            inner.exchange_infos.clone()
        }
    }

    /// Returns the symbol list for `exchange`.
    ///
    /// Returns built-in defaults until the exchange's symbol fetch completes.
    /// Automatically triggers a background fetch on first call for this exchange.
    pub fn symbols(&self, exchange: &str) -> Vec<String> {
        let lower = exchange.trim().to_lowercase();
        let fetching = {
            let inner = self.inner.lock().unwrap();
            if let Some(syms) = inner.symbols.get(exchange).or_else(|| inner.symbols.get(&lower))
                && !syms.is_empty()
            {
                return syms.clone();
            }
            inner.symbol_fetches_in_flight.contains(exchange)
                || inner.symbol_fetches_in_flight.contains(&lower)
        };

        if !fetching {
            self.prefetch_symbols(exchange);
        }

        DEFAULT_SYMBOLS.iter().map(|s| s.to_string()).collect()
    }

    /// Returns the rich SymbolInfo list for `exchange`.
    pub fn symbol_infos(&self, exchange: &str) -> Vec<SymbolInfo> {
        let lower = exchange.trim().to_lowercase();
        let fetching = {
            let inner = self.inner.lock().unwrap();
            if let Some(syms) = inner.symbol_infos.get(exchange).or_else(|| inner.symbol_infos.get(&lower))
                && !syms.is_empty()
            {
                return syms.clone();
            }
            inner.symbol_fetches_in_flight.contains(exchange)
                || inner.symbol_fetches_in_flight.contains(&lower)
        };

        if !fetching {
            self.prefetch_symbols(exchange);
        }

        DEFAULT_SYMBOLS
            .iter()
            .map(|&s| SymbolInfo::from_symbol(s))
            .collect()
    }

    /// Snapshot of fetched exchange IDs and their fetched symbols, without fallback entries.
    pub fn fetched_catalog(&self) -> Vec<(String, Vec<SymbolInfo>)> {
        let inner = self.inner.lock().unwrap();
        inner.exchanges.iter().map(|exchange| {
            (exchange.clone(), inner.symbol_infos.get(exchange).cloned().unwrap_or_default())
        }).collect()
    }

    /// Check if symbols for `exchange` have been loaded from the API.
    pub fn symbols_loaded(&self, exchange: &str) -> bool {
        let lower = exchange.trim().to_lowercase();
        let inner = self.inner.lock().unwrap();
        inner.symbols.contains_key(exchange) || inner.symbols.contains_key(&lower)
    }

    // -----------------------------------------------------------------------
    // Background fetches
    // -----------------------------------------------------------------------

    /// Kick off a background fetch for the full exchange list.
    pub fn prefetch_exchanges(&self) {
        self.prefetch_exchanges_with_token(None);
    }

    /// Kick off a background fetch for the exchange list, using an explicit session token if provided.
    pub fn prefetch_exchanges_with_token(&self, token: Option<&str>) {
        let inner = Arc::clone(&self.inner);
        let token_opt = token.map(str::to_string);
        tokio_runtime().spawn(async move {
            fetch_exchanges(inner, token_opt.as_deref()).await;
        });
    }

    /// Kick off a background fetch for `exchange` symbols (no-op if already
    /// loaded or currently fetching).
    pub fn prefetch_symbols(&self, exchange: &str) {
        self.prefetch_symbols_with_token(exchange, None);
    }

    /// Kick off a background fetch for `exchange` symbols with optional token.
    pub fn prefetch_symbols_with_token(&self, exchange: &str, token: Option<&str>) {
        let lower = exchange.trim().to_lowercase();
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.symbols.contains_key(exchange)
                || inner.symbols.contains_key(&lower)
                || inner.symbol_fetches_in_flight.contains(exchange)
                || inner.symbol_fetches_in_flight.contains(&lower)
            {
                return;
            }
            inner.symbol_fetches_in_flight.insert(exchange.to_string());
            inner.symbol_fetches_in_flight.insert(lower.clone());
        }

        let inner = Arc::clone(&self.inner);
        let exchange = exchange.to_string();
        let token_opt = token.map(str::to_string);
        tokio_runtime().spawn(async move {
            fetch_symbols(inner, &exchange, token_opt.as_deref()).await;
        });
    }

    /// Force a background fetch for `exchange` symbols, bypassing the cache.
    pub fn force_refresh_symbols(&self, exchange: &str) {
        self.force_refresh_symbols_with_token(exchange, None);
    }

    /// Force a background fetch for `exchange` symbols with optional token.
    pub fn force_refresh_symbols_with_token(&self, exchange: &str, token: Option<&str>) {
        let inner = Arc::clone(&self.inner);
        let exchange = exchange.to_string();
        let token_opt = token.map(str::to_string);
        tokio_runtime().spawn(async move {
            fetch_symbols(inner, &exchange, token_opt.as_deref()).await;
        });
    }

    /// Prefetch symbols for every exchange currently returned by the exchange list.
    pub fn prefetch_all_symbols(&self) {
        let exchanges = self.inner.lock().unwrap().exchanges.clone();
        for exchange in exchanges {
            self.prefetch_symbols(&exchange);
        }
    }

    /// Prefetch exchanges and symbols for all exchanges configured in the user's modules during a login session.
    pub fn prefetch_for_modules(&self, module_exchanges: &[String], token: Option<&str>) {
        self.prefetch_exchanges_with_token(token);
        for exchange in module_exchanges {
            let ex = exchange.trim();
            if !ex.is_empty() {
                self.prefetch_symbols_with_token(ex, token);
            }
        }
    }

    /// Refresh all registry data using the active login session.
    pub fn refresh_for_session(&self, token: Option<&str>) {
        let loaded_exchanges: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner.symbols.keys().cloned().collect()
        };
        self.prefetch_exchanges_with_token(token);
        for exchange in loaded_exchanges {
            self.force_refresh_symbols_with_token(&exchange, token);
        }
    }
}

// ---------------------------------------------------------------------------
// Async fetch helpers
// ---------------------------------------------------------------------------

async fn fetch_exchanges(inner: Arc<Mutex<RegistryInner>>, token: Option<&str>) {
    let base = gateway_base();
    let url = format!("{base}/api/v1/exchanges");
    info!("GatewayRegistry: fetching exchanges from CoinWatch API: {url}");

    let client = reqwest::Client::new();
    let mut req = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Origin", base.clone())
        .header("Referer", format!("{}/", base));

    if let Some(t) = token {
        req = req.bearer_auth(t);
    } else if let Ok(Some(session)) = session_store::load_session() {
        req = req.bearer_auth(session.access_token);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.text().await {
                Ok(body) => match parse_exchanges_body(&body) {
                    Ok(infos) => {
                        let ids: Vec<String> = infos.iter().map(|e| e.id.clone()).collect();
                        info!("GatewayRegistry: {} exchanges loaded from CoinWatch API", ids.len());
                        let mut guard = inner.lock().unwrap();
                        guard.exchanges = ids;
                        guard.exchange_infos = infos;
                    }
                    Err(e) => error!("GatewayRegistry: exchanges parse error: {e}"),
                },
                Err(e) => error!("GatewayRegistry: failed to read exchanges response: {e}"),
            }
        }
        Ok(resp) => {
            error!("GatewayRegistry: exchanges HTTP {}", resp.status());
        }
        Err(e) => {
            error!("GatewayRegistry: exchanges fetch error: {e}");
        }
    }
}

async fn fetch_symbols(inner: Arc<Mutex<RegistryInner>>, exchange: &str, token: Option<&str>) {
    let base = gateway_base();
    let lower_exchange = exchange.trim().to_lowercase();
    let encoded_exchange = urlencoding::encode(&lower_exchange);
    let url = format!("{base}/api/v1/symbols?exchange={encoded_exchange}");
    info!("GatewayRegistry: fetching symbols for {exchange} from CoinWatch API: {url}");

    let client = reqwest::Client::new();
    let mut req = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Origin", base.clone())
        .header("Referer", format!("{}/", base));

    if let Some(t) = token {
        req = req.bearer_auth(t);
    } else if let Ok(Some(session)) = session_store::load_session() {
        req = req.bearer_auth(session.access_token);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            match resp.text().await {
                Ok(body) => match parse_symbols_body(&body) {
                    Ok(infos) => {
                        let sym_strings: Vec<String> = infos.iter().map(|s| s.symbol.clone()).collect();
                        info!("GatewayRegistry: {} symbols loaded for {exchange}", sym_strings.len());
                        let mut guard = inner.lock().unwrap();
                        guard.symbols.insert(exchange.to_string(), sym_strings.clone());
                        guard.symbols.insert(lower_exchange.clone(), sym_strings);
                        guard.symbol_infos.insert(exchange.to_string(), infos.clone());
                        guard.symbol_infos.insert(lower_exchange.clone(), infos);
                        guard.symbol_fetches_in_flight.remove(exchange);
                        guard.symbol_fetches_in_flight.remove(&lower_exchange);
                    }
                    Err(e) => {
                        error!("GatewayRegistry: symbols parse error for {exchange}: {e}");
                        let mut guard = inner.lock().unwrap();
                        guard.symbol_fetches_in_flight.remove(exchange);
                        guard.symbol_fetches_in_flight.remove(&lower_exchange);
                    }
                },
                Err(e) => {
                    error!("GatewayRegistry: failed to read symbols response for {exchange}: {e}");
                    let mut guard = inner.lock().unwrap();
                    guard.symbol_fetches_in_flight.remove(exchange);
                    guard.symbol_fetches_in_flight.remove(&lower_exchange);
                }
            }
        }
        Ok(resp) => {
            error!("GatewayRegistry: symbols HTTP {} for {exchange}", resp.status());
            let mut guard = inner.lock().unwrap();
            guard.symbol_fetches_in_flight.remove(exchange);
            guard.symbol_fetches_in_flight.remove(&lower_exchange);
        }
        Err(e) => {
            error!("GatewayRegistry: symbols fetch error for {exchange}: {e}");
            let mut guard = inner.lock().unwrap();
            guard.symbol_fetches_in_flight.remove(exchange);
            guard.symbol_fetches_in_flight.remove(&lower_exchange);
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_returned_before_load() {
        let reg = GatewayRegistry {
            inner: Arc::new(Mutex::new(RegistryInner::default())),
        };
        // Exchanges: returns built-in defaults when empty
        let exch = reg.exchanges();
        assert!(!exch.is_empty());
        assert!(exch.contains(&"binance".to_string()));

        // Symbols: returns built-in defaults when empty
        let syms = reg.symbols("binance");
        assert!(!syms.is_empty());
        assert!(syms.contains(&"BTC/USDT".to_string()));
    }

    #[test]
    fn loaded_exchanges_override_defaults() {
        let reg = GatewayRegistry {
            inner: Arc::new(Mutex::new(RegistryInner {
                exchanges: vec!["myexchange".to_string()],
                ..Default::default()
            })),
        };
        let exch = reg.exchanges();
        assert_eq!(exch, vec!["myexchange".to_string()]);
    }

    #[test]
    fn loaded_symbols_override_defaults() {
        let mut sym_map = HashMap::new();
        sym_map.insert(
            "binance".to_string(),
            vec!["ETH/BTC".to_string(), "SHIB/USDT".to_string()],
        );
        let reg = GatewayRegistry {
            inner: Arc::new(Mutex::new(RegistryInner {
                symbols: sym_map,
                ..Default::default()
            })),
        };
        let syms = reg.symbols("binance");
        assert!(syms.contains(&"ETH/BTC".to_string()));
        assert!(!syms.contains(&"BTC/USDT".to_string()));
    }

    #[test]
    fn unknown_exchange_returns_defaults() {
        let reg = GatewayRegistry {
            inner: Arc::new(Mutex::new(RegistryInner::default())),
        };
        // Even with no loaded data, returns built-in defaults
        let syms = reg.symbols("some_unknown_exchange");
        assert!(!syms.is_empty());
    }

    #[test]
    fn exchange_and_symbol_infos_fallback_and_loaded() {
        let reg = GatewayRegistry {
            inner: Arc::new(Mutex::new(RegistryInner::default())),
        };
        let ex_infos = reg.exchange_infos();
        assert!(!ex_infos.is_empty());
        assert_eq!(ex_infos[0].id, "binance");

        let sym_infos = reg.symbol_infos("binance");
        assert!(!sym_infos.is_empty());
        assert_eq!(sym_infos[0].symbol, "BTC/USDT");
        assert_eq!(sym_infos[0].base, "BTC");
        assert_eq!(sym_infos[0].quote, "USDT");
    }
}
