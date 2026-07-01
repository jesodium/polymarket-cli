//! Standalone take-profit / stop-loss exit guard.
//!
//! A buy from the TUI can arm a guard on the token: a take-profit, a stop-loss,
//! and/or a trailing stop. The guards are persisted to
//! `~/.config/polymarket/guards.json` (override with `POLYMARKET_GUARD_FILE`)
//! and evaluated by the TUI's background [`crate::tui`] data refresher each
//! pass — when a held position crosses a threshold the guard market-sells it.
//!
//! This is deliberately small and self-contained: no strategy engine, no
//! plugins, just "watch a position I hold and exit it on my terms".

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "guards.json";
const PATH_ENV_VAR: &str = "POLYMARKET_GUARD_FILE";

/// One armed exit guard on a token. All three exits are optional; the first to
/// trigger fires and sells the whole held position.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Guard {
    pub token_id: String,
    /// Guards a live CLOB position (false = paper account). Recorded at arm
    /// time so the background worker knows which account to watch.
    #[serde(default)]
    pub live: bool,
    /// Sell once unrealized gain reaches this percent (e.g. 30 = +30%).
    #[serde(default)]
    pub take_profit_pct: Option<Decimal>,
    /// Sell once unrealized loss reaches this percent (e.g. 20 = -20%).
    #[serde(default)]
    pub stop_loss_pct: Option<Decimal>,
    /// Sell once the mark falls this percent below its peak since arming.
    #[serde(default)]
    pub trailing_stop_pct: Option<Decimal>,
}

impl Guard {
    pub fn is_empty(&self) -> bool {
        self.take_profit_pct.is_none()
            && self.stop_loss_pct.is_none()
            && self.trailing_stop_pct.is_none()
    }

    /// A short human description, e.g. `TP +30%, SL -20%`.
    pub fn describe(&self) -> String {
        let mut bits = Vec::new();
        if let Some(tp) = self.take_profit_pct {
            bits.push(format!("TP +{}%", tp.normalize()));
        }
        if let Some(sl) = self.stop_loss_pct {
            bits.push(format!("SL -{}%", sl.normalize()));
        }
        if let Some(tr) = self.trailing_stop_pct {
            bits.push(format!("trail {}%", tr.normalize()));
        }
        bits.join(", ")
    }
}

/// The whole set of armed guards.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct GuardBook {
    #[serde(default)]
    pub guards: Vec<Guard>,
}

pub(crate) fn config_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var(PATH_ENV_VAR)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    Ok(crate::config::config_dir()?.join(FILE_NAME))
}

pub(crate) fn load() -> Result<GuardBook> {
    let path = config_path()?;
    match fs::read_to_string(&path) {
        Ok(data) => {
            serde_json::from_str(&data).context(format!("Invalid JSON in {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GuardBook::default()),
        Err(e) => Err(anyhow::anyhow!(e).context(format!("Failed to read {}", path.display()))),
    }
}

pub(crate) fn save(book: &GuardBook) -> Result<()> {
    let path = config_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context("Failed to create config directory")?;
    }
    fs::write(&path, serde_json::to_string_pretty(book)?)
        .context(format!("Failed to write {}", path.display()))
}

/// Arm (or replace) the guard on `token_id`. A guard with no exits set is
/// removed instead. Persists the change.
pub(crate) fn arm(
    token_id: &str,
    live: bool,
    take_profit_pct: Option<Decimal>,
    stop_loss_pct: Option<Decimal>,
    trailing_stop_pct: Option<Decimal>,
) -> Result<()> {
    let mut book = load().unwrap_or_default();
    book.guards.retain(|g| g.token_id != token_id);
    let guard = Guard {
        token_id: token_id.to_string(),
        live,
        take_profit_pct,
        stop_loss_pct,
        trailing_stop_pct,
    };
    if !guard.is_empty() {
        book.guards.push(guard);
    }
    save(&book)
}

/// Remove the guard on `token_id` (no-op if none). Persists.
pub(crate) fn clear(token_id: &str) -> Result<()> {
    let mut book = load().unwrap_or_default();
    let before = book.guards.len();
    book.guards.retain(|g| g.token_id != token_id);
    if book.guards.len() != before {
        save(&book)?;
    }
    Ok(())
}

/// Which exit (if any) fires for the given marks. Pure and testable.
pub(crate) fn exit_reason(
    guard: &Guard,
    gain_pct: Decimal,
    mid: Decimal,
    peak: Decimal,
    avg: Decimal,
) -> Option<&'static str> {
    if let Some(tp) = guard.take_profit_pct
        && gain_pct >= tp
    {
        return Some("take-profit");
    }
    if let Some(sl) = guard.stop_loss_pct
        && gain_pct <= -sl
    {
        return Some("stop-loss");
    }
    if let Some(tr) = guard.trailing_stop_pct
        && peak > avg // only arm the trail once the position has been in profit
        && peak > Decimal::ZERO
    {
        let drop_pct = (peak - mid) / peak * Decimal::ONE_HUNDRED;
        if drop_pct >= tr {
            return Some("trailing-stop");
        }
    }
    None
}

/// What the guard ticker decides to do for one token this pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuardAction {
    /// Sell `shares` of the token; `reason` names the exit that fired.
    Sell {
        shares: Decimal,
        reason: &'static str,
    },
    /// The position is gone — drop the guard.
    Drop,
    /// Nothing to do.
    Hold,
}

