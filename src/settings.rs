//! Polygun-style trading settings.
//!
//! Modeled on the PolyGun bot's Settings page: a *trading mode* that decides
//! when an order needs confirmation, a confirmation *threshold*, customizable
//! *quickbuy* / *quicksell* presets, a default *slippage* tolerance, and
//! default *take-profit* / *stop-loss* levels that get attached to new
//! positions. Persisted to `~/.config/polymarket/settings.json` (override with
//! `POLYMARKET_SETTINGS_FILE`), separate from the wallet config and the paper
//! account.

use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "settings.json";
const PATH_ENV_VAR: &str = "POLYMARKET_SETTINGS_FILE";

/// How aggressively orders execute — the PolyGun "Trading Mode".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TradingMode {
    /// Every order asks for confirmation first.
    Cautious,
    /// Only orders at/above [`Settings::confirm_threshold_usd`] confirm.
    Standard,
    /// Fire instantly, never confirm.
    Expert,
}

impl TradingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cautious => "Cautious",
            Self::Standard => "Standard",
            Self::Expert => "Expert",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Cautious => "Always confirm before sending an order",
            Self::Standard => "Confirm only orders at/above the threshold",
            Self::Expert => "Execute instantly, no confirmation",
        }
    }

    /// Cycle to the next mode (for a UI toggle).
    pub fn next(self) -> Self {
        match self {
            Self::Cautious => Self::Standard,
            Self::Standard => Self::Expert,
            Self::Expert => Self::Cautious,
        }
    }
}

impl std::fmt::Display for TradingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

impl FromStr for TradingMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_lowercase().as_str() {
            "cautious" => Ok(Self::Cautious),
            "standard" => Ok(Self::Standard),
            "expert" => Ok(Self::Expert),
            other => anyhow::bail!("Unknown trading mode '{other}' (cautious|standard|expert)"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// When an order needs confirmation.
    pub trading_mode: TradingMode,
    /// In Standard mode, orders at/above this notional (pUSD) confirm first.
    pub confirm_threshold_usd: Decimal,
    /// One-tap buy amounts (pUSD) shown as buttons in the order ticket.
    pub quickbuy_presets: Vec<Decimal>,
    /// One-tap sell fractions (percent of the held position).
    pub quicksell_presets: Vec<Decimal>,
    /// Price slippage tolerance (percent) allowed on market orders.
    pub slippage_pct: Decimal,
    /// Default take-profit (percent gain) attached to new positions, if set.
    pub default_take_profit_pct: Option<Decimal>,
    /// Default stop-loss (percent loss) attached to new positions, if set.
    pub default_stop_loss_pct: Option<Decimal>,
    /// Default trailing-stop (percent off peak) attached to new positions.
    pub default_trailing_stop_pct: Option<Decimal>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trading_mode: TradingMode::Standard,
            confirm_threshold_usd: Decimal::from(100),
            quickbuy_presets: vec![
                Decimal::from(10),
                Decimal::from(25),
                Decimal::from(50),
                Decimal::from(100),
            ],
            quicksell_presets: vec![Decimal::from(25), Decimal::from(50), Decimal::from(100)],
            slippage_pct: Decimal::from(2),
            default_take_profit_pct: None,
            default_stop_loss_pct: None,
            default_trailing_stop_pct: None,
        }
    }
}

impl Settings {
    /// Whether an order of `notional` pUSD must be confirmed before sending,
    /// per the current trading mode.
    pub fn requires_confirmation(&self, notional: Decimal) -> bool {
        match self.trading_mode {
            TradingMode::Cautious => true,
            TradingMode::Standard => notional >= self.confirm_threshold_usd,
            TradingMode::Expert => false,
        }
    }

    /// Whether any default exit (TP/SL/trailing) is configured.
    pub fn has_default_exit(&self) -> bool {
        self.default_take_profit_pct.is_some()
            || self.default_stop_loss_pct.is_some()
            || self.default_trailing_stop_pct.is_some()
    }
}

pub(crate) fn config_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var(PATH_ENV_VAR)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    Ok(crate::config::config_dir()?.join(FILE_NAME))
}

/// Load settings, falling back to defaults if the file is missing.
pub(crate) fn load() -> Settings {
    let Ok(path) = config_path() else {
        return Settings::default();
    };
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub(crate) fn save(settings: &Settings) -> Result<()> {
    let path = config_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context("Failed to create config directory")?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(&path, json).context(format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Format a percent list like `[25, 50, 100]` → `25% / 50% / 100%`.
pub(crate) fn fmt_pct_list(values: &[Decimal]) -> String {
    if values.is_empty() {
        return "—".to_string();
    }
    values
        .iter()
        .map(|v| format!("{}%", v.normalize()))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Format a money list like `[10, 25, 50]` → `$10 / $25 / $50`.
pub(crate) fn fmt_money_list(values: &[Decimal]) -> String {
    if values.is_empty() {
        return "—".to_string();
    }
    values
        .iter()
        .map(|v| format!("${}", v.normalize()))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Parse a comma/space separated number list, e.g. "10, 25, 50".
pub(crate) fn parse_number_list(s: &str) -> Result<Vec<Decimal>> {
    let mut out = Vec::new();
    for part in s.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
        let v = Decimal::from_str(part).map_err(|_| anyhow::anyhow!("'{part}' is not a number"))?;
        if v <= Decimal::ZERO {
            anyhow::bail!("values must be positive (got {part})");
        }
        out.push(v);
    }
    if out.is_empty() {
        anyhow::bail!("provide at least one value");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn standard_mode_confirms_above_threshold() {
        let s = Settings::default();
        assert!(!s.requires_confirmation(dec!(50)));
        assert!(s.requires_confirmation(dec!(100)));
        assert!(s.requires_confirmation(dec!(250)));
    }

    #[test]
    fn cautious_always_confirms_expert_never() {
        let mut s = Settings {
            trading_mode: TradingMode::Cautious,
            ..Default::default()
        };
        assert!(s.requires_confirmation(dec!(1)));
        s.trading_mode = TradingMode::Expert;
        assert!(!s.requires_confirmation(dec!(100_000)));
    }

    #[test]
    fn mode_cycles_through_all_three() {
        let m = TradingMode::Cautious;
        assert_eq!(m.next(), TradingMode::Standard);
        assert_eq!(m.next().next(), TradingMode::Expert);
        assert_eq!(m.next().next().next(), TradingMode::Cautious);
    }

    #[test]
    fn parses_number_lists() {
        assert_eq!(
            parse_number_list("10, 25, 50").unwrap(),
            vec![dec!(10), dec!(25), dec!(50)]
        );
        assert_eq!(parse_number_list("5 15").unwrap(), vec![dec!(5), dec!(15)]);
        assert!(parse_number_list("").is_err());
        assert!(parse_number_list("-1").is_err());
        assert!(parse_number_list("abc").is_err());
    }

    #[test]
    fn has_default_exit_reflects_any_set() {
        let mut s = Settings::default();
        assert!(!s.has_default_exit());
        s.default_take_profit_pct = Some(dec!(50));
        assert!(s.has_default_exit());
    }
}
