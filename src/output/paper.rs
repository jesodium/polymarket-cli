use polymarket_client_sdk_v2::types::Decimal;
use serde_json::json;
use tabled::settings::Style;
use tabled::{Table, Tabled};

use crate::output::{DASH, OutputFormat, print_detail_table, print_json, truncate};
use crate::paper::types::{OpenOrder, PaperAccount, PortfolioView, Stats, Trade};

pub(crate) fn money(d: Decimal) -> String {
    format!(
        "${:.2}",
        d.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
    )
}

pub(crate) fn signed_money(d: Decimal) -> String {
    if d < Decimal::ZERO {
        format!("-{}", money(-d))
    } else {
        money(d)
    }
}

/// Notice shown when a `clob` order command was routed to the simulator.
pub fn print_paper_notice(output: &OutputFormat) {
    if matches!(output, OutputFormat::Table) {
        println!("[paper] Simulated order — uses the paper account, no real funds move.");
        println!("[paper] Live trading: `polymarket paper disable` (and drop --paper).");
    }
}

pub fn print_fill(trade: &Trade, cash_after: Decimal, output: &OutputFormat) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => {
            let mut rows = vec![
                ["Status".into(), "FILLED (paper)".to_string()],
                ["Trade ID".into(), trade.id.to_string()],
                ["Market".into(), trade.question.clone()],
                ["Outcome".into(), trade.outcome.clone()],
                ["Side".into(), trade.side.to_string()],
                ["Type".into(), trade.kind.to_string()],
                ["Shares".into(), trade.size.round_dp(4).to_string()],
                ["Price".into(), trade.price.round_dp(4).to_string()],
                ["Notional".into(), money(trade.notional)],
            ];
            if let Some(pnl) = trade.realized_pnl {
                rows.push(["Realized PnL".into(), signed_money(pnl)]);
            }
            rows.push(["Cash after".into(), money(cash_after)]);
            rows.push(["Time".into(), crate::output::format_date(&trade.timestamp)]);
            print_detail_table(rows);
        }
        OutputFormat::Json => {
            print_json(&json!({
                "paper": true,
                "status": "filled",
                "trade": trade,
                "cash_after": cash_after,
            }))?;
        }
    }
    Ok(())
}

pub fn print_resting(
    order: &OpenOrder,
    cash_after: Decimal,
    output: &OutputFormat,
) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => {
            let rows = vec![
                ["Status".into(), "OPEN (paper limit order)".to_string()],
                ["Order ID".into(), order.id.to_string()],
                ["Market".into(), order.question.clone()],
                ["Outcome".into(), order.outcome.clone()],
                ["Side".into(), order.side.to_string()],
                ["Limit price".into(), order.price.to_string()],
                ["Shares".into(), order.size.round_dp(4).to_string()],
                ["Cash after".into(), money(cash_after)],
            ];
            print_detail_table(rows);
            println!("Fills when the market crosses your price (checked on each paper command).");
        }
        OutputFormat::Json => {
            print_json(&json!({
                "paper": true,
                "status": "open",
                "order": order,
                "cash_after": cash_after,
            }))?;
        }
    }
    Ok(())
}

/// Report limit orders that filled while the CLI was idle.
pub fn print_settled_fills(fills: &[Trade], output: &OutputFormat) {
    if fills.is_empty() {
        return;
    }
    if matches!(output, OutputFormat::Table) {
        for fill in fills {
            println!(
                "[paper] Limit order filled: {} {} {} @ {} ({})",
                fill.side,
                fill.size.round_dp(2),
                truncate(&fill.question, 40),
                fill.price,
                fill.outcome
            );
        }
    }
}

