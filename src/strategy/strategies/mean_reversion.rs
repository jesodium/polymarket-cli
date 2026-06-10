//! Mean-reversion strategy: fade deviations from a moving average.
//!
//! Maintains a simple moving average of the midpoint over `lookback` ticks.
//! When the mid drops more than `band` below the SMA it buys (betting on a
//! bounce); when it rises more than `band` above the SMA and a position is
//! held, it sells into the strength.

use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

use crate::strategy::{Signal, Strategy, StrategyContext};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Params {
    /// Window length for the moving average, in ticks.
    pub lookback: usize,
    /// Deviation from the SMA (in probability) needed to act, e.g. 0.03.
    pub band: f64,
    /// pUSD to deploy per buy signal.
    pub trade_usd: f64,
    /// Stop buying once the position is worth this much pUSD.
    pub max_position_usd: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            lookback: 10,
            band: 0.03,
            trade_usd: 100.0,
            max_position_usd: 500.0,
        }
    }
}

pub(crate) struct MeanReversion {
    params: Params,
}

impl MeanReversion {
    pub fn new(params: Params) -> Self {
        Self { params }
    }
}

impl Strategy for MeanReversion {
    fn kind(&self) -> &'static str {
        "mean_reversion"
    }

    fn describe(&self) -> String {
        format!(
            "Buy {:.0}c below / sell {:.0}c above the {}-tick SMA. ${:.0}/order, ${:.0} cap.",
            self.params.band * 100.0,
            self.params.band * 100.0,
            self.params.lookback,
            self.params.trade_usd,
            self.params.max_position_usd
        )
    }

    fn on_tick(&mut self, ctx: &StrategyContext) -> Vec<Signal> {
        let mut signals = Vec::new();
        let band = dec(self.params.band);
        let trade_usd = dec(self.params.trade_usd);
        let cap = dec(self.params.max_position_usd);

        for t in &ctx.tokens {
            let n = t.history.len();
            if n < self.params.lookback {
                continue;
            }
            let window = &t.history[n - self.params.lookback..];
            let sum: Decimal = window.iter().copied().sum();
            let sma = sum / Decimal::from(window.len() as u64);
            let mid = t.history[n - 1];

            if mid < sma - band {
                if t.position_value() < cap && ctx.cash >= trade_usd && t.best_ask.is_some() {
                    signals.push(Signal::MarketBuy {
                        token_id: t.token_id.clone(),
                        usd: trade_usd,
                    });
                }
            } else if mid > sma + band && t.position_size > Decimal::ZERO && t.best_bid.is_some() {
                signals.push(Signal::MarketSell {
                    token_id: t.token_id.clone(),
                    // Round down — rounding up past the held size gets the
                    // sell rejected.
                    shares: t
                        .position_size
                        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::ToZero),
                });
            }
        }
        signals
    }
}

fn dec(v: f64) -> Decimal {
    Decimal::try_from(v).unwrap_or(Decimal::ZERO)
}
