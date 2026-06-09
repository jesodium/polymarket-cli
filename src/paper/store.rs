//! JSON persistence for the paper trading account.
//!
//! Lives at `~/.config/polymarket/paper_account.json` (override with the
//! `POLYMARKET_PAPER_FILE` env var). Deliberately separate from
//! `config.json` so wallet operations never touch paper data.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::types::PaperAccount;

const FILE_NAME: &str = "paper_account.json";
const PATH_ENV_VAR: &str = "POLYMARKET_PAPER_FILE";

pub(crate) const NO_ACCOUNT_MSG: &str =
    "No paper trading account. Run `polymarket paper enable` to create one";

pub(crate) fn account_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var(PATH_ENV_VAR)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    Ok(crate::config::config_dir()?.join(FILE_NAME))
}

/// Load the account. `Ok(None)` if none exists yet.
pub(crate) fn load() -> Result<Option<PaperAccount>> {
    let path = account_path()?;
    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(e).context(format!("Failed to read {}", path.display())));
        }
    };
    let account = serde_json::from_str(&data).context(format!(
        "Invalid JSON in paper account file {}",
        path.display()
    ))?;
    Ok(Some(account))
}

/// Load the account or fail with a setup hint.
pub(crate) fn load_required() -> Result<PaperAccount> {
    load()?.ok_or_else(|| anyhow::anyhow!("{NO_ACCOUNT_MSG}"))
}

pub(crate) fn save(account: &PaperAccount) -> Result<()> {
    let path = account_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context("Failed to create config directory")?;
    }
    let json = serde_json::to_string_pretty(account)?;
    fs::write(&path, json).context(format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Whether paper mode is toggled on (false if no account exists).
pub(crate) fn is_enabled() -> Result<bool> {
    Ok(load()?.is_some_and(|a| a.enabled))
}