/// Decide what to do for one guard, given the held position and current mark.
/// `peaks` tracks the highest mark per token for the trailing stop and is
/// updated in place. Pure except for the peak bookkeeping.
pub(crate) fn evaluate(
    guard: &Guard,
    free_shares: Decimal,
    avg_price: Decimal,
    mid: Option<Decimal>,
    best_bid: Option<Decimal>,
    peaks: &mut HashMap<String, Decimal>,
) -> GuardAction {
    if free_shares <= Decimal::ZERO || avg_price <= Decimal::ZERO {
        peaks.remove(&guard.token_id);
        return GuardAction::Drop;
    }
    // Need a mark and something to sell into this pass.
    let (Some(mid), Some(_)) = (mid, best_bid) else {
        return GuardAction::Hold;
    };
    let peak = peaks
        .entry(guard.token_id.clone())
        .and_modify(|p| {
            if mid > *p {
                *p = mid;
            }
        })
        .or_insert(mid);
    let peak = *peak;
    let gain_pct = (mid - avg_price) / avg_price * Decimal::ONE_HUNDRED;
    match exit_reason(guard, gain_pct, mid, peak, avg_price) {
        Some(reason) => {
            let shares =
                free_shares.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::ToZero);
            if shares > Decimal::ZERO {
                peaks.remove(&guard.token_id);
                GuardAction::Sell { shares, reason }
            } else {
                GuardAction::Hold
            }
        }
        None => GuardAction::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn guard(tp: Option<Decimal>, sl: Option<Decimal>, tr: Option<Decimal>) -> Guard {
        Guard {
            token_id: "t".into(),
            live: false,
            take_profit_pct: tp,
            stop_loss_pct: sl,
            trailing_stop_pct: tr,
        }
    }

    #[test]
    fn take_profit_fires() {
        let g = guard(Some(dec!(20)), None, None);
        let mut peaks = HashMap::new();
        // entry 0.50, mark 0.65 → +30% ≥ +20%.
        let action = evaluate(
            &g,
            dec!(100),
            dec!(0.50),
            Some(dec!(0.65)),
            Some(dec!(0.65)),
            &mut peaks,
        );
        assert_eq!(
            action,
            GuardAction::Sell {
                shares: dec!(100),
                reason: "take-profit"
            }
        );
    }

    #[test]
    fn stop_loss_fires() {
        let g = guard(None, Some(dec!(20)), None);
        let mut peaks = HashMap::new();
        // entry 0.50, mark 0.39 → -22% ≤ -20%.
        let action = evaluate(
            &g,
            dec!(100),
            dec!(0.50),
            Some(dec!(0.39)),
            Some(dec!(0.39)),
            &mut peaks,
        );
        assert!(matches!(
            action,
            GuardAction::Sell {
                reason: "stop-loss",
                ..
            }
        ));
    }

    #[test]
    fn holds_inside_band() {
        let g = guard(Some(dec!(50)), Some(dec!(50)), None);
        let mut peaks = HashMap::new();
        let action = evaluate(
            &g,
            dec!(100),
            dec!(0.50),
            Some(dec!(0.52)),
            Some(dec!(0.52)),
            &mut peaks,
        );
        assert_eq!(action, GuardAction::Hold);
    }

    #[test]
    fn trailing_stop_fires_after_peak_then_drop() {
        let g = guard(None, None, Some(dec!(10)));
        let mut peaks = HashMap::new();
        assert_eq!(
            evaluate(
                &g,
                dec!(100),
                dec!(0.50),
                Some(dec!(0.60)),
                Some(dec!(0.60)),
                &mut peaks
            ),
            GuardAction::Hold
        );
        assert_eq!(
            evaluate(
                &g,
                dec!(100),
                dec!(0.50),
                Some(dec!(0.80)),
                Some(dec!(0.80)),
                &mut peaks
            ),
            GuardAction::Hold
        );
        // Drop from peak 0.80 to 0.70 = -12.5% ≥ 10% trail.
        assert!(matches!(
            evaluate(
                &g,
                dec!(100),
                dec!(0.50),
                Some(dec!(0.70)),
                Some(dec!(0.70)),
                &mut peaks
            ),
            GuardAction::Sell {
                reason: "trailing-stop",
                ..
            }
        ));
    }

    #[test]
    fn drops_guard_when_flat() {
        let g = guard(Some(dec!(10)), None, None);
        let mut peaks = HashMap::new();
        assert_eq!(
            evaluate(&g, dec!(0), dec!(0), None, None, &mut peaks),
            GuardAction::Drop
        );
    }

    #[test]
    fn holds_without_a_bid_to_sell_into() {
        let g = guard(Some(dec!(10)), None, None);
        let mut peaks = HashMap::new();
        let action = evaluate(
            &g,
            dec!(100),
            dec!(0.50),
            Some(dec!(0.65)),
            None,
            &mut peaks,
        );
        assert_eq!(action, GuardAction::Hold);
    }

    #[test]
    fn arm_then_clear_roundtrips() {
        let dir = std::env::temp_dir().join(format!("guard-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("guards.json");
        unsafe { std::env::set_var(PATH_ENV_VAR, &path) };
        let _ = std::fs::remove_file(&path);

        arm("123", false, Some(dec!(30)), Some(dec!(20)), None).unwrap();
        let book = load().unwrap();
        assert_eq!(book.guards.len(), 1);
        assert_eq!(book.guards[0].take_profit_pct, Some(dec!(30)));

        // Arming with no exits removes it.
        arm("123", false, None, None, None).unwrap();
        assert!(load().unwrap().guards.is_empty());

        unsafe { std::env::remove_var(PATH_ENV_VAR) };
        let _ = std::fs::remove_file(&path);
    }
}
