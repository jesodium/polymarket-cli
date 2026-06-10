//! Persistence for configured strategy instances.
//!
//! Lives at `~/.config/polymarket/strategies.json` (override with
//! `POLYMARKET_STRATEGY_FILE`). Holds the user's strategy roster — which
//! kinds are configured, their watchlists, parameters, and whether they are
//! enabled — separate from runtime state, which the engine owns in memory.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const FILE_NAME: &str = "strategies.json";
const PATH_ENV_VAR: &str = "POLYMARKET_STRATEGY_FILE";

/// One configured strategy instance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StrategyConfig {
    /// Unique instance id (defaults to the kind; lets you run two momentum
    /// configs side by side).
    pub id: String,
    /// Strategy kind, matching the registry key.
    pub kind: String,
    /// Whether the strategy is allowed to run.
    pub enabled: bool,
    /// Token IDs the strategy watches and may trade.
    #[serde(default)]
    pub tokens: Vec<String>,
    /// Strategy-specific parameters.
    #[serde(default)]
    pub params: Value,
}

impl StrategyConfig {
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        let kind = kind.into();
        let params = super::registry::default_params(&kind);
        Self {
            id: id.into(),
            kind,
            enabled: false,
            tokens: Vec::new(),
            params,
        }
    }
}

/// The whole roster.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct StrategyBook {
    #[serde(default)]
    pub strategies: Vec<StrategyConfig>,
}

pub(crate) fn config_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var(PATH_ENV_VAR)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    Ok(crate::config::config_dir()?.join(FILE_NAME))
}

/// Load the roster, returning an empty book if the file doesn't exist yet.
pub(crate) fn load() -> Result<StrategyBook> {
    let path = config_path()?;
    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(StrategyBook::default()),
        Err(e) => {
            return Err(anyhow::anyhow!(e).context(format!("Failed to read {}", path.display())));
        }
    };
    serde_json::from_str(&data).context(format!("Invalid JSON in {}", path.display()))
}

pub(crate) fn save(book: &StrategyBook) -> Result<()> {
    let path = config_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context("Failed to create config directory")?;
    }
    let json = serde_json::to_string_pretty(book)?;
    fs::write(&path, json).context(format!("Failed to write {}", path.display()))?;
    Ok(())
}
