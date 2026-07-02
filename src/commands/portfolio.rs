use std::collections::BTreeMap;

use anyhow::Result;
use chrono::Utc;
use clap::Args;
use crossterm::style::Stylize;
use polymarket_client_sdk_v2::types::Decimal;
use tabled::settings::Style;
use tabled::{Table, Tabled};

use crate::output::paper::{money, signed_money};
use crate::output::{DASH, OutputFormat, print_json, truncate};

fn style_pnl(d: Decimal) -> String {
    if d > Decimal::ZERO {
        signed_money(d).green().to_string()
    } else if d < Decimal::ZERO {
        signed_money(d).red().to_string()
    } else {
        signed_money(d)
    }
}
use crate::paper;
use crate::paper::types::PaperAccount;

#[derive(Args)]
pub struct PortfolioArgs {
    /// Show the paper-trading portfolio instead of live
    #[arg(long)]
    pub paper: bool,
}

pub async fn execute(args: PortfolioArgs, output: OutputFormat) -> Result<()> {
    if args.paper {
        paper_portfolio(&output).await
    } else {
        live_portfolio(&output).await
    }
}

fn colored_signed(d: Decimal) -> String {
    style_pnl(d)
}

fn colored_roi(d: Decimal) -> String {
    let s = format!("{}%", d);
    if d > Decimal::ZERO {
        s.green().to_string()
    } else if d < Decimal::ZERO {
        s.red().to_string()
    } else {
        s
    }
}

fn fmt_win_rate(rate: Decimal, wins: usize, losses: usize) -> String {
    format!("{:.2}% ({}W/{}L)", rate, wins, losses)
}

fn daily_pnl(acct: &PaperAccount) -> Decimal {
    let today = Utc::now().date_naive();
    acct.trades
        .iter()
        .filter(|t| t.timestamp.date_naive() == today)
        .filter_map(|t| t.realized_pnl)
        .sum()
}

struct TradeStats {
    wins: usize,
    losses: usize,
    win_rate: Decimal,
    avg_win: Decimal,
    avg_loss: Decimal,
    profit_factor: Option<Decimal>,
    expectancy: Decimal,
}

fn trade_stats(acct: &PaperAccount) -> TradeStats {
    let pnls: Vec<Decimal> = acct.trades.iter().filter_map(|t| t.realized_pnl).collect();
    let closed = pnls.len();
    if closed == 0 {
        return TradeStats {
            wins: 0,
            losses: 0,
            win_rate: Decimal::ZERO,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: None,
            expectancy: Decimal::ZERO,
        };
    }
    let wins: Vec<Decimal> = pnls
        .iter()
        .copied()
        .filter(|p| *p > Decimal::ZERO)
        .collect();
    let losses: Vec<Decimal> = pnls
        .iter()
        .copied()
        .filter(|p| *p < Decimal::ZERO)
        .collect();
    let sum_win: Decimal = wins.iter().sum();
    let sum_loss: Decimal = losses.iter().sum();
    let avg = |xs: &[Decimal], s: Decimal| {
        if xs.is_empty() {
            Decimal::ZERO
        } else {
            s / Decimal::from(xs.len())
        }
    };
    let hundred = Decimal::from(100);
    TradeStats {
        win_rate: Decimal::from(wins.len()) * hundred / Decimal::from(closed),
        wins: wins.len(),
        losses: losses.len(),
        avg_win: avg(&wins, sum_win),
        avg_loss: avg(&losses, sum_loss),
        profit_factor: if sum_loss == Decimal::ZERO {
            None
        } else {
            Some(sum_win / -sum_loss)
        },
        expectancy: pnls.iter().sum::<Decimal>() / Decimal::from(closed),
    }
}

async fn paper_portfolio(output: &OutputFormat) -> Result<()> {
    let mut account = paper::store::load_required()?;
    let client = crate::auth::unauthenticated_clob_client()?;
    let fills = paper::quotes::settle_resting_orders(&mut account, &client).await?;
    crate::output::paper::print_settled_fills(&fills, output);

    let tokens: Vec<String> = account.positions.keys().cloned().collect();
    let marks = paper::quotes::fetch_marks(&client, &tokens).await?;
    let view = paper::engine::portfolio_view(&account, &marks);
    let daily = daily_pnl(&account);
    let stats = trade_stats(&account);
    let equity_stats = equity_metrics(&account.equity_curve);
    let total_pnl = view.realized_pnl + view.unrealized_pnl;
    let open_positions = account.positions.len();
    let open_orders = account.open_orders.len();

    print_full_portfolio(
        output,
        "paper",
        &view,
        daily,
        total_pnl,
        open_positions,
        open_orders,
        &stats,
        equity_stats,
        &account.trades,
    )
}

