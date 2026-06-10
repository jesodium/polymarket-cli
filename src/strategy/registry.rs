//! Plugin registry: maps a strategy `kind` string to its constructor and
//! default parameters. This is the only place the core engine learns which
//! strategies exist — adding one means adding a line here, nothing else.

use anyhow::{Result, bail};
use serde_json::Value;

use super::Strategy;
use super::strategies::{mean_reversion, momentum, tp_sl};

/// Static description of an available strategy, for listings/UI.
pub(crate) struct StrategyMeta {
    pub kind: &'static str,
    pub summary: &'static str,
    /// Default parameters as JSON, so the UI/CLI can show and edit them.
    pub default_params: Value,
}

/// Every strategy the build knows about.
pub(crate) fn available() -> Vec<StrategyMeta> {
    vec![
        StrategyMeta {
            kind: "momentum",
            summary: "Trend follower: buys upward momentum, trims on reversals.",
            default_params: serde_json::to_value(momentum::Params::default())
                .unwrap_or(Value::Null),
        },
        StrategyMeta {
            kind: "mean_reversion",
            summary: "Contrarian: fades deviations from a moving average.",
            default_params: serde_json::to_value(mean_reversion::Params::default())
                .unwrap_or(Value::Null),
        },
        StrategyMeta {
            kind: "tp_sl",
            summary: "Take-profit / stop-loss guard: auto-exits a held position.",
            default_params: serde_json::to_value(tp_sl::Params::default())
                .unwrap_or(Value::Null),
        },
    ]
}

/// Default parameters for a strategy kind, or `Null` if unknown.
pub(crate) fn default_params(kind: &str) -> Value {
    available()
        .into_iter()
        .find(|m| m.kind == kind)
        .map(|m| m.default_params)
        .unwrap_or(Value::Null)
}

/// Build a live strategy instance from its kind and JSON parameters.
/// Missing or partial params fall back to the strategy's defaults.
pub(crate) fn build(kind: &str, params: &Value) -> Result<Box<dyn Strategy>> {
    match kind {
        "momentum" => {
            let p: momentum::Params = parse(params)?;
            Ok(Box::new(momentum::Momentum::new(p)))
        }
        "mean_reversion" => {
            let p: mean_reversion::Params = parse(params)?;
            Ok(Box::new(mean_reversion::MeanReversion::new(p)))
        }
        "tp_sl" => {
            let p: tp_sl::Params = parse(params)?;
            Ok(Box::new(tp_sl::TpSl::new(p)))
        }
        other => bail!("Unknown strategy '{other}'. Run `polymarket strategy list` to see options"),
    }
}

fn parse<T: serde::de::DeserializeOwned + Default>(params: &Value) -> Result<T> {
    if params.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(params.clone()).map_err(|e| anyhow::anyhow!("Invalid parameters: {e}"))
}
