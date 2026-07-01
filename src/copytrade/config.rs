//! Persistence for followed traders (copy-trading roster).
//!
//! Lives at `~/.config/polymarket/copytrades.json` (override with
//! `POLYMARKET_COPYTRADE_FILE`). Holds the wallets you mirror and the per-trader
//! sizing / filtering rules — separate from runtime state (last-seen activity,
//! counters), which the engine keeps in memory.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "copytrades.json";
const PATH_ENV_VAR: &str = "POLYMARKET_COPYTRADE_FILE";

/// One followed trader and the rules for copying them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CopyTrader {
    /// Unique instance id (defaults to the nickname or a wallet prefix).
    pub id: String,
    /// Blockchain (proxy) address of the trader to follow, as a hex string.
    pub wallet: String,
    /// Friendly label for logs and listings.
    pub nickname: String,
    /// pUSD to deploy on each copied buy.
    pub copy_size_usd: Decimal,
    /// Hard ceiling (pUSD) on any single copied buy — caps `copy_size_usd`.
    pub max_dollar_cap: Decimal,
    /// Only copy buys whose fill price is at or above this (probability 0..1).
    pub price_min: Decimal,
    /// Only copy buys whose fill price is at or below this (probability 0..1).
    pub price_max: Decimal,
    /// Slippage tolerance (percent) enforced on copied paper market orders.
    pub slippage_pct: Decimal,
    /// When the followed trader sells a token you hold, sell your stake too.
    pub mirror_sells: bool,
    /// Whether copying is allowed to run for this trader.
    pub enabled: bool,
    /// Mirror onto the paper account instead of the live wallet. Rosters written
    /// before this field existed default to paper — never silently go live.
    #[serde(default = "default_true")]
    pub paper: bool,
}

fn default_true() -> bool {
    true
}

/// The whole copy-trading roster.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct CopyBook {
    #[serde(default)]
    pub traders: Vec<CopyTrader>,
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
pub(crate) fn load() -> Result<CopyBook> {
    let path = config_path()?;
    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CopyBook::default()),
        Err(e) => {
            return Err(anyhow::anyhow!(e).context(format!("Failed to read {}", path.display())));
        }
    };
    serde_json::from_str(&data).context(format!("Invalid JSON in {}", path.display()))
}

pub(crate) fn save(book: &CopyBook) -> Result<()> {
    let path = config_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context("Failed to create config directory")?;
    }
    let json = serde_json::to_string_pretty(book)?;
    fs::write(&path, json).context(format!("Failed to write {}", path.display()))?;
    Ok(())
}
