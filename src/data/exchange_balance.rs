//! Read-only live account balance lookups through CCXT.
//!
//! The trade desk keeps its own paper/simulation ledger. In live mode the
//! available balance has to come from the selected exchange instead, so this
//! module performs a small authenticated `fetch_balance` request using the
//! first configured API credential for that venue.

use crate::data::api_management;
use crate::data::trade_store::MarketType;
use ccxt::{Config, Params, TypedExchangeExt};

/// Fetch the free balance of `quote` for `exchange`.
///
/// Returns the `free` amount reported by the exchange, falling back to `total`
/// when the venue does not expose a free balance. The amount is expressed in
/// the quote currency (e.g. USDT), which is what the trade desk uses to size
/// margin.
pub async fn fetch_available_balance(
    exchange: &str,
    quote: &str,
    market_type: MarketType,
) -> Result<f64, String> {
    let exchange_id = exchange.trim().to_lowercase();
    if exchange_id.is_empty() {
        return Err("No exchange selected".to_string());
    }

    let credential = api_management::get_credentials_for_exchange(&exchange_id)
        .into_iter()
        .find(|c| !c.api_key.trim().is_empty() && !c.api_secret.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "No API credentials configured for {}",
                exchange_id.to_uppercase()
            )
        })?;

    let mut config = Config::new()
        .api_key(credential.api_key.trim())
        .secret(credential.api_secret.trim())
        .enable_rate_limit(true)
        .timeout_ms(15_000);
    if !credential.passphrase.trim().is_empty() {
        config = config.password(credential.passphrase.trim());
    }
    if credential.is_testnet {
        config = config.sandbox(true);
    }
    // Dedicated futures venues already configure their own account type. For
    // unified venues, make the selected market explicit.
    if !MarketType::is_dedicated_futures_exchange(&exchange_id) {
        config = config.option_str("defaultType", market_type.ccxt_type(&exchange_id));
    }

    let mut client = ccxt::from_id(&exchange_id, config.into_option()).ok_or_else(|| {
        format!(
            "{} is not included in this build of ccxt",
            exchange_id.to_uppercase()
        )
    })?;

    let balance_params = if MarketType::is_dedicated_futures_exchange(&exchange_id) {
        Params::none()
    } else {
        Params::new().with_str("type", market_type.ccxt_type(&exchange_id))
    };
    let balances = client.fetch_balance(balance_params).await.map_err(|err| {
        let message = err.to_string();
        if message.contains("AuthenticationError") || message.contains("Invalid API-key") {
            format!(
                "{} rejected the API credentials (check key, secret, IP whitelist and permissions)",
                exchange_id.to_uppercase()
            )
        } else {
            format!(
                "{} balance request failed: {message}",
                exchange_id.to_uppercase()
            )
        }
    })?;

    let code = quote.trim().to_uppercase();
    let amount = balances
        .free
        .get(&code)
        .copied()
        .or_else(|| {
            balances
                .free
                .iter()
                .find(|(currency, _)| currency.eq_ignore_ascii_case(&code))
                .map(|(_, value)| *value)
        })
        .or_else(|| balances.total.get(&code).copied())
        .or_else(|| {
            balances
                .total
                .iter()
                .find(|(currency, _)| currency.eq_ignore_ascii_case(&code))
                .map(|(_, value)| *value)
        })
        .unwrap_or(0.0);

    if !amount.is_finite() || amount < 0.0 {
        return Err(format!(
            "{} returned an invalid {code} balance",
            exchange_id.to_uppercase()
        ));
    }

    Ok(amount)
}

#[cfg(test)]
mod tests {
    #[test]
    fn compiled_venues_are_available_to_the_desk() {
        for id in [
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
        ] {
            assert!(
                ccxt::from_id(id, None).is_some(),
                "{id} should be compiled into ccxt"
            );
        }
        assert!(ccxt::from_id("not-a-real-exchange", None).is_none());
    }
}
