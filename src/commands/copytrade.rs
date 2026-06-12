//! `polymarket copytrade` — follow wallets and mirror their trades.
//!
//! The PolyGun "Copy Trade" workflow, headless: add a wallet with your sizing
//! and filters, enable it, and `run` the poller. Orders route to the paper
//! account by default (or the live CLOB with `--live`), sharing the exact
//! execution path the manual trader uses.

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use polymarket_client_sdk_v2::types::{Address, Decimal};

use crate::copytrade::config::{self, CopyTrader};
use crate::copytrade::engine::{CopyEngine, ExecutionMode};
use crate::output::OutputFormat;
use crate::paper::store;

#[derive(Args)]
pub struct CopyTradeArgs {
    #[command(subcommand)]
    pub command: CopyTradeCommand,
}

#[derive(Subcommand)]
pub enum CopyTradeCommand {
    /// Follow a wallet and copy its trades
    Add {
        /// Wallet (proxy) address to follow (0x...)
        wallet: Address,
        /// Friendly label for this trader
        #[arg(long)]
        nickname: Option<String>,
        /// Instance id (defaults to the nickname or a wallet prefix)
        #[arg(long)]
        id: Option<String>,
        /// pUSD to deploy on each copied buy
        #[arg(long, default_value = "25")]
        size: Decimal,
        /// Hard ceiling (pUSD) on any single copied buy
        #[arg(long = "max", default_value = "100")]
        max_dollar: Decimal,
        /// Only copy buys at/above this price (probability 0..1)
        #[arg(long, default_value = "0")]
        min_price: Decimal,
        /// Only copy buys at/below this price (probability 0..1)
        #[arg(long, default_value = "1")]
        max_price: Decimal,
        /// Slippage tolerance (percent) on copied paper market orders
        #[arg(long, default_value = "2")]
        slippage: Decimal,
        /// Do NOT mirror the trader's sells (only copy entries)
        #[arg(long)]
        no_mirror_sells: bool,
    },

    /// Stop following a wallet
    Remove {
        /// Instance id
        id: String,
    },

    /// Enable a follower (allowed to run)
    Enable {
        /// Instance id
        id: String,
    },

    /// Disable a follower
    Disable {
        /// Instance id
        id: String,
    },

    /// Show the roster and runtime status
    Status,

    /// List followed traders and their rules
    List,

    /// Run the copy-trading poller in the foreground (Ctrl-C to stop)
    Run {
        /// Only run this follower (default: all enabled)
        #[arg(long)]
        id: Option<String>,
        /// Seconds between polls
        #[arg(long, default_value = "15")]
        interval: u64,
        /// Mirror trades to the live CLOB instead of the paper account.
        /// Real funds — needs a configured wallet.
        #[arg(long)]
        live: bool,
    },

    /// Print the copy-trading log file
    Logs {
        /// Max lines to show (most recent)
        #[arg(long, default_value = "50")]
        limit: usize,
    },
}

pub async fn execute(args: CopyTradeArgs, output: OutputFormat) -> Result<()> {
    match args.command {
        CopyTradeCommand::Add {
            wallet,
            nickname,
            id,
            size,
            max_dollar,
            min_price,
            max_price,
            slippage,
            no_mirror_sells,
        } => {
            let wallet_str = format!("{wallet:#x}");
            let nickname = nickname.unwrap_or_else(|| short_wallet(&wallet_str));
            let id = id.unwrap_or_else(|| default_id(&nickname, &wallet_str));
            if min_price < Decimal::ZERO || max_price > Decimal::ONE || min_price > max_price {
                bail!("Price band must satisfy 0 <= min <= max <= 1");
            }
            let cfg = CopyTrader {
                id: id.clone(),
                wallet: wallet_str,
                nickname: nickname.clone(),
                copy_size_usd: size,
                max_dollar_cap: max_dollar,
                price_min: min_price,
                price_max: max_price,
                slippage_pct: slippage,
                mirror_sells: !no_mirror_sells,
                enabled: true,
            };
            let engine = build_engine(ExecutionMode::Paper)?;
            engine.add(cfg)?;
            println!("Following '{nickname}' as '{id}'. Start mirroring with `copytrade run`.");
            Ok(())
        }
        CopyTradeCommand::Remove { id } => {
            build_engine(ExecutionMode::Paper)?.remove(&id)?;
            println!("Unfollowed '{id}'.");
            Ok(())
        }
        CopyTradeCommand::Enable { id } => {
            build_engine(ExecutionMode::Paper)?.set_enabled(&id, true)?;
            println!("Enabled '{id}'.");
            Ok(())
        }
        CopyTradeCommand::Disable { id } => {
            build_engine(ExecutionMode::Paper)?.set_enabled(&id, false)?;
            println!("Disabled '{id}'.");
            Ok(())
        }
        CopyTradeCommand::Status => {
            print_status(&build_engine(ExecutionMode::Paper)?, output);
            Ok(())
        }
        CopyTradeCommand::List => list(output),
        CopyTradeCommand::Run { id, interval, live } => run(id, interval, live).await,
        CopyTradeCommand::Logs { limit } => {
            print_log_file(limit);
            Ok(())
        }
    }
}