pub fn print_portfolio(view: &PortfolioView, output: &OutputFormat) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => {
            let rows = vec![
                ["Cash".into(), money(view.cash)],
                ["Reserved (open orders)".into(), money(view.reserved_cash)],
                ["Positions value".into(), money(view.positions_value)],
                ["Total equity".into(), money(view.equity)],
                ["Realized PnL".into(), signed_money(view.realized_pnl)],
                ["Unrealized PnL".into(), signed_money(view.unrealized_pnl)],
                ["ROI".into(), format!("{}%", view.roi_pct)],
                ["Initial balance".into(), money(view.initial_balance)],
                ["Open orders".into(), view.open_orders.to_string()],
            ];
            print_detail_table(rows);

            if view.positions.is_empty() {
                println!("No open positions.");
                return Ok(());
            }

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
                #[tabled(rename = "Unrealized")]
                unrealized: String,
            }
            let rows: Vec<Row> = view
                .positions
                .iter()
                .map(|p| Row {
                    market: truncate(&p.position.question, 40),
                    outcome: truncate(&p.position.outcome, 12),
                    shares: p.position.size.round_dp(2).to_string(),
                    avg: p.position.avg_price.round_dp(4).to_string(),
                    mark: p
                        .mark_price
                        .map_or(DASH.to_string(), |m| m.round_dp(4).to_string()),
                    value: p.market_value.map_or(DASH.to_string(), money),
                    unrealized: p.unrealized_pnl.map_or(DASH.to_string(), signed_money),
                })
                .collect();
            let table = Table::new(rows).with(Style::rounded()).to_string();
            println!("{table}");
        }
        OutputFormat::Json => print_json(view)?,
    }
    Ok(())
}

pub fn print_history(trades: &[Trade], output: &OutputFormat) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => {
            if trades.is_empty() {
                println!("No paper trades yet.");
                return Ok(());
            }
            #[derive(Tabled)]
            struct Row {
                #[tabled(rename = "ID")]
                id: u64,
                #[tabled(rename = "Time")]
                time: String,
                #[tabled(rename = "Market")]
                market: String,
                #[tabled(rename = "Outcome")]
                outcome: String,
                #[tabled(rename = "Side")]
                side: String,
                #[tabled(rename = "Type")]
                kind: String,
                #[tabled(rename = "Shares")]
                shares: String,
                #[tabled(rename = "Price")]
                price: String,
                #[tabled(rename = "Realized PnL")]
                pnl: String,
            }
            let rows: Vec<Row> = trades
                .iter()
                .map(|t| Row {
                    id: t.id,
                    time: crate::output::format_date(&t.timestamp),
                    market: truncate(&t.question, 32),
                    outcome: truncate(&t.outcome, 12),
                    side: t.side.to_string(),
                    kind: t.kind.to_string(),
                    shares: t.size.round_dp(2).to_string(),
                    price: t.price.round_dp(4).to_string(),
                    pnl: t.realized_pnl.map_or(DASH.to_string(), signed_money),
                })
                .collect();
            let table = Table::new(rows).with(Style::rounded()).to_string();
            println!("{table}");
        }
        OutputFormat::Json => print_json(trades)?,
    }
    Ok(())
}

pub fn print_open_orders(orders: &[OpenOrder], output: &OutputFormat) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => {
            if orders.is_empty() {
                println!("No open paper orders.");
                return Ok(());
            }
            #[derive(Tabled)]
            struct Row {
                #[tabled(rename = "ID")]
                id: u64,
                #[tabled(rename = "Created")]
                created: String,
                #[tabled(rename = "Market")]
                market: String,
                #[tabled(rename = "Outcome")]
                outcome: String,
                #[tabled(rename = "Side")]
                side: String,
                #[tabled(rename = "Limit")]
                price: String,
                #[tabled(rename = "Shares")]
                shares: String,
            }
            let rows: Vec<Row> = orders
                .iter()
                .map(|o| Row {
                    id: o.id,
                    created: crate::output::format_date(&o.created_at),
                    market: truncate(&o.question, 36),
                    outcome: truncate(&o.outcome, 12),
                    side: o.side.to_string(),
                    price: o.price.to_string(),
                    shares: o.size.round_dp(2).to_string(),
                })
                .collect();
            let table = Table::new(rows).with(Style::rounded()).to_string();
            println!("{table}");
        }
        OutputFormat::Json => print_json(orders)?,
    }
    Ok(())
}

