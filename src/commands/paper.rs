use std::str::FromStr;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Subcommand};
use polymarket_client_sdk_v2::types::Decimal;

use crate::auth;
use crate::output::OutputFormat;
use crate::output::paper::{
    print_fill, print_history, print_open_orders, print_portfolio, print_resting,
    print_settled_fills, print_stats, print_status,
};
use crate::paper::types::{PaperAccount, default_starting_balance};
use crate::paper::{engine, quotes, store};

#[derive(Args)]
pub struct PaperArgs {
    #[command(subcommand)]
    pub command: PaperCommand,
}

#[derive(Subcommand)]
pub enum PaperCommand {
    /// Turn on paper mode (creates a $10,000 virtual account if none exists)
    Enable,

    /// Turn off paper mode (account data is kept)
    Disable,

    /// Show paper mode status and account summary
    Status,

    /// Start over with a fresh virtual account
    Reset {
        /// Starting virtual balance in pUSD
        #[arg(long, default_value = "10000")]
        balance: String,
    },

    /// Simulated buy: market (--amount) or limit (--price + --size)
    Buy {
        /// Token ID (numeric string)
        token_id: String,
        /// pUSD to spend (market order)
        #[arg(long, conflicts_with_all = ["price", "size"])]
        amount: Option<String>,
        /// Limit price (requires --size)
        #[arg(long, requires = "size")]
        price: Option<String>,
        /// Number of shares (limit order, requires --price)
        #[arg(long, requires = "price")]
        size: Option<String>,
    },

    /// Simulated sell: market by default, limit with --price
    Sell {
        /// Token ID (numeric string)
        token_id: String,
        /// Number of shares to sell
        #[arg(long)]
        size: String,
        /// Limit price (omit for a market sell)
        #[arg(long)]
        price: Option<String>,
    },

    /// Show virtual portfolio: cash, positions, PnL, ROI
    Portfolio,

    /// Show the simulated trade log
    History {
        /// Max trades to show (most recent first)
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// List resting paper limit orders
    Orders,

    /// Cancel a resting paper limit order
    Cancel {
        /// Paper order ID
        order_id: u64,
    },

    /// Settle a held position at market resolution ($1 won, $0 lost).
    /// Cancels the token's resting orders, then books realized PnL.
    /// Omit --payout to auto-resolve the winner from Gamma (same source the TUI
    /// uses); fails if the market hasn't finalized yet.
    Settle {
        /// Token ID (numeric string)
        token_id: String,
        /// Resolution payout per share: 1 = won, 0 = lost.
        /// Omit to auto-resolve from the market's final outcome price.
        #[arg(long)]
        payout: Option<String>,
    },

    /// Performance analytics: win rate, best/worst trade, daily PnL
    Stats,

    /// Save / restore / list named copies of the paper account
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommand,
    },

    /// Write the trade log, positions, or per-market history to a CSV file on the Desktop
    Export {
        /// What to export
        #[arg(value_enum)]
        what: ExportKind,
    },
}

