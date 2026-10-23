/// Canonical unified tick/trade representation.
///
/// All exchange data is normalised into this struct before entering the
/// render pipeline, backtest engine, or storage layer.
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Exchange identifier (e.g. "binance", "coinbase").
pub type ExchangeId = String;

/// Trading pair symbol (e.g. "BTC/USDT").
pub type Symbol = String;

/// Price as a 64-bit float — sufficient for all crypto pairs at MVP scale.
pub type Price = f64;

/// Trade amount (base currency).
pub type Amount = f64;

/// Unix timestamp in milliseconds.
pub type TimestampMs = i64;

/// Side of the trade from the taker's perspective.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradeSide {
    Buy,
    Sell,
    Unknown,
}

impl TradeSide {
    /// True when the taker was the buyer (price typically moves up).
    pub fn is_buy(&self) -> bool {
        matches!(self, TradeSide::Buy)
    }
}

// ---------------------------------------------------------------------------
// UnifiedTick — the canonical trade struct
// ---------------------------------------------------------------------------

/// A single normalised trade event from any exchange.
///
/// Produced by the CoinWatch bridge, broadcast to all consumers
/// (Tape panel, SQLite writer, backtest engine).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnifiedTick {
    /// Exchange the trade originated from.
    pub exchange: ExchangeId,
    /// Trading pair (e.g. "BTC/USDT").
    pub symbol: Symbol,
    /// Executed price.
    pub price: Price,
    /// Traded amount in base currency.
    pub amount: Amount,
    /// Taker side.
    pub side: TradeSide,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: TimestampMs,
    /// Rolling 24-hour ticker statistics; absent on individual trades.
    #[serde(default)]
    pub change_24h_percent: Option<f64>,
    #[serde(default)]
    pub quote_volume_24h: Option<f64>,
    #[serde(default)]
    pub open_24h: Option<f64>,
    #[serde(default)]
    pub high_24h: Option<f64>,
    #[serde(default)]
    pub low_24h: Option<f64>,
}

impl UnifiedTick {
    /// Construct a new tick.
    pub fn new(
        exchange: impl Into<ExchangeId>,
        symbol: impl Into<Symbol>,
        price: Price,
        amount: Amount,
        side: TradeSide,
        timestamp_ms: TimestampMs,
    ) -> Self {
        Self {
            exchange: exchange.into(),
            symbol: symbol.into(),
            price,
            amount,
            side,
            timestamp_ms,
            change_24h_percent: None,
            quote_volume_24h: None,
            open_24h: None,
            high_24h: None,
            low_24h: None,
        }
    }

    /// Notional value (price × amount).
    pub fn notional(&self) -> f64 {
        self.price * self.amount
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tick(price: f64, amount: f64, side: TradeSide) -> UnifiedTick {
        UnifiedTick::new(
            "binance",
            "BTC/USDT",
            price,
            amount,
            side,
            1_700_000_000_000,
        )
    }

    #[test]
    fn notional_is_price_times_amount() {
        let tick = make_tick(50_000.0, 0.5, TradeSide::Buy);
        assert!((tick.notional() - 25_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn trade_side_is_buy_works() {
        assert!(TradeSide::Buy.is_buy());
        assert!(!TradeSide::Sell.is_buy());
        assert!(!TradeSide::Unknown.is_buy());
    }

    #[test]
    fn round_trip_serde_json() {
        let tick = make_tick(30_000.0, 0.1, TradeSide::Sell);
        let json = serde_json::to_string(&tick).expect("serialize");
        let back: UnifiedTick = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.exchange, "binance");
        assert_eq!(back.symbol, "BTC/USDT");
        assert!((back.price - 30_000.0).abs() < f64::EPSILON);
        assert_eq!(back.side, TradeSide::Sell);
    }

    #[test]
    fn serde_side_lowercase() {
        let json = r#"{"exchange":"coinbase","symbol":"ETH/USD","price":2000.0,"amount":1.0,"side":"buy","timestamp_ms":0}"#;
        let tick: UnifiedTick = serde_json::from_str(json).expect("deserialize");
        assert_eq!(tick.side, TradeSide::Buy);
    }

    #[test]
    fn serde_side_unknown() {
        let json = r#"{"exchange":"kraken","symbol":"SOL/USDT","price":100.0,"amount":5.0,"side":"unknown","timestamp_ms":0}"#;
        let tick: UnifiedTick = serde_json::from_str(json).expect("deserialize");
        assert_eq!(tick.side, TradeSide::Unknown);
    }
}