async fn live_portfolio(output: &OutputFormat) -> Result<()> {
    let user = crate::tui::live::resolve_user_address()?;
    let cash = crate::tui::live::fetch_collateral().await?;
    let positions = crate::tui::live::fetch_positions(user).await?;
    let closed_trades = crate::tui::live::fetch_closed_trades(user).await?;

    let mut account = PaperAccount {
        version: paper::types::ACCOUNT_VERSION,
        enabled: false,
        created_at: Utc::now(),
        initial_balance: Decimal::ZERO,
        cash,
        next_id: 1,
        positions: BTreeMap::new(),
        open_orders: Vec::new(),
        trades: closed_trades,
        equity_curve: Vec::new(),
    };
    for p in positions {
        account.positions.insert(p.token_id.clone(), p);
    }

    let tokens: Vec<String> = account.positions.keys().cloned().collect();
    let client = crate::auth::unauthenticated_clob_client()?;
    let marks = paper::quotes::fetch_marks(&client, &tokens).await?;
    let view = paper::engine::portfolio_view(&account, &marks);
    let daily = daily_pnl(&account);
    let stats = trade_stats(&account);
    let open_positions = account.positions.len();

    // fetch live open-orders count
    let live_orders = crate::tui::live::fetch_open_orders()
        .await
        .unwrap_or_default();
    let open_orders_count = live_orders.len();

    let total_pnl = view.realized_pnl + view.unrealized_pnl;

    print_full_portfolio(
        output,
        "live",
        &view,
        daily,
        total_pnl,
        open_positions,
        open_orders_count,
        &stats,
        None,
        &account.trades,
    )
}

#[allow(clippy::too_many_arguments, clippy::vec_init_then_push)]
fn print_full_portfolio(
    output: &OutputFormat,
    mode: &str,
    view: &paper::types::PortfolioView,
    daily: Decimal,
    total_pnl: Decimal,
    open_positions: usize,
    open_orders: usize,
    stats: &TradeStats,
    equity_stats: Option<EquityMetrics>,
    trades: &[paper::types::Trade],
) -> Result<()> {
    match output {
        OutputFormat::Table => {
            println!("Portfolio ({})", mode);

            fn kv(label: &str, val: String) {
                println!("  {:20} {val}", label.dim());
            }
            kv("Portfolio Value", money(view.equity));
            kv("Cash Balance", money(view.cash));
            kv("Daily PnL", colored_signed(daily));
            kv("Total PnL", colored_signed(total_pnl));
            kv("Open Positions", open_positions.to_string());
            kv("Open Orders", open_orders.to_string());
            kv("ROI", colored_roi(view.roi_pct));
            kv("Realized PnL", colored_signed(view.realized_pnl));
            kv("Unrealized PnL", colored_signed(view.unrealized_pnl));
            kv(
                "Win Rate",
                fmt_win_rate(stats.win_rate, stats.wins, stats.losses),
            );
            kv("Avg Win", colored_signed(stats.avg_win));
            kv("Avg Loss", colored_signed(stats.avg_loss));
            let pf = match stats.profit_factor {
                Some(pf) => pf.round_dp(2).to_string(),
                None => "∞".into(),
            };
            kv("Profit Factor", pf);
            kv("Expectancy", colored_signed(stats.expectancy));
            if mode == "paper" {
                kv("Reserved (orders)", money(view.reserved_cash));
                kv("Initial Balance", money(view.initial_balance));
            }
            if let Some(eq) = equity_stats
                && let Some(sh) = eq.sharpe
            {
                kv("Sharpe", sh.to_string());
            }

            if view.positions.is_empty() {
                println!("  {}", "No open positions.".dim());
            } else {
                print_positions(view);
            }

            if !trades.is_empty() {
                print_recent_trades(trades);
            }
        }
        OutputFormat::Json => {
            let mut json = serde_json::json!({
                "mode": mode,
                "equity": view.equity,
                "cash": view.cash,
                "daily_pnl": daily,
                "total_pnl": total_pnl,
                "open_positions": open_positions,
                "open_orders": open_orders,
                "roi_pct": view.roi_pct,
                "realized_pnl": view.realized_pnl,
                "unrealized_pnl": view.unrealized_pnl,
                "positions": view.positions,
                "trade_stats": {
                    "wins": stats.wins,
                    "losses": stats.losses,
                    "win_rate_pct": stats.win_rate,
                    "avg_win": stats.avg_win,
                    "avg_loss": stats.avg_loss,
                    "profit_factor": stats.profit_factor,
                    "expectancy": stats.expectancy,
                },
                "recent_trades": &trades.iter().rev().take(8).cloned().collect::<Vec<_>>(),
            });
            if mode == "paper" {
                json["reserved_cash"] = serde_json::json!(view.reserved_cash);
                json["initial_balance"] = serde_json::json!(view.initial_balance);
            }
            if let Some(eq) = equity_stats
                && let Some(sh) = eq.sharpe
            {
                json["sharpe"] = serde_json::json!(sh);
            }
            print_json(&json)?;
        }
    }
    Ok(())
}