#[derive(Subcommand)]
pub enum SnapshotCommand {
    /// Copy the current account to a named snapshot
    Save { name: String },
    /// Overwrite the current account with a named snapshot
    Restore { name: String },
    /// List saved snapshots
    List,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ExportKind {
    Trades,
    Positions,
    /// Per-market summary: original size, final size, ROI, PnL.
    History,
}

pub async fn execute(args: PaperArgs, output: OutputFormat) -> Result<()> {
    let output = &output;
    match args.command {
        PaperCommand::Enable => {
            let mut account = match store::load()? {
                Some(a) => a,
                None => PaperAccount::new(default_starting_balance(), false),
            };
            account.enabled = true;
            store::save(&account)?;
            match output {
                OutputFormat::Table => {
                    println!(
                        "Paper trading enabled. Virtual balance: ${}.",
                        account.cash.round_dp(2)
                    );
                    println!(
                        "`clob create-order` and `clob market-order` now simulate fills — no real funds move."
                    );
                }
                OutputFormat::Json => {
                    crate::output::print_json(
                        &serde_json::json!({"enabled": true, "cash": account.cash}),
                    )?;
                }
            }
        }

        PaperCommand::Disable => {
            let mut account = store::load_required()?;
            account.enabled = false;
            store::save(&account)?;
            match output {
                OutputFormat::Table => {
                    println!("Paper trading disabled. Order commands hit the live CLOB again.");
                    println!("Account data kept; re-enable with `polymarket paper enable`.");
                }
                OutputFormat::Json => {
                    crate::output::print_json(&serde_json::json!({"enabled": false}))?;
                }
            }
        }

        PaperCommand::Status => {
            let account = store::load()?;
            print_status(account.as_ref(), &store::account_path()?, output)?;
        }

        PaperCommand::Reset { balance } => {
            let balance = parse_decimal(&balance, "balance")?;
            if balance <= Decimal::ZERO {
                bail!("Balance must be positive, got {balance}");
            }
            let enabled = store::load()?.is_some_and(|a| a.enabled);
            let account = PaperAccount::new(balance, enabled);
            store::save_force(&account)?; // reset resets next_id; bypass the stale-write guard
            match output {
                OutputFormat::Table => {
                    println!(
                        "Paper account reset. Virtual balance: ${}. Paper mode: {}.",
                        balance.round_dp(2),
                        if enabled { "enabled" } else { "disabled" }
                    );
                }
                OutputFormat::Json => {
                    crate::output::print_json(
                        &serde_json::json!({"reset": true, "balance": balance, "enabled": enabled}),
                    )?;
                }
            }
        }

        PaperCommand::Buy {
            token_id,
            amount,
            price,
            size,
        } => match (amount, price, size) {
            (Some(amount), None, None) => {
                market_buy(&token_id, &amount, output).await?;
            }
            (None, Some(price), Some(size)) => {
                limit_order(
                    &token_id,
                    crate::paper::types::TradeSide::Buy,
                    &price,
                    &size,
                    output,
                )
                .await?;
            }
            _ => bail!("Specify either --amount (market buy) or --price and --size (limit buy)"),
        },

        PaperCommand::Sell {
            token_id,
            size,
            price,
        } => match price {
            None => market_sell(&token_id, &size, output).await?,
            Some(price) => {
                limit_order(
                    &token_id,
                    crate::paper::types::TradeSide::Sell,
                    &price,
                    &size,
                    output,
                )
                .await?;
            }
        },

        PaperCommand::Portfolio => {
            let mut account = store::load_required()?;
            let client = auth::unauthenticated_clob_client()?;
            let fills = quotes::settle_resting_orders(&mut account, &client).await?;
            print_settled_fills(&fills, output);

            let tokens: Vec<String> = account.positions.keys().cloned().collect();
            let marks = quotes::fetch_marks(&client, &tokens).await?;
            let view = engine::portfolio_view(&account, &marks);
            print_portfolio(&view, output)?;
        }

        PaperCommand::History { limit } => {
            let mut account = store::load_required()?;
            let client = auth::unauthenticated_clob_client()?;
            let fills = quotes::settle_resting_orders(&mut account, &client).await?;
            print_settled_fills(&fills, output);

            let mut trades = account.trades;
            trades.reverse(); // most recent first
            trades.truncate(limit);
            print_history(&trades, output)?;
        }

        PaperCommand::Orders => {
            let mut account = store::load_required()?;
            let client = auth::unauthenticated_clob_client()?;
            let fills = quotes::settle_resting_orders(&mut account, &client).await?;
            print_settled_fills(&fills, output);
            print_open_orders(&account.open_orders, output)?;
        }

        PaperCommand::Cancel { order_id } => {
            let mut account = store::load_required()?;
            let order = engine::cancel_order(&mut account, order_id)?;
            store::save(&account)?;
            match output {
                OutputFormat::Table => {
                    println!(
                        "Cancelled paper order {}: {} {} @ {}. Cash: ${}.",
                        order.id,
                        order.side,
                        order.size.round_dp(2),
                        order.price,
                        account.cash.round_dp(2)
                    );
                }
                OutputFormat::Json => {
                    crate::output::print_json(
                        &serde_json::json!({"cancelled": order, "cash_after": account.cash}),
                    )?;
                }
            }
        }

        PaperCommand::Settle { token_id, payout } => {
            let token = quotes::parse_token_id(&token_id)?;
            let token_id = token.to_string();
            // Explicit --payout wins; otherwise auto-resolve from Gamma using the
            // same resolution check the TUI trusts (only fires once finalized).
            let payout = match payout {
                Some(p) => parse_decimal(&p, "payout")?,
                None => {
                    let gamma = polymarket_client_sdk_v2::gamma::Client::default();
                    let res = crate::tui::data::fetch_resolutions(
                        &gamma,
                        std::slice::from_ref(&token_id),
                    )
                    .await?;
                    match res.get(&token_id) {
                        Some(info) => info.payout,
                        None => bail!(
                            "Market for token {token_id} hasn't finalized yet; \
                             pass --payout 0|1 to force settlement"
                        ),
                    }
                }
            };
            let mut account = store::load_required()?;
            let trade = engine::settle_position(&mut account, &token_id, payout, Utc::now())?;
            store::save(&account)?;
            match output {
                OutputFormat::Table => {
                    println!(
                        "Settled {} shares of {} at ${} payout. Cash: ${}.",
                        trade.size.round_dp(2),
                        token_id,
                        payout.round_dp(2),
                        account.cash.round_dp(2)
                    );
                }
                OutputFormat::Json => {
                    crate::output::print_json(
                        &serde_json::json!({"settled": trade, "cash_after": account.cash}),
                    )?;
                }
            }
        }

        PaperCommand::Stats => {
            let mut account = store::load_required()?;
            let client = auth::unauthenticated_clob_client()?;
            let fills = quotes::settle_resting_orders(&mut account, &client).await?;
            print_settled_fills(&fills, output);
            print_stats(&engine::compute_stats(&account), output)?;
        }

        PaperCommand::Snapshot { command } => match command {
            SnapshotCommand::Save { name } => {
                let path = store::snapshot_save(&name)?;
                match output {
                    OutputFormat::Table => println!("Saved snapshot '{name}' → {}", path.display()),
                    OutputFormat::Json => {
                        crate::output::print_json(&serde_json::json!({"saved": name}))?
                    }
                }
            }
            SnapshotCommand::Restore { name } => {
                store::snapshot_restore(&name)?;
                match output {
                    OutputFormat::Table => {
                        println!("Restored snapshot '{name}'. Restart the TUI if it's open.")
                    }
                    OutputFormat::Json => {
                        crate::output::print_json(&serde_json::json!({"restored": name}))?
                    }
                }
            }
            SnapshotCommand::List => {
                let names = store::snapshot_list()?;
                match output {
                    OutputFormat::Table => {
                        if names.is_empty() {
                            println!("No snapshots. Save one with `paper snapshot save <name>`.");
                        } else {
                            for n in &names {
                                println!("{n}");
                            }
                        }
                    }
                    OutputFormat::Json => {
                        crate::output::print_json(&serde_json::json!({"snapshots": names}))?
                    }
                }
            }
        },

        PaperCommand::Export { what } => {
            let account = store::load_required()?;
            let (name, csv) = match what {
                ExportKind::Trades => ("trades", export_trades(&account)),
                ExportKind::Positions => ("positions", export_positions(&account)),
                ExportKind::History => ("history", export_history(&account)),
            };
            let path = write_export(name, &csv)?;
            match output {
                OutputFormat::Table => {
                    println!(
                        "Wrote {} rows to {}",
                        csv.lines().count() - 1,
                        path.display()
                    );
                }
                OutputFormat::Json => {
                    crate::output::print_json(&serde_json::json!({
                        "path": path.display().to_string(),
                        "rows": csv.lines().count() - 1,
                    }))?;
                }
            }
        }
    }

    Ok(())
}

/// Save a CSV to `~/Desktop/paper-<name>-<timestamp>.csv` (falls back to the
/// home dir if there's no Desktop) and return the path written.
fn write_export(name: &str, csv: &str) -> Result<std::path::PathBuf> {
    let dir = dirs::desktop_dir()
        .or_else(dirs::home_dir)
        .context("Could not determine Desktop or home directory")?;
    let file = format!("paper-{}-{}.csv", name, Utc::now().format("%Y%m%d-%H%M%S"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(file);
    std::fs::write(&path, csv).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn export_trades(account: &PaperAccount) -> String {
    let mut out = String::from(
        "id,timestamp,token_id,question,outcome,side,kind,size,price,notional,realized_pnl\n",
    );
    for t in &account.trades {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{}\n",
            t.id,
            t.timestamp.to_rfc3339(),
            t.token_id,
            csv_field(&t.question),
            csv_field(&t.outcome),
            t.side,
            t.kind,
            t.size,
            t.price,
            t.notional,
            t.realized_pnl.map(|p| p.to_string()).unwrap_or_default(),
        ));
    }
    out
}

fn export_positions(account: &PaperAccount) -> String {
    let mut out = String::from("token_id,question,outcome,size,avg_price,realized_pnl\n");
    for p in account.positions.values() {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            p.token_id,
            csv_field(&p.question),
            csv_field(&p.outcome),
            p.size,
            p.avg_price,
            p.realized_pnl,
        ));
    }
    out
}

