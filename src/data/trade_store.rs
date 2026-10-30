use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub fn normalize_symbol(s: &str) -> String {
    let s = s.trim();
    let after_colon = s.split(':').last().unwrap_or(s);
    after_colon
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

pub fn symbol_matches(a: &str, b: &str) -> bool {
    let na = normalize_symbol(a);
    let nb = normalize_symbol(b);
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    na == nb
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TradeSide {
    Long,
    Short,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TradeOrderType {
    Market,
    Limit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TradeOrder {
    pub id: String,
    pub timestamp: u64,
    pub symbol: String,
    pub side: TradeSide,
    pub order_type: TradeOrderType,
    pub price: f64,
    pub quantity: f64,
    pub notional: f64,
    pub leverage: u32,
    pub margin: f64,
    pub status: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeskPosition {
    pub id: String,
    pub symbol: String,
    pub side: TradeSide,
    pub entry_price: f64,
    pub quantity: f64,
    pub margin: f64,
    pub leverage: u32,
    pub timestamp: u64,
}

pub struct TradeStore {
    pub paper_balance: f64,
    pub positions: Vec<DeskPosition>,
    pub orders: Vec<TradeOrder>,
    pub mark_prices: HashMap<String, f64>,
    pub version: u64,
}

impl Default for TradeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TradeStore {
    pub fn global() -> &'static Mutex<TradeStore> {
        static STORE: OnceLock<Mutex<TradeStore>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(TradeStore::new()))
    }

    pub fn new() -> Self {
        Self {
            paper_balance: 100_000.0,
            positions: Vec::new(),
            orders: Vec::new(),
            mark_prices: HashMap::new(),
            version: 0,
        }
    }

    pub fn set_mark_price(&mut self, symbol: &str, price: f64) {
        if price.is_finite() && price > 0.0 {
            let prev = self.mark_prices.get(symbol).copied();
            if prev != Some(price) {
                self.mark_prices.insert(symbol.to_string(), price);
                self.check_open_orders(symbol, price);
                self.version = self.version.wrapping_add(1);
            }
        }
    }

    pub fn get_mark_price(&self, symbol: &str) -> Option<f64> {
        self.mark_prices
            .get(symbol)
            .copied()
            .or_else(|| {
                self.mark_prices.iter().find_map(|(s, &p)| {
                    if symbol_matches(s, symbol) {
                        Some(p)
                    } else {
                        None
                    }
                })
            })
    }

    pub fn execute_order(
        &mut self,
        symbol: String,
        side: TradeSide,
        order_type: TradeOrderType,
        price: f64,
        margin: f64,
        leverage: u32,
    ) -> Result<TradeOrder, String> {
        if margin <= 0.0 {
            return Err("Margin amount must be greater than 0.".to_string());
        }
        if margin > self.paper_balance {
            return Err(format!(
                "Insufficient funds: available ${:.2}, required ${:.2}",
                self.paper_balance, margin
            ));
        }
        if price <= 0.0 || !price.is_finite() {
            return Err("Invalid execution price.".to_string());
        }

        let notional = margin * leverage as f64;
        let quantity = notional / price;
        let now_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();

        let is_market = order_type == TradeOrderType::Market;
        let mark_opt = self.get_mark_price(&symbol);
        let immediate_fill = if is_market {
            true
        } else if let Some(mark) = mark_opt {
            match side {
                TradeSide::Long => price >= mark,
                TradeSide::Short => price <= mark,
            }
        } else {
            false
        };

        let status = if immediate_fill { "FILLED" } else { "OPEN" };

        let order = TradeOrder {
            id: id.clone(),
            timestamp: now_sec,
            symbol: symbol.clone(),
            side,
            order_type,
            price,
            quantity,
            notional,
            leverage,
            margin,
            status,
        };

        // Deduct margin from paper balance (reserved for order/position)
        self.paper_balance -= margin;
        self.orders.insert(0, order.clone());

        if immediate_fill {
            let position = DeskPosition {
                id,
                symbol: symbol.clone(),
                side,
                entry_price: price,
                quantity,
                margin,
                leverage,
                timestamp: now_sec,
            };
            self.positions.push(position);
            self.mark_prices.insert(symbol, price);
        }

        self.version = self.version.wrapping_add(1);

        Ok(order)
    }

    pub fn cancel_order(&mut self, order_id: &str) -> Result<TradeOrder, String> {
        let order = self
            .orders
            .iter_mut()
            .find(|o| o.id == order_id && o.status == "OPEN")
            .ok_or_else(|| "Open order not found".to_string())?;

        self.paper_balance += order.margin;
        order.status = "CANCELLED";
        let cancelled = order.clone();
        self.version = self.version.wrapping_add(1);
        Ok(cancelled)
    }

    pub fn check_open_orders(&mut self, symbol: &str, current_price: f64) {
        if !current_price.is_finite() || current_price <= 0.0 {
            return;
        }

        let now_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut newly_filled = Vec::new();

        for order in self.orders.iter_mut() {
            if order.status != "OPEN" || !symbol_matches(&order.symbol, symbol) {
                continue;
            }

            let should_fill = match order.side {
                TradeSide::Long => current_price <= order.price,
                TradeSide::Short => current_price >= order.price,
            };

            if should_fill {
                order.status = "FILLED";
                newly_filled.push(DeskPosition {
                    id: order.id.clone(),
                    symbol: order.symbol.clone(),
                    side: order.side,
                    entry_price: order.price,
                    quantity: order.quantity,
                    margin: order.margin,
                    leverage: order.leverage,
                    timestamp: now_sec,
                });
            }
        }

        if !newly_filled.is_empty() {
            self.positions.extend(newly_filled);
            self.version = self.version.wrapping_add(1);
        }
    }

    pub fn open_orders_for_symbol(&self, symbol: &str) -> Vec<TradeOrder> {
        self.orders
            .iter()
            .filter(|o| o.status == "OPEN" && symbol_matches(&o.symbol, symbol))
            .cloned()
            .collect()
    }

    pub fn positions_for_symbol(&self, symbol: &str) -> Vec<DeskPosition> {
        self.positions
            .iter()
            .filter(|p| symbol_matches(&p.symbol, symbol))
            .cloned()
            .collect()
    }

    pub fn close_position(&mut self, pos_id: &str, current_price: Option<f64>) -> Result<TradeOrder, String> {
        let pos_idx = self
            .positions
            .iter()
            .position(|p| p.id == pos_id)
            .ok_or_else(|| "Position not found".to_string())?;

        let pos = self.positions.remove(pos_idx);
        let mark = current_price
            .or_else(|| self.get_mark_price(&pos.symbol))
            .unwrap_or(pos.entry_price);

        let pnl = match pos.side {
            TradeSide::Long => (mark - pos.entry_price) * pos.quantity,
            TradeSide::Short => (pos.entry_price - mark) * pos.quantity,
        };

        self.paper_balance += (pos.margin + pnl).max(0.0);

        let now_sec = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let close_order = TradeOrder {
            id: uuid::Uuid::new_v4().to_string()[..8].to_string(),
            timestamp: now_sec,
            symbol: pos.symbol.clone(),
            side: match pos.side {
                TradeSide::Long => TradeSide::Short,
                TradeSide::Short => TradeSide::Long,
            },
            order_type: TradeOrderType::Market,
            price: mark,
            quantity: pos.quantity,
            notional: pos.quantity * mark,
            leverage: pos.leverage,
            margin: 0.0,
            status: "CLOSED",
        };

        self.orders.insert(0, close_order.clone());
        self.version = self.version.wrapping_add(1);
        Ok(close_order)
    }

    pub fn reset(&mut self) {
        self.paper_balance = 100_000.0;
        self.positions.clear();
        self.orders.clear();
        self.version = self.version.wrapping_add(1);
    }

    pub fn clear_orders(&mut self) {
        self.orders.clear();
        self.version = self.version.wrapping_add(1);
    }

    pub fn deployed_margin(&self) -> f64 {
        self.positions.iter().map(|p| p.margin).sum()
    }

    pub fn reserved_order_margin(&self) -> f64 {
        self.orders
            .iter()
            .filter(|o| o.status == "OPEN")
            .map(|o| o.margin)
            .sum()
    }

    pub fn total_unrealized_pnl(&self) -> f64 {
        self.positions
            .iter()
            .map(|pos| {
                let mark = self.get_mark_price(&pos.symbol).unwrap_or(pos.entry_price);
                match pos.side {
                    TradeSide::Long => (mark - pos.entry_price) * pos.quantity,
                    TradeSide::Short => (pos.entry_price - mark) * pos.quantity,
                }
            })
            .sum()
    }

    pub fn total_equity(&self) -> f64 {
        self.paper_balance
            + self.deployed_margin()
            + self.reserved_order_margin()
            + self.total_unrealized_pnl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trade_store_execution_and_close() {
        let mut store = TradeStore::new();
        assert_eq!(store.paper_balance, 100_000.0);

        // Execute 1,000 margin at 10x leverage on BTC @ $50,000 -> $10,000 notional, 0.2 BTC
        let order = store
            .execute_order(
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Market,
                50_000.0,
                1_000.0,
                10,
            )
            .expect("order should succeed");

        assert_eq!(order.quantity, 0.2);
        assert_eq!(order.notional, 10_000.0);
        assert_eq!(store.paper_balance, 99_000.0);
        assert_eq!(store.deployed_margin(), 1_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.orders.len(), 1);

        // Price moves up to 55,000 (+5,000 per BTC -> +$1,000 PnL for 0.2 BTC)
        store.set_mark_price("BTC/USDT", 55_000.0);
        assert_eq!(store.total_unrealized_pnl(), 1_000.0);
        assert_eq!(store.total_equity(), 101_000.0); // 99k cash + 1k margin + 1k pnl

        // Close position
        let pos_id = store.positions[0].id.clone();
        let close_order = store
            .close_position(&pos_id, Some(55_000.0))
            .expect("close should succeed");

        assert_eq!(close_order.status, "CLOSED");
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders.len(), 2);
        assert_eq!(store.paper_balance, 101_000.0); // 99k + (1k margin + 1k pnl)
    }

    #[test]
    fn test_trade_store_insufficient_funds() {
        let mut store = TradeStore::new();
        let res = store.execute_order(
            "ETH/USDT".to_string(),
            TradeSide::Long,
            TradeOrderType::Market,
            2_500.0,
            150_000.0, // more than 100k balance
            5,
        );
        assert!(res.is_err());
    }

    #[test]
    fn test_trade_store_reset() {
        let mut store = TradeStore::new();
        let _ = store.execute_order(
            "SOL/USDT".to_string(),
            TradeSide::Short,
            TradeOrderType::Market,
            150.0,
            500.0,
            10,
        );
        assert_eq!(store.positions.len(), 1);
        store.reset();
        assert_eq!(store.paper_balance, 100_000.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders.len(), 0);
    }

    #[test]
    fn test_symbol_matches() {
        assert!(symbol_matches("BTC/USDT", "BTCUSDT"));
        assert!(symbol_matches("BINANCE:BTCUSDT", "BTC/USDT"));
        assert!(symbol_matches("binance:btc-usdt", "BTCUSDT"));
        assert!(symbol_matches("BTC_USDT", "BINANCE:BTCUSDT"));
        assert!(!symbol_matches("BTC/USDT", "ETH/USDT"));
    }

    #[test]
    fn test_open_limit_order_and_cancel() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 50_000.0);

        // Place limit buy below mark price -> should be OPEN
        let order = store
            .execute_order(
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Limit,
                48_000.0,
                1_000.0,
                5,
            )
            .expect("limit order should succeed");

        assert_eq!(order.status, "OPEN");
        assert_eq!(store.paper_balance, 99_000.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.reserved_order_margin(), 1_000.0);
        assert_eq!(store.total_equity(), 100_000.0);

        // Cancel order -> refunds margin
        let cancelled = store.cancel_order(&order.id).expect("cancel should succeed");
        assert_eq!(cancelled.status, "CANCELLED");
        assert_eq!(store.paper_balance, 100_000.0);
        assert_eq!(store.reserved_order_margin(), 0.0);
    }

    #[test]
    fn test_open_limit_order_fill_on_price_move() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 50_000.0);

        // Place limit buy @ 48,000
        let order = store
            .execute_order(
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Limit,
                48_000.0,
                1_000.0,
                5,
            )
            .expect("limit order should succeed");
        assert_eq!(order.status, "OPEN");
        assert_eq!(store.positions.len(), 0);

        // Price drops to 48,000 -> order should fill and create position
        store.set_mark_price("BTC/USDT", 48_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.positions[0].entry_price, 48_000.0);
        assert_eq!(store.orders[0].status, "FILLED");
        assert_eq!(store.deployed_margin(), 1_000.0);
        assert_eq!(store.reserved_order_margin(), 0.0);
    }
}