fn print_positions(view: &paper::types::PortfolioView) {
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "Market")]
        market: String,
        #[tabled(rename = "Outcome")]
        outcome: String,
        #[tabled(rename = "Shares")]
        shares: String,
        #[tabled(rename = "Avg Entry")]
        avg: String,
        #[tabled(rename = "Mark")]
        mark: String,
        #[tabled(rename = "Value")]
        value: String,
        #[tabled(rename = "uPnL")]
        unrealized: String,
    }
    let rows: Vec<Row> = view
        .positions
        .iter()
        .map(|p| {
            let upnl_str = p.unrealized_pnl.map_or(DASH.to_string(), signed_money);
            Row {
                market: truncate(&p.position.question, 40),
                outcome: truncate(&p.position.outcome, 12),
                shares: p.position.size.round_dp(2).to_string(),
                avg: p.position.avg_price.round_dp(4).to_string(),
                mark: p
                    .mark_price
                    .map_or(DASH.to_string(), |m| m.round_dp(4).to_string()),
                value: p.market_value.map_or(DASH.to_string(), money),
                unrealized: upnl_str,
            }
        })
        .collect();
    let table = Table::new(rows).with(Style::rounded()).to_string();
    println!("\nOpen Positions:");
    println!("{table}");
}

fn print_recent_trades(trades: &[paper::types::Trade]) {
    #[derive(Tabled)]
    struct Row {
        #[tabled(rename = "Time")]
        time: String,
        #[tabled(rename = "Side")]
        side: String,
        #[tabled(rename = "Market")]
        market: String,
        #[tabled(rename = "Size")]
        size: String,
        #[tabled(rename = "Price")]
        price: String,
        #[tabled(rename = "PnL")]
        pnl: String,
    }
    let recent: Vec<_> = trades.iter().rev().take(8).collect();
    let rows: Vec<Row> = recent
        .iter()
        .map(|t| {
            let pnl_str = t.realized_pnl.map_or(DASH.to_string(), signed_money);
            Row {
                time: t.timestamp.format("%H:%M:%S").to_string(),
                side: match t.side {
                    paper::types::TradeSide::Buy => "BUY".into(),
                    paper::types::TradeSide::Sell => "SELL".into(),
                },
                market: truncate(&t.question, 30),
                size: t.size.round_dp(1).to_string(),
                price: format!("{:.3}", t.price),
                pnl: pnl_str,
            }
        })
        .collect();
    let table = Table::new(rows).with(Style::rounded()).to_string();
    println!("\nRecent Trades:");
    println!("{table}");
}

struct EquityMetrics {
    sharpe: Option<Decimal>,
}

fn equity_metrics(curve: &[(chrono::DateTime<chrono::Utc>, Decimal)]) -> Option<EquityMetrics> {
    if curve.len() < 11 {
        return None;
    }
    let eq: Vec<f64> = curve
        .iter()
        .map(|&(_, e)| f64::try_from(e).unwrap_or(0.0))
        .collect();
    let returns: Vec<f64> = eq
        .windows(2)
        .filter(|w| w[0] != 0.0)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect();
    let sharpe = if returns.is_empty() {
        None
    } else {
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
        let sd = var.sqrt();
        (sd > 0.0).then(|| Decimal::try_from(mean / sd).unwrap_or_default().round_dp(2))
    };
    Some(EquityMetrics { sharpe })
}
