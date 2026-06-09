use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

pub(crate) const ACCOUNT_VERSION: u32 = 1;

pub(crate) fn default_starting_balance() -> Decimal {
    Decimal::from(10_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum TradeSide {
    Buy,
    Sell,
}

impl std::fmt::Display for TradeSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Buy => write!(f, "BUY"),
            Self::Sell => write!(f, "SELL"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OrderKind {
    Market,
    Limit,
}

impl std::fmt::Display for OrderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Market => write!(f, "market"),
            Self::Limit => write!(f, "limit"),
        }
    }
}

/// Market metadata attached to positions and trades so output is readable
/// without extra lookups.
#[derive(Clone, Debug, Default)]
pub(crate) struct MarketMeta {
    pub question: String,
    pub outcome: String,
}

/// Best bid/ask for a token, derived from the live order book.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Quote {
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Position {
    pub token_id: String,
    pub question: String,
    pub outcome: String,
    /// Shares held (excludes nothing; shares reserved by open sell orders
    /// remain here until the order fills).
    pub size: Decimal,
    pub avg_price: Decimal,
    /// Realized PnL accumulated by sells of this position.
    pub realized_pnl: Decimal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Trade {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    pub token_id: String,
    pub question: String,
    pub outcome: String,
    pub side: TradeSide,
    pub kind: OrderKind,
    pub size: Decimal,
    pub price: Decimal,
    /// `size * price` in pUSD.
    pub notional: Decimal,
    /// Set on sells: `(price - avg_entry) * size`.
    pub realized_pnl: Option<Decimal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct OpenOrder {
    pub id: u64,
    pub created_at: DateTime<Utc>,
    pub token_id: String,
    pub question: String,
    pub outcome: String,
    pub side: TradeSide,
    /// Limit price.
    pub price: Decimal,
    pub size: Decimal,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PaperAccount {
    pub version: u32,
    /// When true, `clob create-order` / `clob market-order` route here.
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub initial_balance: Decimal,
    /// Free cash. Cash reserved by open limit buys is already deducted.
    pub cash: Decimal,
    pub next_id: u64,
    /// Keyed by token ID (decimal string).
    pub positions: BTreeMap<String, Position>,
    pub open_orders: Vec<OpenOrder>,
    pub trades: Vec<Trade>,
}

impl PaperAccount {
    pub fn new(initial_balance: Decimal, enabled: bool) -> Self {
        Self {
            version: ACCOUNT_VERSION,
            enabled,
            created_at: Utc::now(),
            initial_balance,
            cash: initial_balance,
            next_id: 1,
            positions: BTreeMap::new(),
            open_orders: Vec::new(),
            trades: Vec::new(),
        }
    }

    pub fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Cash locked by open limit buy orders (already deducted from `cash`).
    pub fn reserved_cash(&self) -> Decimal {
        self.open_orders
            .iter()
            .filter(|o| o.side == TradeSide::Buy)
            .map(|o| o.price * o.size)
            .sum()
    }

    /// Shares of `token_id` locked by open limit sell orders.
    pub fn reserved_shares(&self, token_id: &str) -> Decimal {
        self.open_orders
            .iter()
            .filter(|o| o.side == TradeSide::Sell && o.token_id == token_id)
            .map(|o| o.size)
            .sum()
    }
}

/// A position annotated with current market data.
#[derive(Debug, Serialize)]
pub(crate) struct PositionView {
    #[serde(flatten)]
    pub position: Position,
    /// Mark (midpoint) price, if the market quoted one.
    pub mark_price: Option<Decimal>,
    pub market_value: Option<Decimal>,
    pub unrealized_pnl: Option<Decimal>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PortfolioView {
    pub initial_balance: Decimal,
    pub cash: Decimal,
    pub reserved_cash: Decimal,
    pub positions_value: Decimal,
    /// cash + reserved + positions value.
    pub equity: Decimal,
    pub realized_pnl: Decimal,
    pub unrealized_pnl: Decimal,
    /// (equity - initial) / initial * 100, rounded to 2 dp.
    pub roi_pct: Decimal,
    pub open_orders: usize,
    pub positions: Vec<PositionView>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Stats {
    pub total_trades: usize,
    pub buys: usize,
    pub sells: usize,
    pub wins: usize,
    pub losses: usize,
    /// Percentage of sells with positive realized PnL, rounded to 2 dp.
    pub win_rate_pct: Option<Decimal>,
    pub realized_pnl: Decimal,
    pub volume: Decimal,
    pub best_trade: Option<Trade>,
    pub worst_trade: Option<Trade>,
    /// Realized PnL per UTC day, ascending.
    pub daily_pnl: Vec<(NaiveDate, Decimal)>,
    /// Initial balance + cumulative realized PnL per UTC day, ascending.
    pub equity_curve: Vec<(NaiveDate, Decimal)>,
}