fn list(output: OutputFormat) -> Result<()> {
    let book = config::load().unwrap_or_default();
    match output {
        OutputFormat::Json => {
            crate::output::print_json(&book.traders)?;
        }
        OutputFormat::Table => {
            if book.traders.is_empty() {
                println!("Not following anyone yet. Add a wallet:");
                println!("  polymarket copytrade add 0x<WALLET> --nickname whale");
                return Ok(());
            }
            println!("Followed traders:");
            for t in &book.traders {
                println!(
                    "  {:<14} {:<16} {} → ${} (cap ${}), band {}–{}, slip {}%{}{}",
                    t.id,
                    t.nickname,
                    short_wallet(&t.wallet),
                    t.copy_size_usd.normalize(),
                    t.max_dollar_cap.normalize(),
                    t.price_min.normalize(),
                    t.price_max.normalize(),
                    t.slippage_pct.normalize(),
                    if t.mirror_sells { ", mirror-sells" } else { "" },
                    if t.enabled { " [enabled]" } else { "" },
                );
            }
        }
    }
    Ok(())
}

fn print_status(engine: &CopyEngine, output: OutputFormat) {
    let snap = engine.snapshot();
    match output {
        OutputFormat::Json => {
            let rows: Vec<_> = snap
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "nickname": s.nickname,
                        "wallet": s.wallet,
                        "enabled": s.enabled,
                        "running": s.running,
                        "copied": s.copied,
                        "skipped": s.skipped,
                        "errors": s.errors,
                        "last_action": s.last_action,
                    })
                })
                .collect();
            let _ = crate::output::print_json(&serde_json::json!({ "followers": rows }));
        }
        OutputFormat::Table => {
            if snap.is_empty() {
                println!("Not following anyone.");
                return;
            }
            println!(
                "{:<14} {:<16} {:<9} {:<7} {:<8} LAST ACTION",
                "ID", "NICKNAME", "STATE", "COPIED", "SKIPPED"
            );
            for s in &snap {
                let state = if s.running {
                    "running"
                } else if s.enabled {
                    "idle"
                } else {
                    "disabled"
                };
                println!(
                    "{:<14} {:<16} {:<9} {:<7} {:<8} {}",
                    s.id,
                    s.nickname,
                    state,
                    s.copied,
                    s.skipped,
                    s.last_action.as_deref().unwrap_or("-")
                );
            }
        }
    }
}

async fn run(id: Option<String>, interval: u64, live: bool) -> Result<()> {
    if store::load()?.is_none() {
        bail!(
            "No paper account. Run `polymarket paper enable` first (copy-trading mirrors onto the paper account)."
        );
    }
    let mode = if live {
        ExecutionMode::Live
    } else {
        ExecutionMode::Paper
    };
    let engine = build_engine_with_interval(mode, interval)?;

    match &id {
        Some(id) => {
            engine.set_enabled(id, true)?;
            engine.start(id)?;
        }
        None => engine.start_all(),
    }

    if engine.running_count() == 0 {
        bail!(
            "No followers running. Add and enable one with `copytrade add` / `copytrade enable`."
        );
    }

    println!(
        "Copy-trading running ({mode} mode, {interval}s poll). {} follower(s) active. Ctrl-C to stop.\n",
        engine.running_count()
    );

    let mut seen = 0usize;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\nStopping copy-trading. Account state saved.");
                let _ = engine.save_account();
                break;
            }
            r = engine.poll() => {
                if let Err(e) = r {
                    eprintln!("poll error: {e}");
                }
                seen = drain_logs(&engine, seen);
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
    }
    Ok(())
}

fn drain_logs(engine: &CopyEngine, seen: usize) -> usize {
    let logs = engine.recent_logs(500);
    if logs.len() > seen {
        for line in &logs[seen..] {
            println!(
                "{} [{}] {} — {}",
                line.time.format("%H:%M:%S"),
                line.level.label(),
                line.source,
                line.message
            );
        }
    }
    logs.len()
}

fn build_engine(mode: ExecutionMode) -> Result<CopyEngine> {
    build_engine_with_interval(mode, 15)
}

fn build_engine_with_interval(mode: ExecutionMode, interval: u64) -> Result<CopyEngine> {
    let account = store::load()?.unwrap_or_else(|| {
        crate::paper::types::PaperAccount::new(
            crate::paper::types::default_starting_balance(),
            false,
        )
    });
    let account = Arc::new(Mutex::new(account));
    Ok(CopyEngine::new(account, interval, mode))
}

fn print_log_file(limit: usize) {
    let Ok(dir) = crate::config::config_dir() else {
        return;
    };
    let path = dir.join("copytrade.log");
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let lines: Vec<&str> = contents.lines().collect();
            let start = lines.len().saturating_sub(limit);
            for line in &lines[start..] {
                println!("{line}");
            }
        }
        Err(_) => println!("No copy-trading log yet at {}.", path.display()),
    }
}

/// `0x1234…cdef` short form for listings.
fn short_wallet(wallet: &str) -> String {
    let w = wallet.trim();
    if w.len() <= 12 {
        return w.to_string();
    }
    format!("{}…{}", &w[..6], &w[w.len() - 4..])
}

/// Derive a roster id from the nickname, falling back to a wallet prefix.
fn default_id(nickname: &str, wallet: &str) -> String {
    let slug: String = nickname
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        wallet.trim_start_matches("0x").chars().take(8).collect()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_wallet_truncates_long_addresses() {
        assert_eq!(
            short_wallet("0x1234567890abcdef1234567890abcdef12345678"),
            "0x1234…5678"
        );
        assert_eq!(short_wallet("0xabc"), "0xabc");
    }

    #[test]
    fn default_id_slugifies_nickname() {
        assert_eq!(default_id("Big Whale!", "0xdeadbeef00000000"), "big-whale");
        assert_eq!(default_id("  ", "0xdeadbeef00000000"), "deadbeef");
    }
}
