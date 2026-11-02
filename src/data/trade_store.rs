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
pub enum TradingMode {
    Paper,
    Live,
}

impl TradingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            TradingMode::Paper => "PAPER",
            TradingMode::Live => "LIVE",
        }
    }
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
    pub mode: TradingMode,
    pub initial_mark: Option<f64>,
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
    pub mode: TradingMode,
}

pub struct TradeStore {
    pub paper_balance: f64,
    pub live_balance: f64,
    pub active_mode: TradingMode,
    pub positions: Vec<DeskPosition>,
    pub orders: Vec<TradeOrder>,
    pub mark_prices: HashMap<String, f64>,
    /// Canonical symbols whose mark price is currently driven by a live tick
    /// feed, with the timestamp of the last feed update. Charts only stand in as
    /// the price source once the feed has gone quiet (e.g. desk panel closed).
    pub feed_mark_at: HashMap<String, std::time::Instant>,
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
            live_balance: 10_000.0,
            active_mode: TradingMode::Paper,
            positions: Vec::new(),
            orders: Vec::new(),
            mark_prices: HashMap::new(),
            feed_mark_at: HashMap::new(),
            version: 0,
        }
    }

    pub fn balance(&self, mode: TradingMode) -> f64 {
        match mode {
            TradingMode::Paper => self.paper_balance,
            TradingMode::Live => self.live_balance,
        }
    }

    pub fn active_balance(&self) -> f64 {
        self.balance(self.active_mode)
    }

    /// Replace the live cash balance with the real account balance fetched
    /// from the selected exchange. The paper ledger is left untouched.
    pub fn set_live_balance(&mut self, balance: f64) {
        if !balance.is_finite() || balance < 0.0 {
            return;
        }
        if (self.live_balance - balance).abs() <= f64::EPSILON {
            return;
        }
        self.live_balance = balance;
        self.version = self.version.wrapping_add(1);
    }

    /// Record the latest live market price for `symbol`.
    ///
    /// Mark prices are stored under a single canonical key per logical symbol so
    /// that callers using different notations ("BTC/USDT", "BTCUSDT",
    /// "BINANCE:BTCUSDT") always observe the *same* latest price. Without this,
    /// two writers (chart candle close vs. tick feed) could leave duplicate
    /// entries and PnL would jump between stale values.
    pub fn set_mark_price(&mut self, symbol: &str, price: f64) {
        if !price.is_finite() || price <= 0.0 {
            return;
        }
        let key = normalize_symbol(symbol);
        if key.is_empty() {
            return;
        }
        let prev = self
            .mark_prices
            .iter()
            .find(|(s, _)| normalize_symbol(s) == key)
            .map(|(_, p)| *p);
        // This symbol is driven by a real feed; refresh the heartbeat even when the
        // price is unchanged so charts never take over mid-session.
        self.feed_mark_at
            .insert(key.clone(), std::time::Instant::now());
        if prev.map(|p| (p - price).abs() <= f64::EPSILON).unwrap_or(false) {
            return;
        }
        // Drop any stale duplicate keys for this logical symbol.
        self.mark_prices
            .retain(|s, _| normalize_symbol(s) != key);
        self.mark_prices.insert(key, price);
        self.check_open_orders(symbol, price);
        self.version = self.version.wrapping_add(1);
    }

    /// Update the mark price from chart data (candle close) as a fallback.
    ///
    /// Ignored while the symbol receives live ticks, so chart rendering can never
    /// fight the feed and make PnL flicker between two prices. Once the feed goes
    /// quiet (desk panel closed), the chart takes over so marks stay fresh.
    pub fn sync_chart_mark_price(&mut self, symbol: &str, price: f64) -> bool {
        if !price.is_finite() || price <= 0.0 {
            return false;
        }
        let key = normalize_symbol(symbol);
        if key.is_empty() {
            return false;
        }
        let feed_active = self
            .feed_mark_at
            .get(&key)
            .map(|t| t.elapsed() < std::time::Duration::from_secs(2))
            .unwrap_or(false);
        if feed_active {
            return false;
        }
        let prev = self
            .mark_prices
            .iter()
            .find(|(s, _)| normalize_symbol(s) == key)
            .map(|(_, p)| *p);
        if prev.map(|p| (p - price).abs() <= f64::EPSILON).unwrap_or(false) {
            return false;
        }
        self.mark_prices
            .retain(|s, _| normalize_symbol(s) != key);
        self.mark_prices.insert(key, price);
        self.check_open_orders(symbol, price);
        self.version = self.version.wrapping_add(1);
        true
    }

    pub fn get_mark_price(&self, symbol: &str) -> Option<f64> {
        let key = normalize_symbol(symbol);
        self.mark_prices
            .get(&key)
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

    /// Unrealized P&L for a single position, using the freshest mark price.
    ///
    /// This is the single source of truth used by the trade desk, the positions
    /// panel, and the chart overlays so every module shows identical numbers.
    pub fn position_unrealized_pnl(&self, pos: &DeskPosition) -> f64 {
        let mark = self
            .get_mark_price(&pos.symbol)
            .unwrap_or(pos.entry_price);
        match pos.side {
            TradeSide::Long => (mark - pos.entry_price) * pos.quantity,
            TradeSide::Short => (pos.entry_price - mark) * pos.quantity,
        }
    }

    /// Mark price used for a position's P&L / display.
    pub fn position_mark_price(&self, pos: &DeskPosition) -> f64 {
        self.get_mark_price(&pos.symbol)
            .unwrap_or(pos.entry_price)
    }

    pub fn execute_order(
        &mut self,
        mode: TradingMode,
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
        let available = self.balance(mode);
        if margin > available {
            return Err(format!(
                "Insufficient {} funds: available ${:.2}, required ${:.2}",
                mode.as_str().to_lowercase(),
                available,
                margin
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
        // Market orders execute immediately. Limit orders always enter as OPEN orders
        // and only fill when the live market price moves to or across the limit price.
        let immediate_fill = is_market;

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
            mode,
            initial_mark: mark_opt,
        };

        // Deduct margin from the respective balance (reserved for order/position)
        match mode {
            TradingMode::Paper => self.paper_balance -= margin,
            TradingMode::Live => self.live_balance -= margin,
        }
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
                mode,
            };
            self.positions.push(position);
            // Note: do not overwrite mark_prices here; mark prices are driven solely by live feeds.
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

        match order.mode {
            TradingMode::Paper => self.paper_balance += order.margin,
            TradingMode::Live => self.live_balance += order.margin,
        }
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

            let should_fill = match order.initial_mark {
                Some(init_mark) => {
                    if (order.price - init_mark).abs() < 1e-6 {
                        match order.side {
                            TradeSide::Long => current_price <= order.price,
                            TradeSide::Short => current_price >= order.price,
                        }
                    } else if order.price > init_mark {
                        // Order price set above the market price at placement (e.g. breakout buy or high sell):
                        // Only fills when live feed price rises to or crosses the limit price.
                        current_price >= order.price
                    } else {
                        // Order price set below the market price at placement (e.g. dip buy or low sell):
                        // Only fills when live feed price drops to or crosses the limit price.
                        current_price <= order.price
                    }
                }
                None => match order.side {
                    TradeSide::Long => current_price <= order.price,
                    TradeSide::Short => current_price >= order.price,
                },
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
                    mode: order.mode,
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

        let payout = (pos.margin + pnl).max(0.0);
        match pos.mode {
            TradingMode::Paper => self.paper_balance += payout,
            TradingMode::Live => self.live_balance += payout,
        }

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
            mode: pos.mode,
            initial_mark: Some(mark),
        };

        self.orders.insert(0, close_order.clone());
        self.version = self.version.wrapping_add(1);
        Ok(close_order)
    }

    pub fn reset_paper(&mut self) {
        self.paper_balance = 100_000.0;
        self.positions.retain(|p| p.mode != TradingMode::Paper);
        self.orders.retain(|o| o.mode != TradingMode::Paper);
        self.version = self.version.wrapping_add(1);
    }

    pub fn reset_live(&mut self) {
        self.live_balance = 10_000.0;
        self.positions.retain(|p| p.mode != TradingMode::Live);
        self.orders.retain(|o| o.mode != TradingMode::Live);
        self.version = self.version.wrapping_add(1);
    }

    /// Clear simulated live positions/orders without touching the live cash
    /// balance, which is sourced from the real exchange account.
    pub fn reset_live_positions(&mut self) {
        self.positions.retain(|p| p.mode != TradingMode::Live);
        self.orders.retain(|o| o.mode != TradingMode::Live);
        self.version = self.version.wrapping_add(1);
    }

    pub fn reset(&mut self) {
        self.paper_balance = 100_000.0;
        self.live_balance = 10_000.0;
        self.positions.clear();
        self.orders.clear();
        self.version = self.version.wrapping_add(1);
    }

    pub fn clear_orders(&mut self) {
        self.orders.clear();
        self.version = self.version.wrapping_add(1);
    }

    pub fn clear_orders_for_mode(&mut self, mode: TradingMode) {
        self.orders.retain(|o| o.mode != mode);
        self.version = self.version.wrapping_add(1);
    }

    pub fn deployed_margin(&self, mode: TradingMode) -> f64 {
        self.positions.iter().filter(|p| p.mode == mode).map(|p| p.margin).sum()
    }

    pub fn deployed_margin_all(&self) -> f64 {
        self.positions.iter().map(|p| p.margin).sum()
    }

    pub fn reserved_order_margin(&self, mode: TradingMode) -> f64 {
        self.orders
            .iter()
            .filter(|o| o.status == "OPEN" && o.mode == mode)
            .map(|o| o.margin)
            .sum()
    }

    pub fn reserved_order_margin_all(&self) -> f64 {
        self.orders
            .iter()
            .filter(|o| o.status == "OPEN")
            .map(|o| o.margin)
            .sum()
    }

    pub fn total_unrealized_pnl(&self, mode: TradingMode) -> f64 {
        self.positions
            .iter()
            .filter(|pos| pos.mode == mode)
            .map(|pos| self.position_unrealized_pnl(pos))
            .sum()
    }

    pub fn total_unrealized_pnl_all(&self) -> f64 {
        self.positions
            .iter()
            .map(|pos| self.position_unrealized_pnl(pos))
            .sum()
    }

    pub fn total_equity(&self, mode: TradingMode) -> f64 {
        self.balance(mode)
            + self.deployed_margin(mode)
            + self.reserved_order_margin(mode)
            + self.total_unrealized_pnl(mode)
    }

    pub fn total_equity_all(&self) -> f64 {
        self.paper_balance
            + self.live_balance
            + self.deployed_margin_all()
            + self.reserved_order_margin_all()
            + self.total_unrealized_pnl_all()
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
                TradingMode::Paper,
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
        assert_eq!(order.mode, TradingMode::Paper);
        assert_eq!(store.paper_balance, 99_000.0);
        assert_eq!(store.deployed_margin(TradingMode::Paper), 1_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.orders.len(), 1);

        // Price moves up to 55,000 (+5,000 per BTC -> +$1,000 PnL for 0.2 BTC)
        store.set_mark_price("BTC/USDT", 55_000.0);
        assert_eq!(store.total_unrealized_pnl(TradingMode::Paper), 1_000.0);
        assert_eq!(store.total_equity(TradingMode::Paper), 101_000.0); // 99k cash + 1k margin + 1k pnl

        // Close position
        let pos_id = store.positions[0].id.clone();
        let close_order = store
            .close_position(&pos_id, Some(55_000.0))
            .expect("close should succeed");

        assert_eq!(close_order.status, "CLOSED");
        assert_eq!(close_order.mode, TradingMode::Paper);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders.len(), 2);
        assert_eq!(store.paper_balance, 101_000.0); // 99k + (1k margin + 1k pnl)
    }

    #[test]
    fn test_trade_store_live_execution_isolation() {
        let mut store = TradeStore::new();
        assert_eq!(store.paper_balance, 100_000.0);
        assert_eq!(store.live_balance, 10_000.0);

        // Execute live order with $2,000 margin
        let live_order = store
            .execute_order(
                TradingMode::Live,
                "ETH/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Market,
                3_000.0,
                2_000.0,
                5,
            )
            .expect("live order should succeed");

        assert_eq!(live_order.status, "FILLED");
        assert_eq!(live_order.mode, TradingMode::Live);
        // Live balance deducted, paper balance untouched
        assert_eq!(store.live_balance, 8_000.0);
        assert_eq!(store.paper_balance, 100_000.0);
        assert_eq!(store.deployed_margin(TradingMode::Live), 2_000.0);
        assert_eq!(store.deployed_margin(TradingMode::Paper), 0.0);

        // Close live position with profit
        let pos_id = store.positions[0].id.clone();
        let close_order = store
            .close_position(&pos_id, Some(3_300.0))
            .expect("live close should succeed");

        assert_eq!(close_order.mode, TradingMode::Live);
        assert_eq!(store.paper_balance, 100_000.0); // Completely isolated
        assert!(store.live_balance > 10_900.0); // Payout returned to live balance
    }

    #[test]
    fn test_trade_store_insufficient_funds() {
        let mut store = TradeStore::new();
        let res_paper = store.execute_order(
            TradingMode::Paper,
            "ETH/USDT".to_string(),
            TradeSide::Long,
            TradeOrderType::Market,
            2_500.0,
            150_000.0, // more than 100k balance
            5,
        );
        assert!(res_paper.is_err());
        assert!(res_paper.unwrap_err().contains("paper funds"));

        let res_live = store.execute_order(
            TradingMode::Live,
            "ETH/USDT".to_string(),
            TradeSide::Long,
            TradeOrderType::Market,
            2_500.0,
            25_000.0, // more than 10k live balance
            5,
        );
        assert!(res_live.is_err());
        assert!(res_live.unwrap_err().contains("live funds"));
    }

    #[test]
    fn test_trade_store_reset_isolation() {
        let mut store = TradeStore::new();
        let _ = store.execute_order(
            TradingMode::Paper,
            "SOL/USDT".to_string(),
            TradeSide::Short,
            TradeOrderType::Market,
            150.0,
            500.0,
            10,
        );
        let _ = store.execute_order(
            TradingMode::Live,
            "BTC/USDT".to_string(),
            TradeSide::Long,
            TradeOrderType::Market,
            50_000.0,
            1_000.0,
            5,
        );
        assert_eq!(store.positions.len(), 2);

        // Reset paper only
        store.reset_paper();
        assert_eq!(store.paper_balance, 100_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.positions[0].mode, TradingMode::Live);
        assert_eq!(store.live_balance, 9_000.0);

        // Reset live only
        store.reset_live();
        assert_eq!(store.live_balance, 10_000.0);
        assert_eq!(store.positions.len(), 0);
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
                TradingMode::Paper,
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Limit,
                48_000.0,
                1_000.0,
                5,
            )
            .expect("limit order should succeed");

        assert_eq!(order.status, "OPEN");
        assert_eq!(order.mode, TradingMode::Paper);
        assert_eq!(store.paper_balance, 99_000.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.reserved_order_margin(TradingMode::Paper), 1_000.0);
        assert_eq!(store.total_equity(TradingMode::Paper), 100_000.0);

        // Cancel order -> refunds margin to paper balance
        let cancelled = store.cancel_order(&order.id).expect("cancel should succeed");
        assert_eq!(cancelled.status, "CANCELLED");
        assert_eq!(store.paper_balance, 100_000.0);
        assert_eq!(store.reserved_order_margin(TradingMode::Paper), 0.0);
    }

    #[test]
    fn test_open_limit_order_fill_on_price_move() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 50_000.0);

        // Place live limit buy @ 48,000
        let order = store
            .execute_order(
                TradingMode::Live,
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Limit,
                48_000.0,
                1_000.0,
                5,
            )
            .expect("limit order should succeed");
        assert_eq!(order.status, "OPEN");
        assert_eq!(order.mode, TradingMode::Live);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.live_balance, 9_000.0);

        // Price drops to 48,000 -> order should fill and create position with Live mode
        store.set_mark_price("BTC/USDT", 48_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.positions[0].entry_price, 48_000.0);
        assert_eq!(store.positions[0].mode, TradingMode::Live);
        assert_eq!(store.orders[0].status, "FILLED");
        assert_eq!(store.deployed_margin(TradingMode::Live), 1_000.0);
        assert_eq!(store.reserved_order_margin(TradingMode::Live), 0.0);
    }

    #[test]
    fn test_limit_order_does_not_execute_at_market_price_when_far() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 50_000.0);

        // 1. Long limit order set far below current price (40,000 vs 50,000)
        let buy_order = store
            .execute_order(
                TradingMode::Paper,
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Limit,
                40_000.0,
                500.0,
                10,
            )
            .expect("limit buy order should succeed");

        assert_eq!(buy_order.status, "OPEN");
        assert_eq!(buy_order.price, 40_000.0);
        // Positions must not contain this order
        assert_eq!(store.positions.len(), 0);
        // Mark price should remain untouched at 50,000, NOT overwritten by 40,000
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(50_000.0));

        // 2. Short limit order set far above current price (60,000 vs 50,000)
        let sell_order = store
            .execute_order(
                TradingMode::Live,
                "BTC/USDT".to_string(),
                TradeSide::Short,
                TradeOrderType::Limit,
                60_000.0,
                500.0,
                10,
            )
            .expect("limit sell order should succeed");

        assert_eq!(sell_order.status, "OPEN");
        assert_eq!(sell_order.price, 60_000.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(50_000.0));

        // Both orders remain open at their exact limit prices, not market price
        let open_orders = store.open_orders_for_symbol("BTC/USDT");
        assert_eq!(open_orders.len(), 2);
        assert_eq!(open_orders[0].price, 60_000.0);
        assert_eq!(open_orders[1].price, 40_000.0);
    }

    #[test]
    fn test_limit_buy_above_market_price_remains_open_until_reached() {
        let mut store = TradeStore::new();
        // Current market price of BTC is 77,000
        store.set_mark_price("BTC/USDT", 77_000.0);

        // Place limit buy @ 80,000 (above current market price of 77,000)
        let buy_order = store
            .execute_order(
                TradingMode::Paper,
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Limit,
                80_000.0,
                1_000.0,
                10,
            )
            .expect("limit buy order should succeed");

        // MUST NOT fill immediately at market price of 77,000!
        assert_eq!(buy_order.status, "OPEN");
        assert_eq!(buy_order.price, 80_000.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.paper_balance, 99_000.0); // 1k margin reserved

        // Price moves around below 80,000 (77,100, 78,500, 79,900) -> order must REMAIN open
        store.set_mark_price("BTC/USDT", 77_100.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders[0].status, "OPEN");

        store.set_mark_price("BTC/USDT", 78_500.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders[0].status, "OPEN");

        store.set_mark_price("BTC/USDT", 79_900.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders[0].status, "OPEN");

        // Price reaches 80,000 -> order fills at 80,000
        store.set_mark_price("BTC/USDT", 80_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.positions[0].entry_price, 80_000.0);
        assert_eq!(store.orders[0].status, "FILLED");
    }

    #[test]
    fn test_limit_sell_below_market_price_remains_open_until_reached() {
        let mut store = TradeStore::new();
        // Current market price of BTC is 77,000
        store.set_mark_price("BTC/USDT", 77_000.0);

        // Place limit sell @ 70,000 (below current market price of 77,000)
        let sell_order = store
            .execute_order(
                TradingMode::Paper,
                "BTC/USDT".to_string(),
                TradeSide::Short,
                TradeOrderType::Limit,
                70_000.0,
                1_000.0,
                10,
            )
            .expect("limit sell order should succeed");

        // MUST NOT fill immediately at market price of 77,000!
        assert_eq!(sell_order.status, "OPEN");
        assert_eq!(sell_order.price, 70_000.0);
        assert_eq!(store.positions.len(), 0);

        // Price moves around above 70,000 -> order must REMAIN open
        store.set_mark_price("BTC/USDT", 76_500.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders[0].status, "OPEN");

        store.set_mark_price("BTC/USDT", 71_000.0);
        assert_eq!(store.positions.len(), 0);
        assert_eq!(store.orders[0].status, "OPEN");

        // Price drops to 70,000 -> order fills at 70,000
        store.set_mark_price("BTC/USDT", 70_000.0);
        assert_eq!(store.positions.len(), 1);
        assert_eq!(store.positions[0].entry_price, 70_000.0);
        assert_eq!(store.orders[0].status, "FILLED");
    }

    #[test]
    fn test_mark_price_keeps_single_canonical_entry() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 77_000.0);
        store.set_mark_price("BINANCE:BTCUSDT", 77_500.0);
        store.set_mark_price("btcusdt", 78_000.0);

        // Only one logical price may exist for the symbol.
        assert_eq!(store.mark_prices.len(), 1);
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(78_000.0));
        assert_eq!(store.get_mark_price("BTCUSDT"), Some(78_000.0));
        assert_eq!(store.get_mark_price("BINANCE:BTCUSDT"), Some(78_000.0));
    }

    #[test]
    fn test_pnl_is_consistent_across_aggregates_and_rows() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 50_000.0);
        let _ = store
            .execute_order(
                TradingMode::Paper,
                "BTC/USDT".to_string(),
                TradeSide::Long,
                TradeOrderType::Market,
                50_000.0,
                1_000.0,
                10,
            )
            .expect("order should succeed");
        let _ = store
            .execute_order(
                TradingMode::Live,
                "ETH/USDT".to_string(),
                TradeSide::Short,
                TradeOrderType::Market,
                3_000.0,
                500.0,
                5,
            )
            .expect("order should succeed");

        store.set_mark_price("BTC/USDT", 55_000.0);
        store.set_mark_price("ETH/USDT", 2_700.0);

        let row_sum: f64 = store
            .positions
            .iter()
            .map(|p| store.position_unrealized_pnl(p))
            .sum();
        assert!((store.total_unrealized_pnl_all() - row_sum).abs() < 1e-9);
        assert!(
            (store.total_unrealized_pnl(TradingMode::Paper)
                + store.total_unrealized_pnl(TradingMode::Live)
                - row_sum)
                .abs()
                < 1e-9
        );

        // Row PnL matches the exact formula for each side.
        let btc = store
            .positions
            .iter()
            .find(|p| p.symbol == "BTC/USDT")
            .unwrap();
        assert!((store.position_unrealized_pnl(btc) - 1_000.0).abs() < 1e-9); // 0.2 BTC * 5,000
        let eth = store
            .positions
            .iter()
            .find(|p| p.symbol == "ETH/USDT")
            .unwrap();
        assert!((store.position_unrealized_pnl(eth) - 250.0).abs() < 1e-6); // 0.8333 ETH * 300
        assert_eq!(store.position_mark_price(btc), 55_000.0);
        assert_eq!(store.position_mark_price(eth), 2_700.0);
    }

    #[test]
    fn test_chart_price_cannot_override_live_feed_mark() {
        let mut store = TradeStore::new();
        store.set_mark_price("BTC/USDT", 77_000.0);

        // Chart tries to publish a stale candle close while the feed is live.
        assert!(!store.sync_chart_mark_price("BTC/USDT", 76_000.0));
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(77_000.0));

        // Feed stays authoritative even through many chart frames.
        for close in [76_100.0, 76_900.0, 77_400.0] {
            assert!(!store.sync_chart_mark_price("BTC/USDT", close));
        }
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(77_000.0));

        // A newer tick still wins.
        store.set_mark_price("BTC/USDT", 77_250.0);
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(77_250.0));

        // Once the feed has been quiet for a while, the chart may take over.
        store.feed_mark_at.insert(
            normalize_symbol("BTC/USDT"),
            std::time::Instant::now() - std::time::Duration::from_secs(5),
        );
        assert!(store.sync_chart_mark_price("BTC/USDT", 77_300.0));
        assert_eq!(store.get_mark_price("BTC/USDT"), Some(77_300.0));
    }
}
