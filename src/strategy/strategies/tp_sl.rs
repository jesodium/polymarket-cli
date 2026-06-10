//! Take-profit / stop-loss / trailing-stop automation.
//!
//! This is the PolyGun-style position guard: attach it to a token you hold and
//! it watches the mark each tick, then market-sells the position when it hits
//! your profit target, your loss limit, or trails too far off its peak. It
//! never opens new positions — it only exits existing ones — so it's safe to
//! run alongside a manual or signal-driven entry.
//!
//! All three exits are optional and independent; the first one to trigger
//! fires. Percentages are measured against the position's average entry price
//! (TP/SL) or the highest mark seen since the guard started watching
//! (trailing).

use std::collections::HashMap;

use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

use crate::strategy::{Signal, Strategy, StrategyContext};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Params {
    /// Sell once unrealized gain reaches this percent (e.g. 30 = +30%).
    pub take_profit_pct: Option<f64>,
    /// Sell once unrealized loss reaches this percent (e.g. 20 = -20%).
    pub stop_loss_pct: Option<f64>,
    /// Sell once the mark falls this percent below its peak since entry.
    pub trailing_stop_pct: Option<f64>,
    /// Fraction of the held position to sell when an exit triggers (0..1).
    pub sell_fraction: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            take_profit_pct: Some(30.0),
            stop_loss_pct: Some(20.0),
            trailing_stop_pct: None,
            sell_fraction: 1.0,
        }
    }
}

pub(crate) struct TpSl {
    params: Params,
    /// Highest mark seen per token since we started watching (for trailing).
    peaks: HashMap<String, Decimal>,
}

impl TpSl {
    pub fn new(params: Params) -> Self {
        Self {
            params,
            peaks: HashMap::new(),
        }
    }
}

impl Strategy for TpSl {
    fn kind(&self) -> &'static str {
        "tp_sl"
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(tp) = self.params.take_profit_pct {
            parts.push(format!("TP +{tp:.0}%"));
        }
        if let Some(sl) = self.params.stop_loss_pct {
            parts.push(format!("SL -{sl:.0}%"));
        }
        if let Some(tr) = self.params.trailing_stop_pct {
            parts.push(format!("trail {tr:.0}%"));
        }
        if parts.is_empty() {
            return "Take-profit / stop-loss guard (no exits configured).".to_string();
        }
        format!(
            "Auto-exit guard: {} — sells {:.0}% of the position on trigger.",
            parts.join(", "),
            (self.params.sell_fraction.clamp(0.0, 1.0)) * 100.0
        )
    }

    fn on_tick(&mut self, ctx: &StrategyContext) -> Vec<Signal> {
        let mut signals = Vec::new();
        let sell_fraction = dec(self.params.sell_fraction).clamp(Decimal::ZERO, Decimal::ONE);

        for t in &ctx.tokens {
            // Only manage tokens we actually hold and can mark + sell into.
            if t.position_size <= Decimal::ZERO || t.avg_price <= Decimal::ZERO {
                self.peaks.remove(&t.token_id);
                continue;
            }
            let Some(mid) = t.mid else { continue };
            if t.best_bid.is_none() {
                continue; // nothing to sell into this tick
            }

            // Track the peak mark for trailing.
            let peak = self
                .peaks
                .entry(t.token_id.clone())
                .and_modify(|p| {
                    if mid > *p {
                        *p = mid;
                    }
                })
                .or_insert(mid);
            let peak = *peak;

            let gain_pct = (mid - t.avg_price) / t.avg_price * Decimal::ONE_HUNDRED;

            let reason = exit_reason(&self.params, gain_pct, mid, peak, t.avg_price);
            if reason.is_none() {
                continue;
            }

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
                // Forget the peak so a re-entry starts a fresh trail.
                self.peaks.remove(&t.token_id);
            }
        }
        signals
    }
}