/// Per-market history aggregated from the trade log (closed positions are
/// dropped from `positions`, so trades are the only complete record).
/// original_size = shares bought; final_size = shares still held;
/// pnl = summed realized PnL; roi = pnl / cost basis.
fn export_history(account: &PaperAccount) -> String {
    // (token_id, question, outcome, bought, sold, cost_basis, pnl); Vec keeps
    // first-seen order. IMPORTANT NOTE: O(n²) linear find, fine for a paper log.
    let mut rows: Vec<(String, String, String, Decimal, Decimal, Decimal, Decimal)> = Vec::new();
    for t in &account.trades {
        let row = match rows.iter_mut().find(|r| r.0 == t.token_id) {
            Some(r) => r,
            None => {
                rows.push((
                    t.token_id.clone(),
                    t.question.clone(),
                    t.outcome.clone(),
                    Decimal::ZERO,
                    Decimal::ZERO,
                    Decimal::ZERO,
                    Decimal::ZERO,
                ));
                rows.last_mut().unwrap()
            }
        };
        match t.side {
            crate::paper::types::TradeSide::Buy => {
                row.3 += t.size;
                row.5 += t.notional;
            }
            crate::paper::types::TradeSide::Sell => row.4 += t.size,
        }
        row.6 += t.realized_pnl.unwrap_or(Decimal::ZERO);
    }
    let mut out = String::from("token_id,question,outcome,original_size,final_size,roi_pct,pnl\n");
    for (token_id, question, outcome, bought, sold, basis, pnl) in rows {
        let roi = if basis > Decimal::ZERO {
            (pnl / basis * Decimal::from(100)).round_dp(2).to_string()
        } else {
            String::new()
        };
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            token_id,
            csv_field(&question),
            csv_field(&outcome),
            bought,
            bought - sold,
            roi,
            pnl,
        ));
    }
    out
}

