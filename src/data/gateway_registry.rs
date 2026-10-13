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
    /// exchange_id → list of symbol strings
    symbols: HashMap<String, Vec<String>>,
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

    /// Returns the current exchange list.
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

    /// Returns the symbol list for `exchange`.
    ///
    /// Returns built-in defaults until the exchange's symbol fetch completes.
    /// Automatically triggers a background fetch on first call for this exchange.
    pub fn symbols(&self, exchange: &str) -> Vec<String> {
        let fetching = {
            let inner = self.inner.lock().unwrap();
            if let Some(syms) = inner.symbols.get(exchange)
                && !syms.is_empty()
            {
                return syms.clone();
            }
            inner.symbol_fetches_in_flight.contains(exchange)
        };

        if !fetching {
            self.prefetch_symbols(exchange);
        }

        DEFAULT_SYMBOLS.iter().map(|s| s.to_string()).collect()
    }

    /// Check if symbols for `exchange` have been loaded from the API.
    pub fn symbols_loaded(&self, exchange: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.symbols.contains_key(exchange)
    }

    // -----------------------------------------------------------------------
    // Background fetches
    // -----------------------------------------------------------------------

    /// Kick off a background fetch for the full exchange list.
    pub fn prefetch_exchanges(&self) {
        let inner = Arc::clone(&self.inner);
        tokio_runtime().spawn(async move {
            fetch_exchanges(inner).await;
        });
    }

    /// Kick off a background fetch for `exchange` symbols (no-op if already
    /// loaded or currently fetching).
    pub fn prefetch_symbols(&self, exchange: &str) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.symbols.contains_key(exchange)
                || inner.symbol_fetches_in_flight.contains(exchange)
            {
                return;
            }
            inner.symbol_fetches_in_flight.insert(exchange.to_string());
        }

        let inner = Arc::clone(&self.inner);
        let exchange = exchange.to_string();
        tokio_runtime().spawn(async move {
            fetch_symbols(inner, &exchange).await;
        });
    }

    /// Force a background fetch for `exchange` symbols, bypassing the cache.
    /// Use this when you need fresh data from the API regardless of cached state.
    pub fn force_refresh_symbols(&self, exchange: &str) {
        let inner = Arc::clone(&self.inner);
        let exchange = exchange.to_string();
        tokio_runtime().spawn(async move {
            fetch_symbols(inner, &exchange).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Async fetch helpers
// ---------------------------------------------------------------------------

/// Envelope returned by `GET /api/v1/exchanges`.
/// The navigator may return `{id, value, label}` objects or plain strings.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ExchangeEntry {
    Object { id: String },
    Plain(String),
}

impl ExchangeEntry {
    fn id(self) -> String {
        match self {
            ExchangeEntry::Object { id } => id,
            ExchangeEntry::Plain(s) => s,
        }
    }
}

async fn fetch_exchanges(inner: Arc<Mutex<RegistryInner>>) {
    let url = format!("{}/api/v1/exchanges", gateway_base());
    info!("GatewayRegistry: fetching exchanges from {url}");

    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => match resp.json::<Vec<ExchangeEntry>>().await {
            Ok(entries) => {
                let ids: Vec<String> = entries.into_iter().map(|e| e.id()).collect();
                info!("GatewayRegistry: {} exchanges loaded", ids.len());
                inner.lock().unwrap().exchanges = ids;
            }
            Err(e) => error!("GatewayRegistry: exchanges parse error: {e}"),
        },
        Ok(r) => error!("GatewayRegistry: exchanges HTTP {}", r.status()),
        Err(e) => error!("GatewayRegistry: exchanges fetch error: {e}"),
    }
}

/// One symbol entry from `GET /api/v1/symbols?exchange=<id>`.
#[derive(serde::Deserialize)]
struct SymbolEntry {
    symbol: String,
    #[serde(default = "default_true")]
    active: bool,
    #[serde(rename = "type", default)]
    #[allow(dead_code)]
    symbol_type: String,
}

fn default_true() -> bool {
    true
}

async fn fetch_symbols(inner: Arc<Mutex<RegistryInner>>, exchange: &str) {
    let url = format!(
        "{}/api/v1/symbols?exchange={}",
        gateway_base(),
        urlencoding::encode(exchange),
    );
    info!("GatewayRegistry: fetching symbols for {exchange}");

    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<Vec<SymbolEntry>>().await {
                Ok(entries) => {
                    // Keep active symbols only (include all types: spot, future, swap).
                    let mut syms: Vec<String> = entries
                        .into_iter()
                        .filter(|e| e.active)
                        .map(|e| e.symbol)
                        .collect();
                    syms.sort();
                    info!(
                        "GatewayRegistry: {} symbols loaded for {exchange}",
                        syms.len()
                    );
                    let mut inner = inner.lock().unwrap();
                    inner.symbols.insert(exchange.to_string(), syms);
                    inner.symbol_fetches_in_flight.remove(exchange);
                }
                Err(e) => {
                    error!("GatewayRegistry: symbols parse error for {exchange}: {e}");
                    inner
                        .lock()
                        .unwrap()
                        .symbol_fetches_in_flight
                        .remove(exchange);
                }
            }
        }
        Ok(r) => {
            error!(
                "GatewayRegistry: symbols HTTP {} for {exchange}",
                r.status()
            );
            inner
                .lock()
                .unwrap()
                .symbol_fetches_in_flight
                .remove(exchange);
        }
        Err(e) => {
            error!("GatewayRegistry: symbols fetch error for {exchange}: {e}");
            inner
                .lock()
                .unwrap()
                .symbol_fetches_in_flight
                .remove(exchange);
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
}
