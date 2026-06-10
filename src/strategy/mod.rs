//! Local autonomous strategy engine.
//!
//! Strategies are plugins that implement [`Strategy`]. They receive a
//! read-only [`StrategyContext`] (live quotes, price history, current
//! position, cash) on every tick and emit [`Signal`]s. The [`engine`]
//! turns those signals into orders against the paper account today, and is
//! designed so the very same signal path can drive the authenticated CLOB
//! for live trading later (see [`engine::ExecutionMode`]).
//!
//! Strategy logic never lives in the core app — each plugin is its own file
//! under [`strategies`], registered through [`registry`].

pub(crate) mod config;
pub(crate) mod engine;
pub(crate) mod registry;
pub(crate) mod strategies;

use polymarket_client_sdk_v2::types::Decimal;

/// A live snapshot of one watched token, handed to a strategy each tick.
#[derive(Clone, Debug)]
pub(crate) struct TokenView {
    pub token_id: String,
    // Provided to plugins as context; the bundled strategies don't use them.
    #[allow(dead_code)]
    pub question: String,
    #[allow(dead_code)]
    pub outcome: String,
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    /// Midpoint, when both sides are quoted.
    pub mid: Option<Decimal>,
    /// Recent midpoints, oldest first, most recent last (this tick included).
    pub history: Vec<Decimal>,
    /// Free shares currently held in this token (paper position).
    pub position_size: Decimal,
    /// Average entry price of the held position (zero if flat).
    pub avg_price: Decimal,
}

impl TokenView {
    /// Notional pUSD value of the current position at the mid (or avg if
    /// unmarked).
    pub fn position_value(&self) -> Decimal {
        let mark = self.mid.unwrap_or(self.avg_price);
        mark * self.position_size
    }
}

/// Everything a strategy can see when deciding what to do this tick.
#[derive(Clone, Debug)]
pub(crate) struct StrategyContext {
    /// Free cash in the account.
    pub cash: Decimal,
    /// One entry per token the strategy watches.
    pub tokens: Vec<TokenView>,
}

/// An order request emitted by a strategy. Execution (paper or live) is the
/// engine's responsibility — strategies never touch wallets or the CLOB.
#[derive(Clone, Debug)]
pub(crate) enum Signal {
    MarketBuy {
        token_id: String,
        usd: Decimal,
    },
    MarketSell {
        token_id: String,
        shares: Decimal,
    },
    // The engine fully executes limit signals; the bundled example strategies
    // happen to emit only market orders, so these aren't constructed yet.
    #[allow(dead_code)]
    LimitBuy {
        token_id: String,
        price: Decimal,
        size: Decimal,
    },
    #[allow(dead_code)]
    LimitSell {
        token_id: String,
        price: Decimal,
        size: Decimal,
    },
}

impl Signal {
    pub fn token_id(&self) -> &str {
        match self {
            Signal::MarketBuy { token_id, .. }
            | Signal::MarketSell { token_id, .. }
            | Signal::LimitBuy { token_id, .. }
            | Signal::LimitSell { token_id, .. } => token_id,
        }
    }

    /// A short human description for logs.
    pub fn summary(&self) -> String {
        match self {
            Signal::MarketBuy { usd, .. } => format!("MARKET BUY ${usd}"),
            Signal::MarketSell { shares, .. } => format!("MARKET SELL {shares} shares"),
            Signal::LimitBuy { price, size, .. } => format!("LIMIT BUY {size} @ {price}"),
            Signal::LimitSell { price, size, .. } => format!("LIMIT SELL {size} @ {price}"),
        }
    }
}

/// A strategy plugin. Implementations live under [`strategies`] and are wired
/// in through [`registry`]; the core app knows nothing about their internals.
pub(crate) trait Strategy: Send {
    /// Stable identifier, e.g. `"momentum"`. Matches the registry key.
    #[allow(dead_code)]
    fn kind(&self) -> &'static str;

    /// One-line description of what the strategy does, shown in the UI.
    fn describe(&self) -> String;

    /// Decide what to do given the current market context. Pure and
    /// side-effect free: returns the orders it would like placed.
    fn on_tick(&mut self, ctx: &StrategyContext) -> Vec<Signal>;
}
