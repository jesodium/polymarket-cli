use std::str::FromStr;

use anyhow::{Result, bail};
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

    /// Performance analytics: win rate, best/worst trade, daily PnL
    Stats,
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
            store::save(&account)?;
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

        PaperCommand::Stats => {
            let mut account = store::load_required()?;
            let client = auth::unauthenticated_clob_client()?;
            let fills = quotes::settle_resting_orders(&mut account, &client).await?;
            print_settled_fills(&fills, output);
            print_stats(&engine::compute_stats(&account), output)?;
        }
    }

    Ok(())
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
