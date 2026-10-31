use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Details of a request that failed because user credits have run out.
#[derive(Clone, Debug, PartialEq)]
pub struct CreditExhaustedInfo {
    /// The module requesting data (e.g. "Chart", "Ticker", "OrderBook", "Trades", "AI Agent")
    pub module: String,
    /// Detailed message from the API or gateway
    pub message: String,
    /// Current credit balance (usually 0)
    pub balance: i64,
    /// Required credits for this operation
    pub required: i64,
    /// Web URL to manage or top-up credits
    pub billing_url: String,
    /// When this credit error occurred
    pub timestamp: Instant,
}

static CREDIT_STORE: OnceLock<Mutex<CreditStore>> = OnceLock::new();
static CREDIT_REVISION: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
pub struct CreditStore {
    global_error: Option<CreditExhaustedInfo>,
    module_errors: HashMap<String, CreditExhaustedInfo>,
    dismissed: bool,
}

impl CreditStore {
    fn instance() -> &'static Mutex<CreditStore> {
        CREDIT_STORE.get_or_init(|| Mutex::new(CreditStore::default()))
    }

    /// Monotonically increasing revision counter for UI change detection.
    pub fn revision() -> u64 {
        CREDIT_REVISION.load(Ordering::Relaxed)
    }

    /// Record that a module's request was rejected due to insufficient credits.
    pub fn record(
        module: &str,
        message: &str,
        balance: i64,
        required: i64,
        base_url: Option<&str>,
    ) {
        let billing_url = match base_url {
            Some(url) if !url.trim().is_empty() => {
                let base = url.trim_end_matches('/');
                format!("{}/account/credits", base)
            }
            _ => {
                let base = crate::data::client::service_base_url();
                format!("{}/account/credits", base.trim_end_matches('/'))
            }
        };

        let msg = if message.trim().is_empty() {
            "Not enough credits for market data requests.".to_string()
        } else {
            message.to_string()
        };

        let info = CreditExhaustedInfo {
            module: module.to_string(),
            message: msg,
            balance,
            required,
            billing_url,
            timestamp: Instant::now(),
        };

        if let Ok(mut store) = Self::instance().lock() {
            store.global_error = Some(info.clone());
            store.module_errors.insert(module.to_lowercase(), info);
            store.dismissed = false;
            CREDIT_REVISION.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Check if credits have run out anywhere on the dashboard.
    pub fn is_out_of_credits() -> bool {
        Self::instance()
            .lock()
            .map(|s| s.global_error.is_some())
            .unwrap_or(false)
    }

    /// Check if the dashboard-level banner was dismissed by the user.
    pub fn is_banner_dismissed() -> bool {
        Self::instance()
            .lock()
            .map(|s| s.dismissed)
            .unwrap_or(false)
    }

    /// Dismiss the dashboard-level banner for now.
    pub fn dismiss_banner() {
        if let Ok(mut store) = Self::instance().lock() {
            store.dismissed = true;
            CREDIT_REVISION.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get the latest credit error across the dashboard.
    pub fn global_error() -> Option<CreditExhaustedInfo> {
        Self::instance().lock().ok()?.global_error.clone()
    }

    /// Get credit error for a specific module type (case-insensitive).
    pub fn module_error(module: &str) -> Option<CreditExhaustedInfo> {
        let store = Self::instance().lock().ok()?;
        let key = module.to_lowercase();
        // Match specific module name or alias
        store
            .module_errors
            .get(&key)
            .cloned()
            .or_else(|| {
                // If any market-data request ran out of credits, all CoinWatch market data modules are affected
                if is_coinwatch_module(&key) || (key != "time" && key != "clock") {
                    store.global_error.clone()
                } else {
                    None
                }
            })
    }

    /// Clear credit errors (e.g. after user tops up or re-authenticates).
    pub fn clear() {
        if let Ok(mut store) = Self::instance().lock() {
            store.global_error = None;
            store.module_errors.clear();
            store.dismissed = false;
            CREDIT_REVISION.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Open CoinWatch credit management / billing page in default web browser.
    pub fn open_billing_page(custom_url: Option<&str>) {
        let target = custom_url
            .filter(|u| !u.trim().is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let base = crate::data::client::service_base_url();
                format!("{}/account/credits", base.trim_end_matches('/'))
            });
        let _ = webbrowser::open(&target);
    }
}

fn is_coinwatch_module(module: &str) -> bool {
    let m = module.trim().to_lowercase();
    matches!(
        m.as_str(),
        "chart"
            | "ticker"
            | "orderbook"
            | "trades"
            | "tape"
            | "execution"
            | "trade"
            | "tradedesk"
            | "watchlist"
            | "marketcap"
            | "market_cap"
            | "market cap"
            | "newsfeed"
            | "news"
            | "positions"
            | "position"
            | "feargreed"
            | "fear_greed"
            | "fear & greed"
            | "agent"
            | "ai agent"
            | "stream"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_credit_store_lifecycle() {
        CreditStore::clear();
        assert!(!CreditStore::is_out_of_credits());
        assert!(CreditStore::global_error().is_none());

        CreditStore::record("Chart", "Balance too low", 0, 10, Some("https://example.com"));
        assert!(CreditStore::is_out_of_credits());

        let err = CreditStore::global_error().expect("error should exist");
        assert_eq!(err.module, "Chart");
        assert_eq!(err.balance, 0);
        assert_eq!(err.required, 10);
        assert_eq!(err.billing_url, "https://example.com/account/credits");

        // CoinWatch module check
        assert!(CreditStore::module_error("ticker").is_some());
        assert!(CreditStore::module_error("orderbook").is_some());
        assert!(CreditStore::module_error("Trade").is_some());
        assert!(CreditStore::module_error("positions").is_some());
        assert!(CreditStore::module_error("FearGreed").is_some());
        assert!(CreditStore::module_error("AI Agent").is_some());
        assert!(CreditStore::module_error("clock").is_none());
        assert!(CreditStore::module_error("time").is_none());

        // Dismiss banner
        assert!(!CreditStore::is_banner_dismissed());
        CreditStore::dismiss_banner();
        assert!(CreditStore::is_banner_dismissed());

        // Clear
        CreditStore::clear();
        assert!(!CreditStore::is_out_of_credits());
        assert!(CreditStore::global_error().is_none());
        assert!(!CreditStore::is_banner_dismissed());
    }
}