pub fn print_stats(stats: &Stats, output: &OutputFormat) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => {
            let describe_trade = |t: &Trade| {
                format!(
                    "{} on {}",
                    signed_money(t.realized_pnl.unwrap_or_default()),
                    truncate(&t.question, 40)
                )
            };
            let rows = vec![
                ["Total trades".into(), stats.total_trades.to_string()],
                [
                    "Buys / Sells".into(),
                    format!("{} / {}", stats.buys, stats.sells),
                ],
                [
                    "Win rate".into(),
                    stats.win_rate_pct.map_or(DASH.to_string(), |w| {
                        format!("{w}% ({} W / {} L)", stats.wins, stats.losses)
                    }),
                ],
                ["Realized PnL".into(), signed_money(stats.realized_pnl)],
                ["Volume traded".into(), money(stats.volume)],
                [
                    "Best trade".into(),
                    stats
                        .best_trade
                        .as_ref()
                        .map_or(DASH.to_string(), describe_trade),
                ],
                [
                    "Worst trade".into(),
                    stats
                        .worst_trade
                        .as_ref()
                        .map_or(DASH.to_string(), describe_trade),
                ],
            ];
            print_detail_table(rows);

            if !stats.daily_pnl.is_empty() {
                #[derive(Tabled)]
                struct Row {
                    #[tabled(rename = "Date")]
                    date: String,
                    #[tabled(rename = "Realized PnL")]
                    pnl: String,
                    #[tabled(rename = "Equity (realized)")]
                    equity: String,
                }
                let rows: Vec<Row> = stats
                    .daily_pnl
                    .iter()
                    .zip(&stats.equity_curve)
                    .map(|(&(date, pnl), &(_, equity))| Row {
                        date: date.to_string(),
                        pnl: signed_money(pnl),
                        equity: money(equity),
                    })
                    .collect();
                let table = Table::new(rows).with(Style::rounded()).to_string();
                println!("Daily performance:");
                println!("{table}");
                if stats.equity_curve.len() > 1 {
                    println!(
                        "Equity curve: {}",
                        sparkline(
                            &stats
                                .equity_curve
                                .iter()
                                .map(|&(_, e)| e)
                                .collect::<Vec<_>>()
                        )
                    );
                }
            }
        }
        OutputFormat::Json => print_json(stats)?,
    }
    Ok(())
}

pub fn print_status(
    account: Option<&PaperAccount>,
    path: &std::path::Path,
    output: &OutputFormat,
) -> anyhow::Result<()> {
    match output {
        OutputFormat::Table => match account {
            Some(a) => {
                let rows = vec![
                    [
                        "Paper mode".into(),
                        if a.enabled { "ENABLED" } else { "DISABLED" }.to_string(),
                    ],
                    ["Cash".into(), money(a.cash)],
                    ["Initial balance".into(), money(a.initial_balance)],
                    ["Positions".into(), a.positions.len().to_string()],
                    ["Open orders".into(), a.open_orders.len().to_string()],
                    ["Trades".into(), a.trades.len().to_string()],
                    ["Created".into(), crate::output::format_date(&a.created_at)],
                    ["Data file".into(), path.display().to_string()],
                ];
                print_detail_table(rows);
            }
            None => println!("{}", crate::paper::store::NO_ACCOUNT_MSG),
        },
        OutputFormat::Json => match account {
            Some(a) => print_json(&json!({
                "enabled": a.enabled,
                "cash": a.cash,
                "initial_balance": a.initial_balance,
                "positions": a.positions.len(),
                "open_orders": a.open_orders.len(),
                "trades": a.trades.len(),
                "created_at": a.created_at,
                "file": path.display().to_string(),
            }))?,
            None => print_json(&json!({"enabled": false, "exists": false}))?,
        },
    }
    Ok(())
}

/// Unicode sparkline of a numeric series.
fn sparkline(values: &[Decimal]) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let Some(min) = values.iter().min().copied() else {
        return String::new();
    };
    let max = values.iter().max().copied().unwrap_or(min);
    let range = max - min;
    values
        .iter()
        .map(|v| {
            if range == Decimal::ZERO {
                BARS[3]
            } else {
                use rust_decimal::prelude::ToPrimitive as _;
                let idx = ((v - min) / range * Decimal::from(BARS.len() - 1))
                    .round()
                    .to_usize()
                    .unwrap_or(0)
                    .min(BARS.len() - 1);
                BARS[idx]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn signed_money_formats_negative() {
        assert_eq!(signed_money(dec!(-12.345)), "-$12.35");
        assert_eq!(signed_money(dec!(12.3)), "$12.30");
    }

    #[test]
    fn sparkline_spans_range() {
        let s = sparkline(&[dec!(0), dec!(50), dec!(100)]);
        assert_eq!(s.chars().count(), 3);
        assert!(s.starts_with('▁'));
        assert!(s.ends_with('█'));
    }

    #[test]
    fn sparkline_flat_series_is_midline() {
        assert_eq!(sparkline(&[dec!(5), dec!(5)]), "▄▄");
    }
}