/// Which exit (if any) fires for the given marks. Returned for readability and
/// testing; the engine just needs to know whether to sell.
fn exit_reason(
    params: &Params,
    gain_pct: Decimal,
    mid: Decimal,
    peak: Decimal,
    avg: Decimal,
) -> Option<&'static str> {
    if let Some(tp) = params.take_profit_pct
        && gain_pct >= dec(tp)
    {
        return Some("take-profit");
    }
    if let Some(sl) = params.stop_loss_pct
        && gain_pct <= -dec(sl)
    {
        return Some("stop-loss");
    }
    if let Some(tr) = params.trailing_stop_pct
        && peak > avg // only arm the trail once the position has been in profit
        && peak > Decimal::ZERO
    {
        let drop_pct = (peak - mid) / peak * Decimal::ONE_HUNDRED;
        if drop_pct >= dec(tr) {
            return Some("trailing-stop");
        }
    }
    None
}

fn dec(v: f64) -> Decimal {
    Decimal::try_from(v).unwrap_or(Decimal::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::TokenView;
    use rust_decimal_macros::dec;

    fn ctx(mid: Decimal, avg: Decimal, size: Decimal) -> StrategyContext {
        StrategyContext {
            cash: dec!(10_000),
            tokens: vec![TokenView {
                token_id: "t".into(),
                question: "Q".into(),
                outcome: "Yes".into(),
                best_bid: Some(mid),
                best_ask: Some(mid),
                mid: Some(mid),
                history: vec![mid],
                position_size: size,
                avg_price: avg,
            }],
        }
    }

    #[test]
    fn take_profit_triggers_a_sell() {
        let mut s = TpSl::new(Params {
            take_profit_pct: Some(20.0),
            stop_loss_pct: None,
            trailing_stop_pct: None,
            sell_fraction: 1.0,
        });
        // entry 0.50, mark 0.65 → +30% ≥ +20%.
        let sig = s.on_tick(&ctx(dec!(0.65), dec!(0.50), dec!(100)));
        assert!(matches!(sig.as_slice(), [Signal::MarketSell { shares, .. }] if *shares == dec!(100)));
    }

    #[test]
    fn stop_loss_triggers_a_sell() {
        let mut s = TpSl::new(Params {
            take_profit_pct: None,
            stop_loss_pct: Some(20.0),
            trailing_stop_pct: None,
            sell_fraction: 1.0,
        });
        // entry 0.50, mark 0.39 → -22% ≤ -20%.
        let sig = s.on_tick(&ctx(dec!(0.39), dec!(0.50), dec!(100)));
        assert!(matches!(sig.as_slice(), [Signal::MarketSell { .. }]));
    }

    #[test]
    fn holds_inside_the_band() {
        let mut s = TpSl::new(Params {
            take_profit_pct: Some(50.0),
            stop_loss_pct: Some(50.0),
            trailing_stop_pct: None,
            sell_fraction: 1.0,
        });
        let sig = s.on_tick(&ctx(dec!(0.52), dec!(0.50), dec!(100)));
        assert!(sig.is_empty());
    }

    #[test]
    fn trailing_stop_sells_after_peak_then_drop() {
        let mut s = TpSl::new(Params {
            take_profit_pct: None,
            stop_loss_pct: None,
            trailing_stop_pct: Some(10.0),
            sell_fraction: 1.0,
        });
        // Climb to a peak well above entry; trail arms.
        assert!(s.on_tick(&ctx(dec!(0.60), dec!(0.50), dec!(100))).is_empty());
        assert!(s.on_tick(&ctx(dec!(0.80), dec!(0.50), dec!(100))).is_empty());
        // Drop from peak 0.80 to 0.70 = -12.5% ≥ 10% trail.
        let sig = s.on_tick(&ctx(dec!(0.70), dec!(0.50), dec!(100)));
        assert!(matches!(sig.as_slice(), [Signal::MarketSell { .. }]));
    }

    #[test]
    fn sell_fraction_scales_the_exit() {
        let mut s = TpSl::new(Params {
            take_profit_pct: Some(10.0),
            stop_loss_pct: None,
            trailing_stop_pct: None,
            sell_fraction: 0.5,
        });
        let sig = s.on_tick(&ctx(dec!(0.60), dec!(0.50), dec!(100)));
        assert!(matches!(sig.as_slice(), [Signal::MarketSell { shares, .. }] if *shares == dec!(50)));
    }

    #[test]
    fn no_signal_when_flat() {
        let mut s = TpSl::new(Params::default());
        let sig = s.on_tick(&ctx(dec!(0.60), dec!(0.50), dec!(0)));
        assert!(sig.is_empty());
    }
}