/// Quote a CSV field only when it needs it (comma, quote, or newline). Questions
/// routinely contain commas, so this is not optional.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Simulated market buy with a pUSD budget. Also the entry point for
/// `clob market-order --side buy` when paper mode is on.
pub(crate) async fn market_buy(token_id: &str, amount: &str, output: &OutputFormat) -> Result<()> {
    let amount = parse_decimal(amount, "amount")?;
    let token = quotes::parse_token_id(token_id)?;
    // Canonical decimal form: accepts hex or decimal input, and must match
    // the asset IDs the CLOB returns when settling resting orders.
    let token_id = token.to_string();
    let token_id = token_id.as_str();
    let mut account = store::load_required()?;
    let client = auth::unauthenticated_clob_client()?;

    let fills = quotes::settle_resting_orders(&mut account, &client).await?;
    print_settled_fills(&fills, output);

    let gamma = polymarket_client_sdk_v2::gamma::Client::default();
    let meta = quotes::fetch_meta(&gamma, token).await;
    let book = quotes::fetch_book(&client, token).await?;

    let trade = engine::market_buy(
        &mut account,
        token_id,
        &meta,
        &book.asks,
        &book.bids,
        amount,
        Utc::now(),
    )?;
    store::save(&account)?;
    print_fill(&trade, account.cash, output)
}

/// Simulated market sell of shares. Also the entry point for
/// `clob market-order --side sell` when paper mode is on.
pub(crate) async fn market_sell(token_id: &str, size: &str, output: &OutputFormat) -> Result<()> {
    let size = parse_decimal(size, "size")?;
    let token = quotes::parse_token_id(token_id)?;
    let token_id = token.to_string();
    let token_id = token_id.as_str();
    let mut account = store::load_required()?;
    let client = auth::unauthenticated_clob_client()?;

    let fills = quotes::settle_resting_orders(&mut account, &client).await?;
    print_settled_fills(&fills, output);

    let book = quotes::fetch_book(&client, token).await?;
    let trade = engine::market_sell(&mut account, token_id, &book.bids, size, Utc::now())?;
    store::save(&account)?;
    print_fill(&trade, account.cash, output)
}

/// Simulated limit order. Also the entry point for `clob create-order` when
/// paper mode is on.
pub(crate) async fn limit_order(
    token_id: &str,
    side: crate::paper::types::TradeSide,
    price: &str,
    size: &str,
    output: &OutputFormat,
) -> Result<()> {
    let price = parse_decimal(price, "price")?;
    let size = parse_decimal(size, "size")?;
    let token = quotes::parse_token_id(token_id)?;
    let token_id = token.to_string();
    let token_id = token_id.as_str();
    let mut account = store::load_required()?;
    let client = auth::unauthenticated_clob_client()?;

    let fills = quotes::settle_resting_orders(&mut account, &client).await?;
    print_settled_fills(&fills, output);

    let book = quotes::fetch_book(&client, token).await?;
    let quote = book.quote();
    let outcome = match side {
        crate::paper::types::TradeSide::Buy => {
            let gamma = polymarket_client_sdk_v2::gamma::Client::default();
            let meta = quotes::fetch_meta(&gamma, token).await;
            engine::limit_buy(
                &mut account,
                token_id,
                &meta,
                quote,
                price,
                size,
                Utc::now(),
            )?
        }
        crate::paper::types::TradeSide::Sell => {
            engine::limit_sell(&mut account, token_id, quote, price, size, Utc::now())?
        }
    };
    store::save(&account)?;

    match outcome {
        engine::LimitOutcome::Filled(trade) => print_fill(&trade, account.cash, output),
        engine::LimitOutcome::Resting(order) => print_resting(&order, account.cash, output),
    }
}

fn parse_decimal(s: &str, label: &str) -> Result<Decimal> {
    Decimal::from_str(s).map_err(|_| anyhow::anyhow!("Invalid {label}: {s}"))
}
