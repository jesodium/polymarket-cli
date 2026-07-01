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

/// Persist the account, but refuse to clobber newer on-disk state.
///
/// The account file has several writers — the CLI, the long-lived TUI, and the
/// copy engine — and the TUI keeps the snapshot it loaded at startup. Without a
/// guard its periodic/exit saves overwrite trades the CLI just wrote, silently
/// reverting them. `next_id` only grows as trades are recorded, so a strictly
/// larger value on disk means our in-memory copy is stale; we skip the write
/// rather than lose those trades. Legitimate replacements that lower `next_id`
/// (a `reset`) must call [`save_force`].
///
/// IMPORTANT NOTE: revision check, not a real lock — a tiny read-then-write TOCTOU
/// window remains. Fine for a local single-user paper file; add `flock` if
/// concurrent paper writes ever get heavy.
pub(crate) fn save(account: &PaperAccount) -> Result<()> {
    if let Ok(Some(disk)) = load()
        && disk.next_id > account.next_id
    {
        return Ok(());
    }
    save_force(account)
}

/// Unconditional write. Use only when intentionally replacing the account
/// (e.g. `reset`), where the on-disk revision is expected to be higher.
pub(crate) fn save_force(account: &PaperAccount) -> Result<()> {
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

fn snapshots_dir() -> Result<PathBuf> {
    let dir = account_path()?
        .parent()
        .map(|p| p.join("paper_snapshots"))
        .unwrap_or_else(|| PathBuf::from("paper_snapshots"));
    Ok(dir)
}

/// Reject names that would escape the snapshots dir or need quoting.
fn valid_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains(['/', '\\', '.'])
        || name.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        anyhow::bail!("Snapshot name must be a single word without '/', '.', or spaces");
    }
    Ok(())
}

/// Copy the current account file to `paper_snapshots/<name>.json`.
pub(crate) fn snapshot_save(name: &str) -> Result<PathBuf> {
    valid_name(name)?;
    let src = account_path()?;
    if !src.exists() {
        anyhow::bail!("{NO_ACCOUNT_MSG}");
    }
    let dir = snapshots_dir()?;
    fs::create_dir_all(&dir).context("Failed to create snapshots directory")?;
    let dst = dir.join(format!("{name}.json"));
    fs::copy(&src, &dst).context(format!("Failed to write {}", dst.display()))?;
    Ok(dst)
}

/// Overwrite the account file with snapshot `<name>`.
pub(crate) fn snapshot_restore(name: &str) -> Result<()> {
    valid_name(name)?;
    let src = snapshots_dir()?.join(format!("{name}.json"));
    if !src.exists() {
        anyhow::bail!("No snapshot named '{name}'. See `paper snapshot list`");
    }
    // Validate it parses before clobbering the live account.
    let data = fs::read_to_string(&src).context(format!("Failed to read {}", src.display()))?;
    let account: PaperAccount =
        serde_json::from_str(&data).context(format!("Corrupt snapshot {}", src.display()))?;
    save_force(&account) // bypass the stale-write guard: a restore is intentional
}

/// Snapshot names present on disk (without the `.json` suffix), sorted.
pub(crate) fn snapshot_list() -> Result<Vec<String>> {
    let dir = snapshots_dir()?;
    let mut names: Vec<String> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|e| {
                e.ok()?
                    .file_name()
                    .to_str()?
                    .strip_suffix(".json")
                    .map(String::from)
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(anyhow::anyhow!(e).context(format!("Failed to read {}", dir.display())));
        }
    };
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paper::types::{PaperAccount, default_starting_balance};

    fn acct(next_id: u64) -> PaperAccount {
        let mut a = PaperAccount::new(default_starting_balance(), true);
        a.next_id = next_id;
        a
    }

    #[test]
    fn save_refuses_to_clobber_newer_disk_state() {
        let path = std::env::temp_dir().join(format!("pm_paper_test_{}.json", std::process::id()));
        // SAFETY: single-threaded test; restored before returning.
        unsafe { std::env::set_var(PATH_ENV_VAR, &path) };
        let _ = fs::remove_file(&path);

        // A fresh CLI write seeds disk at a high revision.
        save_force(&acct(10)).unwrap();
        assert_eq!(load().unwrap().unwrap().next_id, 10);

        // A stale writer (the TUI's startup snapshot) must NOT overwrite it.
        save(&acct(5)).unwrap();
        assert_eq!(
            load().unwrap().unwrap().next_id,
            10,
            "stale save clobbered newer disk state"
        );

        // A genuinely newer write goes through.
        save(&acct(12)).unwrap();
        assert_eq!(load().unwrap().unwrap().next_id, 12);

        // `reset` lowers the revision and must still take effect via save_force.
        save_force(&acct(1)).unwrap();
        assert_eq!(load().unwrap().unwrap().next_id, 1);

        let _ = fs::remove_file(&path);
        unsafe { std::env::remove_var(PATH_ENV_VAR) };
    }
}
