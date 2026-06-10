//! Momentum strategy: buy strength, trim weakness.
//!
//! Each tick it compares the current midpoint to the midpoint `lookback`
//! ticks ago. If the move up exceeds `threshold` it buys `trade_usd` worth
//! (while under the per-token cap); if the move down exceeds `threshold` and
//! a position is held, it sells a slice. Deliberately simple — it exists to
//! demonstrate the plugin contract, not to be profitable.

use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

use crate::strategy::{Signal, Strategy, StrategyContext};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Params {
    /// How many ticks back to measure momentum over.
    pub lookback: usize,
    /// Absolute mid move (in probability) needed to act, e.g. 0.02 = 2c.
    pub threshold: f64,
    /// pUSD to deploy per buy signal.
    pub trade_usd: f64,
    /// Stop buying once the position is worth this much pUSD.
    pub max_position_usd: f64,
    /// Fraction of the held position to sell on a down signal (0..1).
    pub sell_fraction: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            lookback: 5,
            threshold: 0.02,
            trade_usd: 100.0,
            max_position_usd: 500.0,
            sell_fraction: 1.0,
        }
    }
}

pub(crate) struct Momentum {
    params: Params,
}

impl Momentum {
    pub fn new(params: Params) -> Self {
        Self { params }
    }
}

impl Strategy for Momentum {
    fn kind(&self) -> &'static str {
        "momentum"
    }

    fn describe(&self) -> String {
        format!(
            "Buy when mid rises > {:.0}c over {} ticks; trim on the reverse. ${:.0}/order, ${:.0} cap.",
            self.params.threshold * 100.0,
            self.params.lookback,
            self.params.trade_usd,
            self.params.max_position_usd
        )
    }

    fn on_tick(&mut self, ctx: &StrategyContext) -> Vec<Signal> {
        let mut signals = Vec::new();
        let threshold = dec(self.params.threshold);
        let trade_usd = dec(self.params.trade_usd);
        let cap = dec(self.params.max_position_usd);
        let sell_fraction = dec(self.params.sell_fraction).clamp(Decimal::ZERO, Decimal::ONE);

        for t in &ctx.tokens {
            let n = t.history.len();
            if n <= self.params.lookback {
                continue;
            }
            let now_mid = t.history[n - 1];
            let past_mid = t.history[n - 1 - self.params.lookback];
            let momentum = now_mid - past_mid;

            if momentum > threshold {
                if t.position_value() < cap && ctx.cash >= trade_usd && t.best_ask.is_some() {
                    signals.push(Signal::MarketBuy {
                        token_id: t.token_id.clone(),
                        usd: trade_usd,
                    });
                }
            } else if momentum < -threshold
                && t.position_size > Decimal::ZERO
                && t.best_bid.is_some()
            {
                // Round down and cap at the held size — rounding up past the
                // position gets the sell rejected.
                let shares = (t.position_size * sell_fraction)
                    .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::ToZero)
                    .min(t.position_size);
                if shares > Decimal::ZERO {
                    signals.push(Signal::MarketSell {
                        token_id: t.token_id.clone(),
                        shares,
                    });
                }
            }
        }
        signals
    }
}

fn dec(v: f64) -> Decimal {
    Decimal::try_from(v).unwrap_or(Decimal::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::TokenView;
    use rust_decimal_macros::dec;

    fn ctx(history: Vec<Decimal>, position: Decimal) -> StrategyContext {
        StrategyContext {
            cash: dec!(10_000),
            tokens: vec![TokenView {
                token_id: "t".into(),
                question: "Q".into(),
                outcome: "Yes".into(),
                best_bid: Some(dec!(0.49)),
                best_ask: Some(dec!(0.51)),
                mid: Some(dec!(0.50)),
                history,
                position_size: position,
                avg_price: if position > Decimal::ZERO {
                    dec!(0.40)
                } else {
                    Decimal::ZERO
                },
            }],
        }
    }

    #[test]
    fn buys_on_upward_momentum() {
        let mut s = Momentum::new(Params {
            lookback: 3,
            threshold: 0.02,
            ..Default::default()
        });
        // mid rose 0.40 -> 0.50 over 3 ticks (> 2c threshold).
        let signals = s.on_tick(&ctx(
            vec![dec!(0.40), dec!(0.43), dec!(0.47), dec!(0.50)],
            dec!(0),
        ));
        assert!(matches!(signals.as_slice(), [Signal::MarketBuy { .. }]));
    }

    #[test]
    fn sells_on_downward_momentum_when_holding() {
        let mut s = Momentum::new(Params {
            lookback: 3,
            threshold: 0.02,
            ..Default::default()
        });
        let signals = s.on_tick(&ctx(
            vec![dec!(0.60), dec!(0.55), dec!(0.52), dec!(0.50)],
            dec!(100),
        ));
        assert!(matches!(signals.as_slice(), [Signal::MarketSell { .. }]));
    }

    #[test]
    fn holds_when_flat_within_threshold() {
        let mut s = Momentum::new(Params {
            lookback: 3,
            threshold: 0.05,
            ..Default::default()
        });
        let signals = s.on_tick(&ctx(
            vec![dec!(0.50), dec!(0.50), dec!(0.50), dec!(0.51)],
            dec!(0),
        ));
        assert!(signals.is_empty());
    }

    #[test]
    fn no_signal_before_enough_history() {
        let mut s = Momentum::new(Params {
            lookback: 5,
            ..Default::default()
        });
        let signals = s.on_tick(&ctx(vec![dec!(0.40), dec!(0.50)], dec!(0)));
        assert!(signals.is_empty());
    }
}
