/// Canonical unified order book representation.
///
/// Level-2 bids and asks normalised from any exchange.  Produced by the CoinWatch
/// bridge and broadcast to all consumers (OrderBook panel, future
/// analytics, backtest engine).
use serde::{Deserialize, Serialize};

use crate::data::tick::{ExchangeId, Symbol, TimestampMs};

// ---------------------------------------------------------------------------
// PriceLevel — a single row in the order book
// ---------------------------------------------------------------------------

/// A single price level in the order book.
///
/// Represents the total quantity available at `price` on one side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel {
    /// Limit price for this level.
    pub price: f64,
    /// Total quantity available at this price.
    pub quantity: f64,
}

impl PriceLevel {
    /// Construct a new price level.
    pub fn new(price: f64, quantity: f64) -> Self {
        Self { price, quantity }
    }

    /// Notional value at this level (price × quantity).
    pub fn notional(&self) -> f64 {
        self.price * self.quantity
    }
}

// ---------------------------------------------------------------------------
// OrderBook — a full L2 snapshot
// ---------------------------------------------------------------------------

/// A normalised Level-2 order book snapshot.
///
/// `bids` are sorted descending by price (best bid first).
/// `asks` are sorted ascending by price (best ask first).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrderBook {
    /// Exchange the snapshot originated from.
    pub exchange: ExchangeId,
    /// Trading pair (e.g. "BTC/USDT").
    pub symbol: Symbol,
    /// Bid side — sorted best (highest) first.
    pub bids: Vec<PriceLevel>,
    /// Ask side — sorted best (lowest) first.
    pub asks: Vec<PriceLevel>,
    /// Unix timestamp in milliseconds.
    pub timestamp_ms: TimestampMs,
}

impl OrderBook {
    /// Construct a new order book snapshot.
    pub fn new(
        exchange: impl Into<ExchangeId>,
        symbol: impl Into<Symbol>,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        timestamp_ms: TimestampMs,
    ) -> Self {
        Self {
            exchange: exchange.into(),
            symbol: symbol.into(),
            bids,
            asks,
            timestamp_ms,
        }
    }

    /// Best bid price, or `None` if the book is empty.
    pub fn best_bid(&self) -> Option<f64> {
        self.bids.first().map(|l| l.price)
    }

    /// Best ask price, or `None` if the book is empty.
    pub fn best_ask(&self) -> Option<f64> {
        self.asks.first().map(|l| l.price)
    }

    /// Bid-ask spread (best ask − best bid), or `None` if either side is empty.
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    /// Mid-price ((best bid + best ask) / 2), or `None` if either side is empty.
    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
            _ => None,
        }
    }

    /// Return the top `n` bid levels (capped at available depth).
    pub fn top_bids(&self, n: usize) -> &[PriceLevel] {
        &self.bids[..self.bids.len().min(n)]
    }

    /// Return the top `n` ask levels (capped at available depth).
    pub fn top_asks(&self, n: usize) -> &[PriceLevel] {
        &self.asks[..self.asks.len().min(n)]
    }

    /// Maximum quantity on either side (used to normalise size bars).
    pub fn max_quantity(&self, depth: usize) -> f64 {
        let max_bid = self
            .top_bids(depth)
            .iter()
            .map(|l| l.quantity)
            .fold(0.0_f64, f64::max);
        let max_ask = self
            .top_asks(depth)
            .iter()
            .map(|l| l.quantity)
            .fold(0.0_f64, f64::max);
        max_bid.max(max_ask)
    }
}

// ---------------------------------------------------------------------------
// Wire format — what the bridge emits for order book updates
// ---------------------------------------------------------------------------

/// Raw order book message as emitted by the CoinWatch bridge.
///
/// Each level is `[price, quantity]` — matching CCXT's own format.
#[derive(Debug, Deserialize)]
pub struct RawOrderBook {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub bids: Vec<[f64; 2]>,
    pub asks: Vec<[f64; 2]>,
    pub timestamp_ms: TimestampMs,
}

impl From<RawOrderBook> for OrderBook {
    fn from(raw: RawOrderBook) -> Self {
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
        OrderBook::new(raw.exchange, raw.symbol, bids, asks, raw.timestamp_ms)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_book() -> OrderBook {
        OrderBook::new(
            "binance",
            "BTC/USDT",
            vec![
                PriceLevel::new(67_000.0, 1.5),
                PriceLevel::new(66_990.0, 3.0),
                PriceLevel::new(66_980.0, 5.0),
            ],
            vec![
                PriceLevel::new(67_010.0, 1.0),
                PriceLevel::new(67_020.0, 2.0),
                PriceLevel::new(67_030.0, 4.0),
            ],
            1_700_000_000_000,
        )
    }

    #[test]
    fn best_bid_and_ask() {
        let book = make_book();
        assert!((book.best_bid().unwrap() - 67_000.0).abs() < f64::EPSILON);
        assert!((book.best_ask().unwrap() - 67_010.0).abs() < f64::EPSILON);
    }

    #[test]
    fn spread_and_mid_price() {
        let book = make_book();
        assert!((book.spread().unwrap() - 10.0).abs() < f64::EPSILON);
        assert!((book.mid_price().unwrap() - 67_005.0).abs() < f64::EPSILON);
    }

    #[test]
    fn top_n_depth() {
        let book = make_book();
        assert_eq!(book.top_bids(2).len(), 2);
        assert_eq!(book.top_asks(10).len(), 3); // capped at available depth
    }

    #[test]
    fn max_quantity_normalisation() {
        let book = make_book();
        // Max bid qty = 5.0, max ask qty = 4.0 → max = 5.0
        assert!((book.max_quantity(3) - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn raw_order_book_converts_to_order_book() {
        let raw = RawOrderBook {
            msg_type: "orderbook".to_string(),
            exchange: "binance".to_string(),
            symbol: "BTC/USDT".to_string(),
            bids: vec![[67_000.0, 1.5], [66_990.0, 3.0]],
            asks: vec![[67_010.0, 1.0], [67_020.0, 2.0]],
            timestamp_ms: 1_700_000_000_000,
        };
        let book = OrderBook::from(raw);
        assert_eq!(book.bids.len(), 2);
        assert_eq!(book.asks.len(), 2);
        assert!((book.bids[0].price - 67_000.0).abs() < f64::EPSILON);
        assert!((book.asks[0].quantity - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_book_returns_none() {
        let book = OrderBook::new("binance", "BTC/USDT", vec![], vec![], 0);
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
        assert!(book.spread().is_none());
        assert!(book.mid_price().is_none());
    }

    #[test]
    fn price_level_notional() {
        let level = PriceLevel::new(50_000.0, 0.5);
        assert!((level.notional() - 25_000.0).abs() < f64::EPSILON);
    }
}
